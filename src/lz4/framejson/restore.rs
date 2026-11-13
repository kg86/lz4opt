use std::io::{Read, Write};

use serde_json::Value;

use super::errors::{quote_string, RestoreError, SchemaError};
use super::restore_schema::{decode_restore_document, field_path, json_type_name};

/// One physical field collected by walking a restore JSON document: a JSON
/// object carrying both "offset" and "hex" directly. `path` identifies the
/// JSON object it was collected from, for layout error messages.
struct RestoreField {
    offset: u64,
    raw: Vec<u8>,
    path: String,
}

/// Reads a restore JSON document from `r`, validates it, and writes the LZ4
/// bytes it describes to `w`.
///
/// `r` is first decoded and schema-checked by [`decode_restore_document`]. The
/// "frames" array is then walked recursively to collect every physical field:
/// a JSON object carrying both "offset" and "hex" directly (an object with
/// "hex" but no "offset" is rejected; one with "offset" but no "hex" is an
/// explanatory value and excluded from concatenation, though it is still
/// walked for nested physical fields). Root-level members outside "frames" are
/// never walked.
///
/// While walking, every array is required to present the physical fields
/// nested under its elements in offset order: for two elements adjacent in the
/// array, the maximum offset among the first element's physical fields must be
/// at most the minimum offset among the second element's (equal offsets are
/// allowed). Elements with no physical fields at all take no part in this
/// check. This constrains array member order -- "frames", "data_blocks",
/// "data", and any array a future schema adds -- without using that order to
/// decide the restored bytes themselves: offset alone still does that, via the
/// sort in `layout_restore_fields`. JSON object member order is never
/// constrained or used.
///
/// Every field's offset and hex are validated, and the full set must lay out a
/// contiguous, non-overlapping byte range starting at offset 0: zero or more
/// zero-length fields and at most one non-empty field may share an offset, and
/// at least one field must be present. Nothing is written to `w` unless every
/// field validates and the array order and layout checks succeed: a decode,
/// validation, or layout failure leaves `w` untouched, though a failing `w` may
/// itself have received part of the output, since `restore` does not control
/// `w`'s own semantics. Attributes other than "offset" and "hex" (value, size,
/// type) play no role in what bytes are written.
pub fn restore(mut w: impl Write, r: impl Read) -> Result<(), RestoreError> {
    let doc = decode_restore_document(r)?;

    let fields = collect_restore_fields(&doc["frames"], "$.frames")?;
    if fields.is_empty() {
        return Err(SchemaError::new(
            "$.frames",
            "no physical fields found: expected at least one offset/hex object starting at offset 0",
        )
        .into());
    }

    let out = layout_restore_fields(fields)?;
    w.write_all(&out).map_err(RestoreError::Io)
}

/// Recursively walks `v`, a value from within a "frames" element, collecting
/// every object that carries both "offset" and "hex" as a physical field. A
/// matched field's own members are not walked further: its offset/hex pair is
/// the whole field, and sibling attributes (value, size, type, ...) are
/// derived or explanatory and carry no independent bytes.
///
/// Every array walked is required to present the physical fields nested under
/// its elements in offset order: see array order in `restore`'s doc comment
/// for the exact rule. Object member order is never checked or used, so this
/// recursion's result does not depend on the order members are visited: each
/// key's contribution is validated independently before being merged, and the
/// merge itself is order-insensitive.
fn collect_restore_fields(v: &Value, path: &str) -> Result<Vec<RestoreField>, SchemaError> {
    match v {
        Value::Object(t) => {
            let (offset_val, hex_val) = (t.get("offset"), t.get("hex"));

            match (offset_val, hex_val) {
                (None, Some(_)) => Err(SchemaError::new(field_path(path, "hex"), "hex present without offset")),
                (Some(offset_val), Some(hex_val)) => Ok(vec![new_restore_field(offset_val, hex_val, path)?]),
                _ => {
                    let mut out = Vec::new();
                    for (k, val) in t {
                        out.extend(collect_restore_fields(val, &field_path(path, k))?);
                    }
                    Ok(out)
                }
            }
        }
        Value::Array(t) => {
            let mut out = Vec::new();
            let mut prev_max = None;
            for (i, val) in t.iter().enumerate() {
                let elem_path = format!("{path}[{i}]");
                let sub = collect_restore_fields(val, &elem_path)?;
                if sub.is_empty() {
                    // An element with no physical fields of its own (an
                    // explanatory value, or one whose nested fields were all
                    // excluded) takes no part in the order check.
                    continue;
                }
                let (elem_min, elem_max) = min_max_offset(&sub);
                if let Some(prev_max) = prev_max {
                    if elem_min < prev_max {
                        return Err(SchemaError::new(
                            elem_path,
                            format!(
                                "array elements are not in offset order: this element's minimum offset {elem_min} precedes a preceding element's maximum offset {prev_max}"
                            ),
                        ));
                    }
                }
                prev_max = Some(elem_max);
                out.extend(sub);
            }
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

/// Returns the smallest and largest offset among `fields`, which must be
/// non-empty.
fn min_max_offset(fields: &[RestoreField]) -> (u64, u64) {
    let offsets = fields.iter().map(|f| f.offset);
    (offsets.clone().min().unwrap(), offsets.max().unwrap())
}

fn new_restore_field(offset_val: &Value, hex_val: &Value, path: &str) -> Result<RestoreField, SchemaError> {
    let offset = parse_restore_offset(offset_val, &field_path(path, "offset"))?;
    let raw = parse_restore_hex(hex_val, &field_path(path, "hex"))?;
    if offset > u64::MAX - raw.len() as u64 {
        return Err(SchemaError::new(path, format!("offset {offset} plus length {} overflows u64", raw.len())));
    }
    Ok(RestoreField { offset, raw, path: path.to_string() })
}

/// Validates `v` as a JSON number written in plain decimal (no sign, fraction,
/// or exponent) that fits in a u64.
fn parse_restore_offset(v: &Value, path: &str) -> Result<u64, SchemaError> {
    let Value::Number(num) = v else {
        return Err(SchemaError::new(path, format!("must be a number, got {}", json_type_name(v))));
    };
    // With arbitrary_precision, a number's text is the document's own.
    let s = num.to_string();
    if s.is_empty() {
        return Err(SchemaError::new(path, "must not be empty"));
    }
    if !s.bytes().all(|c| c.is_ascii_digit()) {
        return Err(SchemaError::new(
            path,
            format!("must be a non-negative integer without a fraction or exponent, got {}", quote_string(&s)),
        ));
    }
    s.parse::<u64>().map_err(|_| SchemaError::new(path, format!("must fit in a u64, got {}", quote_string(&s))))
}

/// Validates `v` as an unprefixed, even-length hex string and decodes it.
fn parse_restore_hex(v: &Value, path: &str) -> Result<Vec<u8>, SchemaError> {
    let Value::String(s) = v else {
        return Err(SchemaError::new(path, format!("must be a string, got {}", json_type_name(v))));
    };
    if s.len() % 2 != 0 {
        return Err(SchemaError::new(path, format!("must have an even number of hex digits, got {}", s.len())));
    }
    decode_hex(s).map_err(|msg| SchemaError::new(path, format!("invalid hex string {}: {msg}", quote_string(s))))
}

/// Decodes `s` as hex, accepting upper and lower case.
fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    fn nibble(b: u8) -> Result<u8, String> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            b'A'..=b'F' => Ok(b - b'A' + 10),
            _ => {
                let c = b as char;
                let shown = if c.is_control() { String::new() } else { format!(" {c:?}") };
                Err(format!("invalid byte: U+{:04X}{shown}", b as u32))
            }
        }
    }
    s.as_bytes().chunks(2).map(|pair| Ok(nibble(pair[0])? << 4 | nibble(pair[1])?)).collect()
}

/// Sorts `fields` by offset and concatenates them into a single contiguous byte
/// range starting at 0. A gap, an overlap, or more than one non-empty field at
/// the same offset is rejected before any bytes are returned. Any number of
/// zero-length fields may share an offset with each other and with at most one
/// non-empty field.
fn layout_restore_fields(mut fields: Vec<RestoreField>) -> Result<Vec<u8>, SchemaError> {
    fields.sort_by_key(|f| f.offset);

    // Every field either is the one non-empty field at its offset
    // (contributing its length to `out`) or has a zero-length raw
    // (contributing 0 either way): since a second non-empty field at the same
    // offset is rejected below before `out` is ever returned, this sum is
    // exactly the final output length whenever layout succeeds.
    let total: u64 = fields.iter().map(|f| f.raw.len() as u64).sum();
    let mut out = Vec::with_capacity(total as usize);
    let mut cursor = 0u64;
    let mut i = 0;
    while i < fields.len() {
        let offset = fields[i].offset;
        if offset > cursor {
            return Err(SchemaError::new(
                fields[i].path.clone(),
                format!("gap in physical fields: expected next offset {cursor}, found {offset}"),
            ));
        }
        if offset < cursor {
            return Err(SchemaError::new(
                fields[i].path.clone(),
                format!("overlapping physical fields: expected next offset {cursor}, found {offset}"),
            ));
        }

        let mut non_empty: Option<&RestoreField> = None;
        let mut j = i;
        while j < fields.len() && fields[j].offset == offset {
            if !fields[j].raw.is_empty() {
                if let Some(first) = non_empty {
                    return Err(SchemaError::new(
                        fields[j].path.clone(),
                        format!("duplicate physical field at offset {offset} (also {})", first.path),
                    ));
                }
                non_empty = Some(&fields[j]);
            }
            j += 1;
        }

        if let Some(f) = non_empty {
            out.extend_from_slice(&f.raw);
            cursor += f.raw.len() as u64;
        }
        i = j;
    }

    Ok(out)
}

#[cfg(test)]
mod tests;
