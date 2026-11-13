use std::io::{self, Write};

use serde_json::{json, Value};

use super::super::errors::RestoreError;
use super::super::model::*;
use super::super::parser::parse;
use super::super::test_fixtures::{
    bd_byte, block_size_bytes, descriptor_fixture, end_mark_bytes, flg_byte, literal_only_block_bytes,
    minimal_normal_frame_bytes, skippable_frame_bytes, u32_le, valid_magic_bytes,
};
use super::super::{MAGIC_NUMBER_SKIPPABLE_MAX, MAGIC_NUMBER_SKIPPABLE_MIN};
use super::*;

// Fixture builders

/// An example document of a frame followed by a skippable frame, in the shape
/// of the example in docs/lz4frame-json/README.md.
const EXAMPLE_JSON: &str = r#"{
  "frames": [
    {
      "type": "lz4_frame",
      "magic_number": {
        "offset": 0,
        "value": 407708164,
        "hex": "04224d18"
      },
      "frame_descriptor": {
        "flg": {
          "offset": 4,
          "value": 124,
          "hex": "7c",
          "version": 1,
          "block_independence": true,
          "block_checksum": true,
          "content_size_present": true,
          "content_checksum": true,
          "reserved_bit": 0,
          "dictionary_id_present": false
        },
        "bd": {
          "offset": 5,
          "value": 64,
          "hex": "40",
          "reserved_bit_7": 0,
          "block_max_size_code": 4,
          "block_max_size_bytes": 65536,
          "reserved_bits_3_0": 0
        },
        "content_size": {
          "offset": 6,
          "value": 3,
          "hex": "0300000000000000"
        },
        "dictionary_id": null,
        "header_checksum": {
          "offset": 14,
          "value": 42,
          "hex": "2a"
        }
      },
      "data_blocks": [
        {
          "block_size": {
            "offset": 15,
            "value": 2147483651,
            "hex": "03000080",
            "data_size": 3,
            "uncompressed": true
          },
          "data": [
            {
              "type": "uncompressed_data",
              "offset": 19,
              "size": 3,
              "hex": "010203",
              "display": "\\x01\\x02\\x03"
            }
          ],
          "block_checksum": {
            "offset": 22,
            "value": 305419896,
            "hex": "78563412"
          }
        }
      ],
      "end_mark": {
        "offset": 26,
        "value": 0,
        "hex": "00000000"
      },
      "content_checksum": {
        "offset": 30,
        "value": 2596069104,
        "hex": "f0debc9a"
      }
    },
    {
      "type": "skippable_frame",
      "magic_number": {
        "offset": 34,
        "value": 407710288,
        "hex": "502a4d18"
      },
      "frame_size": {
        "offset": 38,
        "value": 3,
        "hex": "03000000"
      },
      "data": {
        "offset": 42,
        "size": 3,
        "hex": "aabbcc"
      }
    }
  ]
}"#;

/// Returns a document matching the current inspect schema: one normal frame
/// with an empty content_size/dictionary_id/block_checksum/content_checksum,
/// and one skippable frame, so tests can mutate a single field at a time from
/// a known-good baseline.
fn valid_restore_doc() -> Value {
    json!({
        "frames": [
            {
                "type": FRAME_TYPE_LZ4,
                "magic_number": numeric_field_json(0, "04224d18"),
                "frame_descriptor": {
                    "flg": numeric_field_json(4, "7c"),
                    "bd": numeric_field_json(5, "40"),
                    "content_size": null,
                    "dictionary_id": null,
                    "header_checksum": numeric_field_json(6, "2a"),
                },
                "data_blocks": [
                    {
                        "block_size": numeric_field_json(7, "03000080"),
                        "data": [
                            {"type": SEGMENT_TYPE_UNCOMPRESSED_DATA, "offset": 11, "size": 3, "hex": "010203"},
                        ],
                        "block_checksum": null,
                    },
                ],
                "end_mark": numeric_field_json(14, "00000000"),
                "content_checksum": null,
            },
            {
                "type": FRAME_TYPE_SKIPPABLE,
                "magic_number": numeric_field_json(18, "502a4d18"),
                "frame_size": numeric_field_json(22, "00000000"),
                "data": {"offset": 26, "size": 0, "hex": ""},
            },
        ],
    })
}

fn numeric_field_json(offset: u64, hex: &str) -> Value {
    json!({"offset": offset, "value": 0, "hex": hex})
}

/// Parses `text` as a JSON number, keeping its source text (for values such as
/// `-1`, `1.5`, or `18446744073709551616` that `json!` cannot express as a
/// `u64`).
fn num(text: &str) -> Value {
    let v: Value = serde_json::from_str(text).expect("number literal");
    assert!(v.is_number(), "{text} is not a JSON number");
    v
}

fn must_serialize(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("serializing a JSON value cannot fail")
}

/// Removes `key` from the object `v`.
fn remove(v: &mut Value, key: &str) {
    v.as_object_mut().expect("value is not an object").remove(key);
}

/// Returns the first Data Block of the first frame.
fn block0(doc: &mut Value) -> &mut Value {
    &mut doc["frames"][0]["data_blocks"][0]
}

/// Returns the first segment of the first frame's first Data Block.
fn seg0(doc: &mut Value) -> &mut Value {
    &mut doc["frames"][0]["data_blocks"][0]["data"][0]
}

/// Returns the first frame's Frame Descriptor.
fn fd0(doc: &mut Value) -> &mut Value {
    &mut doc["frames"][0]["frame_descriptor"]
}

fn frame_clone(doc: &Value, i: usize) -> Value {
    doc["frames"][i].clone()
}

/// Concatenates hex strings into bytes.
fn must_hex_concat(parts: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        assert!(p.len() % 2 == 0, "odd-length hex {p:?}");
        for i in (0..p.len()).step_by(2) {
            out.push(u8::from_str_radix(&p[i..i + 2], 16).unwrap_or_else(|e| panic!("hex {p:?}: {e}")));
        }
    }
    out
}

/// Restores `data`, which must succeed, and returns the bytes written.
fn restore_ok(data: &[u8], label: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Err(err) = restore(&mut buf, data) {
        panic!("{label}: restore() error = {err}, want Ok");
    }
    buf
}

/// Restores `data`, which must fail with a schema error before writing any
/// bytes, and returns that error.
fn assert_restore_rejected(data: &[u8], label: &str) -> SchemaError {
    let mut buf = Vec::new();
    let result = restore(&mut buf, data);
    assert!(buf.is_empty(), "{label}: restore() wrote {} bytes before failing, want 0", buf.len());
    match result {
        Err(RestoreError::Schema(err)) => err,
        other => panic!("{label}: restore() = {other:?}, want a schema error"),
    }
}

fn concat_bytes(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// A writer that always reports writing fewer bytes than it was given, with no
/// error, to confirm `restore` does not report success for a truncated write
/// it did not itself perform.
struct ShortWriter;

impl Write for ShortWriter {
    fn write(&mut self, p: &[u8]) -> io::Result<usize> {
        Ok(p.len().saturating_sub(1))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// Successful round trips

/// Reconstructs exactly the bytes `EXAMPLE_JSON` describes,
/// field for field.
#[test]
fn restore_example() {
    let want = must_hex_concat(&[
        "04224d18",         // magic_number
        "7c",               // flg
        "40",               // bd
        "0300000000000000", // content_size
        "2a",               // header_checksum
        "03000080",         // block_size
        "010203",           // data
        "78563412",         // block_checksum
        "00000000",         // end_mark
        "f0debc9a",         // content_checksum
        "502a4d18",         // skippable magic_number
        "03000000",         // frame_size
        "aabbcc",           // skippable data
    ]);

    let got = restore_ok(EXAMPLE_JSON.as_bytes(), "example document");
    assert_eq!(got, want, "restore() output");
}

/// Reconstructs the bytes `valid_restore_doc` describes, including the
/// zero-length skippable frame data that shares its offset with nothing
/// following it.
#[test]
fn restore_valid_doc_round_trip() {
    let want = must_hex_concat(&[
        "04224d18", // magic_number
        "7c",       // flg
        "40",       // bd
        "2a",       // header_checksum (content_size/dictionary_id absent)
        "03000080", // block_size
        "010203",   // data
        "00000000", // end_mark (block_checksum absent)
        "502a4d18", // skippable magic_number
        "00000000", // frame_size
                    // skippable data is zero-length
    ]);

    let got = restore_ok(&must_serialize(&valid_restore_doc()), "valid doc");
    assert_eq!(got, want, "restore() output");
}

/// Builds a stream mixing a fully-populated normal frame (Content Size,
/// Dictionary ID, two Data Blocks including a size-0 uncompressed block
/// sharing its offset with a following Block Checksum, and a Content
/// Checksum), a minimal normal frame, and two skippable frames (one with
/// size-0 data whose offset coincides with the following skippable frame's
/// Magic Number), parses it, renders the result to JSON exactly as inspect
/// would, and confirms `restore` reconstructs the original bytes exactly.
#[test]
fn restore_round_trip_from_parsed_model() {
    let flg = flg_byte(1, true, true, true, true, 0, true);
    let bd = bd_byte(0, 4, 0);
    let descriptor = descriptor_fixture(flg, bd, Some(5), Some(0xdead_beef), 0x2a);

    let block0_data = literal_only_block_bytes(&[0x01, 0x02, 0x03]);
    let full = concat_bytes(&[
        &valid_magic_bytes(),
        &descriptor,
        &block_size_bytes(false, block0_data.len() as u32),
        &block0_data,
        &u32_le(0x1234_5678),       // block 0 checksum
        &block_size_bytes(true, 0), // block 1: empty uncompressed block
        &u32_le(0x9abc_def0),       // block 1 checksum, same offset as block 1's 0-length data
        &end_mark_bytes(),
        &u32_le(0xf0de_bc9a), // content checksum
    ]);

    let minimal = minimal_normal_frame_bytes();
    let skip_zero = skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MIN, &[]);
    let skip_data = skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MAX, &[0xaa, 0xbb]);

    let input = concat_bytes(&[&full, &minimal, &skip_zero, &skip_data]);

    let root = parse(&input[..]).unwrap_or_else(|err| panic!("parse: {err}"));
    let inspected = root.to_json();

    let got = restore_ok(inspected.as_bytes(), "parsed model");
    assert_eq!(got, input, "restore() round trip mismatch");
}

/// Two JSON documents that differ only in object member order restore to the
/// same bytes: offset/hex, not text position, decides layout.
#[test]
fn restore_ignores_member_order() {
    let in_order = r#"{"frames":[{"type":"skippable_frame","magic_number":{"offset":0,"value":407710288,"hex":"502a4d18"},"frame_size":{"offset":4,"value":2,"hex":"0200"},"data":{"offset":6,"size":2,"hex":"aabb"}}]}"#;
    let reordered = r#"{"frames":[{"data":{"hex":"aabb","offset":6,"size":2},"frame_size":{"hex":"0200","value":2,"offset":4},"magic_number":{"hex":"502a4d18","value":407710288,"offset":0},"type":"skippable_frame"}]}"#;

    let want = restore_ok(in_order.as_bytes(), "in order");
    let got = restore_ok(reordered.as_bytes(), "reordered");
    assert_eq!(got, want, "restore(reordered)");
}

/// Changing "value", "size", or "type" on physical fields does not change the
/// restored bytes, as long as offset/hex stay the same.
#[test]
fn restore_ignores_value_and_size() {
    let baseline = restore_ok(&must_serialize(&valid_restore_doc()), "base");

    let mut doc = valid_restore_doc();
    seg0(&mut doc)["size"] = json!(999); // wrong on purpose: size must not matter
    seg0(&mut doc)["type"] = json!("definitely_not_opaque"); // type must not matter
    doc["frames"][0]["magic_number"]["value"] = json!(0); // wrong on purpose: value must not matter

    let got = restore_ok(&must_serialize(&doc), "mutated");
    assert_eq!(got, baseline, "restore(mutated) must be unchanged by value/size/type");
}

/// Reversing the offset order of physical fields across sibling array
/// elements, in "frames", a Data Block's "data" segment array, or the
/// "data_blocks" array itself, is rejected as a schema error before any bytes
/// are written, even though every individual field's own offset/hex is
/// unchanged and valid. Array member order must present offsets
/// non-decreasing; only offset (not array position) ever decides which bytes
/// land where, but a document violating this order is refused rather than
/// silently reordered.
#[test]
fn restore_rejects_array_order_violations() {
    // frames array reversed
    let mut doc = valid_restore_doc();
    doc["frames"] = json!([frame_clone(&doc, 1), frame_clone(&doc, 0)]);
    assert_restore_rejected(&must_serialize(&doc), "frames array reversed");

    // data segment array reversed
    let mut doc = valid_restore_doc();
    let seg1 = json!({"type": "token", "offset": 11, "size": 1, "hex": "01"});
    let seg2 = json!({"type": "literal", "offset": 12, "size": 2, "hex": "0203"});
    block0(&mut doc)["data"] = json!([seg2, seg1]);
    assert_restore_rejected(&must_serialize(&doc), "data segment array reversed");

    // data_blocks array reversed: a single, self-contained frame with two Data
    // Blocks whose fields (in absolute offset terms) are perfectly contiguous
    // regardless of which array position each block sits in:
    // layout_restore_fields always re-sorts by offset, so reversing
    // "data_blocks" changes nothing about the final byte layout. Rejection
    // must therefore come from array order alone, not from an incidental gap
    // or overlap.
    let new_doc = |blocks: Value| {
        json!({
            "frames": [{
                "type": FRAME_TYPE_LZ4,
                "magic_number": numeric_field_json(0, "04224d18"),
                "frame_descriptor": {
                    "flg": numeric_field_json(4, "7c"),
                    "bd": numeric_field_json(5, "40"),
                    "content_size": null,
                    "dictionary_id": null,
                    "header_checksum": numeric_field_json(6, "2a"),
                },
                "data_blocks": blocks,
                "end_mark": numeric_field_json(16, "00000000"),
                "content_checksum": null,
            }],
        })
    };
    let block_a = json!({
        "block_size": numeric_field_json(7, "03000080"),
        "data": [{"type": SEGMENT_TYPE_UNCOMPRESSED_DATA, "offset": 11, "size": 1, "hex": "01"}],
        "block_checksum": null,
    });
    let block_b = json!({
        "block_size": numeric_field_json(12, "80000000"),
        "data": [{"type": SEGMENT_TYPE_UNCOMPRESSED_DATA, "offset": 16, "size": 0, "hex": ""}],
        "block_checksum": null,
    });

    // Sanity check: in order, this document restores without error.
    restore_ok(&must_serialize(&new_doc(json!([block_a.clone(), block_b.clone()]))), "in order");

    assert_restore_rejected(&must_serialize(&new_doc(json!([block_b, block_a]))), "data_blocks array reversed");
}

/// The order check compares a preceding element's *maximum* offset against the
/// next element's minimum, not the two elements' minimums against each other.
/// It calls `collect_restore_fields` directly on a synthetic array rather than
/// going through a full document, because a realistic Data Block layout can't
/// isolate this from the unrelated gap/overlap checks `layout_restore_fields`
/// performs once fields are sorted globally by offset: this test needs the
/// array-order check to be the only thing that can reject the input.
///
/// The first element's own fields span [10, 30]; the second's is just 20. The
/// elements' minimums (10, then 20) are ascending, so a comparison that only
/// tracked minimums would wrongly accept; comparing the first element's
/// maximum (30) against the second's minimum (20) correctly rejects.
#[test]
fn restore_array_order_compares_element_range_not_just_minimum() {
    let elem_a = json!({
        "x": {"offset": 10, "hex": ""},
        "y": {"offset": 30, "hex": ""},
    });
    let elem_b = json!({
        "z": {"offset": 20, "hex": ""},
    });

    let result = collect_restore_fields(&json!([elem_a, elem_b]), "$.test");
    assert!(result.is_err(), "want error (element ranges [10,30] then [20,20] interleave)");

    // Sanity check: shrinking the first element's range to [10, 20] so it no
    // longer exceeds the second element's minimum (20, allowed as an equal
    // boundary) must not be rejected, confirming the rejection above really
    // was about the interleaved range.
    let elem_a_narrow = json!({
        "x": {"offset": 10, "hex": ""},
        "y": {"offset": 20, "hex": ""},
    });
    if let Err(err) = collect_restore_fields(&json!([elem_a_narrow, elem_b]), "$.test") {
        panic!("narrowed ranges: error = {err}, want Ok");
    }
}

/// Neighboring array elements whose physical fields share the same offset
/// (e.g. a size-0 field and the field that follows it at the same byte
/// position) are not treated as an order violation: the rule is
/// non-decreasing, not strictly increasing.
#[test]
fn restore_array_order_allows_equal_offsets() {
    let mut doc = valid_restore_doc();
    let seg1 = json!({"type": "a", "offset": 11, "size": 0, "hex": ""});
    let seg2 = json!({"type": "b", "offset": 11, "size": 3, "hex": "010203"});
    block0(&mut doc)["data"] = json!([seg1, seg2]);

    restore_ok(&must_serialize(&doc), "equal offsets");
}

/// Returns a skippable frame whose magic_number, frame_size, and data are all
/// schema-valid but offset/hex-less empty objects: the realistic fieldless
/// array element under the current schema.
fn fieldless_frame() -> Value {
    json!({
        "type": FRAME_TYPE_SKIPPABLE,
        "magic_number": {},
        "frame_size": {},
        "data": {},
    })
}

/// An array element carrying no physical field anywhere beneath it takes no
/// part in the surrounding order check, regardless of where in the array it
/// sits.
#[test]
fn restore_array_order_ignores_fieldless_elements() {
    let want = restore_ok(&must_serialize(&valid_restore_doc()), "base");

    for pos in ["before", "between", "after"] {
        let mut doc = valid_restore_doc();
        let (f0, f1) = (frame_clone(&doc, 0), frame_clone(&doc, 1));
        let fieldless = fieldless_frame();
        doc["frames"] = match pos {
            "before" => json!([fieldless, f0, f1]),
            "between" => json!([f0, fieldless, f1]),
            _ => json!([f0, f1, fieldless]),
        };

        let got = restore_ok(&must_serialize(&doc), pos);
        assert_eq!(got, want, "{pos}: fieldless element must not affect bytes or order");
    }
}

/// A fieldless element sitting between two field-bearing elements does not
/// reset the order check: a later, higher-offset element followed by an
/// earlier, lower-offset one must still be rejected even with a fieldless
/// element in between, because the check must keep comparing against the last
/// field-bearing element seen, not the fieldless one.
#[test]
fn restore_array_order_fieldless_element_does_not_mask_violation() {
    let mut doc = valid_restore_doc();
    // frames[1] (skippable, offsets 18-25) before frames[0] (normal, offsets
    // 0-17) is already out of order on its own; the fieldless element sits
    // between them and must not hide that.
    doc["frames"] = json!([frame_clone(&doc, 1), fieldless_frame(), frame_clone(&doc, 0)]);

    assert_restore_rejected(&must_serialize(&doc), "fieldless between out-of-order frames");
}

/// An object carrying "offset" without "hex" is excluded from concatenation
/// rather than being an error by itself: an explanatory object, still walked
/// for nested physical fields but not itself one. The
/// "offset-only object excluded, leaves a gap" case of `restore_rejects` only
/// proves *some* error results when magic_number is replaced by an
/// offset-only object entirely: a wrong implementation that rejects any
/// offset-without-hex object outright would pass that case too. This test
/// instead adds a harmless offset-only object whose exclusion costs nothing,
/// and confirms restoration still succeeds, byte-identical to a baseline
/// without it.
#[test]
fn restore_offset_only_object_is_excluded_not_rejected() {
    let want = restore_ok(&must_serialize(&valid_restore_doc()), "base");

    let mut doc = valid_restore_doc();
    fd0(&mut doc)["reserved_note"] = json!({"offset": 4});

    let got = restore_ok(&must_serialize(&doc), "with offset-only object");
    assert_eq!(got, want, "restore(with offset-only object)");
}

/// More than one zero-length physical field may share an offset with each
/// other and with a following non-empty field: `layout_restore_fields` only
/// limits a shared offset to "at most one non-empty field", not to a single
/// zero-length field.
#[test]
fn restore_multiple_zero_length_fields_at_same_offset() {
    let mut doc = valid_restore_doc();
    let zero1 = json!({"type": "a", "offset": 11, "size": 0, "hex": ""});
    let zero2 = json!({"type": "b", "offset": 11, "size": 0, "hex": ""});
    let zero3 = json!({"type": "c", "offset": 11, "size": 0, "hex": ""});
    let non_empty = json!({"type": "d", "offset": 11, "size": 3, "hex": "010203"});
    block0(&mut doc)["data"] = json!([zero1, zero2, zero3, non_empty]);

    let want = must_hex_concat(&[
        "04224d18", // magic_number
        "7c",       // flg
        "40",       // bd
        "2a",       // header_checksum
        "03000080", // block_size
        "010203",   // the one non-empty segment; the three zero-length ones contribute nothing
        "00000000", // end_mark
        "502a4d18", // skippable magic_number
        "00000000", // frame_size
    ]);

    let got = restore_ok(&must_serialize(&doc), "zero-length fields");
    assert_eq!(got, want, "restore() output");
}

/// `collect_restore_fields` keeps recursing through however many wrapper
/// objects sit between an array element and its physical field, not just the
/// one level of nesting today's inspect output happens to produce.
///
/// This uses "block_checksum", which, like content_size/dictionary_id/
/// content_checksum, the restore schema only requires to be *an object*, with
/// nothing said about what's inside it, as a stand-in schema position where
/// recursion depth genuinely isn't fixed, to confirm the recursion has no
/// hardcoded depth limit.
#[test]
fn restore_collects_physical_fields_nested_arbitrarily_deep() {
    let flat_checksum = numeric_field_json(14, "78563412");
    let deep_checksum = json!({
        "future_container": {"nested": {"deeper": flat_checksum.clone()}},
    });

    let build = |checksum: Value| {
        let mut doc = valid_restore_doc();
        block0(&mut doc)["block_checksum"] = checksum;
        // block_checksum now occupies offset 14-17 (4 bytes), so end_mark and
        // the whole skippable frame shift 4 bytes later than
        // valid_restore_doc's default (block_checksum: null) layout.
        doc["frames"][0]["end_mark"] = numeric_field_json(18, "00000000");
        doc["frames"][1]["magic_number"] = numeric_field_json(22, "502a4d18");
        doc["frames"][1]["frame_size"] = numeric_field_json(26, "00000000");
        doc["frames"][1]["data"] = json!({"offset": 30, "size": 0, "hex": ""});
        must_serialize(&doc)
    };

    let want = restore_ok(&build(flat_checksum.clone()), "flat");
    let got = restore_ok(&build(deep_checksum), "deeply nested");
    assert_eq!(got, want, "restore(deeply nested)");
}

/// Splitting a Data Block's data into several adjacent segments that cover the
/// same byte range restores identical bytes.
#[test]
fn restore_adjacent_segments_equal_single_segment() {
    let want = restore_ok(&must_serialize(&valid_restore_doc()), "single");

    let mut doc = valid_restore_doc();
    block0(&mut doc)["data"] = json!([
        {"type": "token", "offset": 11, "size": 1, "hex": "01"},
        {"type": "literal", "offset": 12, "size": 2, "hex": "0203"},
    ]);

    let got = restore_ok(&must_serialize(&doc), "split");
    assert_eq!(got, want, "restore(split)");
}

/// A "data" segment carrying the removed "type": "opaque" restores to the
/// same bytes as the current schema's segment: restore never inspects "type"
/// while collecting physical fields, so the legacy value is neither rejected
/// nor treated specially.
#[test]
fn restore_accepts_legacy_opaque_segment_type() {
    let want = restore_ok(&must_serialize(&valid_restore_doc()), "base");

    let mut doc = valid_restore_doc();
    seg0(&mut doc)["type"] = json!("opaque");

    let got = restore_ok(&must_serialize(&doc), "legacy opaque");
    assert_eq!(got, want, "restore(legacy opaque)");
}

/// An offset/hex-bearing object placed outside the root "frames" array is
/// never walked and never contributes bytes.
#[test]
fn restore_ignores_metadata_outside_frames() {
    let want = restore_ok(&must_serialize(&valid_restore_doc()), "base");

    let mut doc = valid_restore_doc();
    doc["generated_by"] = json!({"offset": 999, "hex": "ff"});

    let got = restore_ok(&must_serialize(&doc), "with metadata");
    assert_eq!(got, want, "restore(with metadata)");
}

// Rejections

#[test]
fn restore_rejects() {
    struct Case {
        name: &'static str,
        mutate: fn(&mut Value),
    }
    let cases = [
        Case { name: "negative offset", mutate: |doc| doc["frames"][0]["magic_number"]["offset"] = num("-1") },
        Case { name: "fractional offset", mutate: |doc| doc["frames"][0]["magic_number"]["offset"] = num("1.5") },
        Case { name: "exponent offset", mutate: |doc| doc["frames"][0]["magic_number"]["offset"] = num("1e2") },
        Case {
            name: "offset overflows u64",
            mutate: |doc| doc["frames"][0]["magic_number"]["offset"] = num("18446744073709551616"),
        },
        Case { name: "offset not a number", mutate: |doc| doc["frames"][0]["magic_number"]["offset"] = json!("0") },
        Case { name: "hex odd length", mutate: |doc| doc["frames"][0]["magic_number"]["hex"] = json!("04224d1") },
        Case {
            // Same length as the valid "04224d18", one character changed to
            // an invalid one: a hypothetical bug that stripped or otherwise
            // special-cased a leading "0x" wouldn't touch this string at all,
            // so a passing test here is specifically about invalid
            // characters, not incidentally passing because the mutation also
            // happened to change the field's length (and so create an
            // unrelated gap/overlap once decoded).
            name: "hex invalid character",
            mutate: |doc| doc["frames"][0]["magic_number"]["hex"] = json!("04224d1g"),
        },
        Case {
            // "0x" plus the field's own already-correct hex digits. This
            // isolates the "0x" prefix itself (the documented format requires
            // unprefixed hex): if a bug stripped a recognized "0x" prefix
            // before decoding, the remainder here decodes to exactly the
            // right bytes, so restore would wrongly succeed instead of
            // failing on some incidental downstream gap.
            name: "hex 0x prefix",
            mutate: |doc| doc["frames"][0]["magic_number"]["hex"] = json!("0x04224d18"),
        },
        Case { name: "hex not a string", mutate: |doc| doc["frames"][0]["magic_number"]["hex"] = json!(1234) },
        Case {
            name: "hex present without offset",
            mutate: |doc| remove(&mut doc["frames"][0]["magic_number"], "offset"),
        },
        Case {
            name: "offset-only object excluded, leaves a gap",
            mutate: |doc| doc["frames"][0]["magic_number"] = json!({"offset": 0}),
        },
        Case {
            // Same rule as "offset-only object excluded, leaves a gap", but
            // anchored on a Data Block "data" element specifically: the
            // schema layer has no per-segment "hex" requirement, so this
            // segment's rejection comes entirely from the generic
            // physical-field walk/layout check.
            name: "data block segment missing hex leaves a gap",
            mutate: |doc| remove(seg0(doc), "hex"),
        },
        Case { name: "first offset is not zero", mutate: |doc| doc["frames"][0]["magic_number"]["offset"] = json!(1) },
        Case { name: "gap between fields", mutate: |doc| fd0(doc)["flg"]["offset"] = json!(100) },
        Case { name: "overlap between fields", mutate: |doc| fd0(doc)["flg"]["offset"] = json!(3) },
        Case {
            // The duplicate is the same length (3 bytes) as the original
            // segment it shares an offset with, not a different length: if
            // they differed, an implementation that dropped the duplicate
            // check and just kept whichever field the sort happened to order
            // last could still advance the cursor to a value that doesn't
            // match the next field's offset, so the test would pass for the
            // unrelated reason of a resulting gap instead of actually
            // exercising duplicate rejection. With matching lengths,
            // dropping the check would make the byte range contiguous either
            // way, so this can only pass "want error" if duplicate detection
            // itself rejects it.
            name: "duplicate non-empty fields at same offset",
            mutate: |doc| {
                let seg = seg0(doc).clone();
                let dup = json!({"type": "dup", "offset": seg["offset"].clone(), "size": 3, "hex": "aabbcc"});
                block0(doc)["data"] = json!([seg, dup]);
            },
        },
    ];

    for case in cases {
        let mut doc = valid_restore_doc();
        (case.mutate)(&mut doc);
        assert_restore_rejected(&must_serialize(&doc), case.name);
    }
}

/// Drives every schema validation failure in `restore_schema_reject_cases`
/// through `restore` itself rather than `decode_restore_document` directly,
/// and confirms none of them write anything to the writer. The schema tests
/// already check the error and JSON path at the schema layer; this test checks
/// that a schema failure never reaches the output writer.
#[test]
fn restore_schema_rejections_leave_writer_untouched() {
    for case in restore_schema_reject_cases() {
        let mut doc = valid_restore_doc();
        (case.mutate)(&mut doc);
        let err = assert_restore_rejected(&must_serialize(&doc), case.name);
        assert_eq!(err.path, case.want_path, "{}: path (err: {err})", case.name);
    }
}

/// A document that passes schema validation (every required container present
/// as an object) but whose containers carry no "offset"/"hex" pair anywhere is
/// rejected rather than silently producing an empty, 0-byte output: the
/// offset-0 start requirement can't hold vacuously.
#[test]
fn restore_rejects_no_physical_fields() {
    const DOC: &str = r#"{"frames":[{"type":"skippable_frame","magic_number":{},"frame_size":{},"data":{}}]}"#;

    assert_restore_rejected(DOC.as_bytes(), "no physical fields");
}

/// A writer that accepts fewer bytes than it was given is reported as a failed
/// write rather than success.
#[test]
fn restore_reports_short_write() {
    let data = must_serialize(&valid_restore_doc());
    match restore(ShortWriter, &data[..]) {
        Err(RestoreError::Io(err)) => assert_eq!(err.kind(), io::ErrorKind::WriteZero, "error kind (err: {err})"),
        other => panic!("restore() = {other:?}, want an I/O write error"),
    }
}

/// The offset+length overflow check, already pinned at the `new_restore_field`
/// unit level by `new_restore_field_offset_overflow_boundary`, is also
/// reachable, and leaves the writer untouched, when driven through a full
/// document via `restore`. This is distinct from "offset overflows u64" in
/// `restore_rejects`, which is the offset value itself not fitting in a u64,
/// caught by `parse_restore_offset` well before this check runs.
#[test]
fn restore_offset_length_overflow_rejected_through_document() {
    let mut doc = valid_restore_doc();
    // offset == u64::MAX fits in a u64 by itself; magic_number's hex is left
    // at its original non-empty 4 bytes, so offset+length overflows u64.
    doc["frames"][0]["magic_number"]["offset"] = num("18446744073709551615");

    let err = assert_restore_rejected(&must_serialize(&doc), "offset+length overflow");
    // new_restore_field reports the overflow at the field's own path (not
    // ".offset" or ".hex", which parse_restore_offset/parse_restore_hex use for
    // their own format-validation failures), so pinning this path confirms the
    // overflow branch specifically fired rather than some other check.
    assert_eq!(err.path, "$.frames[0].magic_number", "path (err: {err})");
}

/// Exercises the offset+length overflow check at the exact edges of u64,
/// where an off-by-one error in the "offset > u64::MAX - length" comparison
/// would either falsely reject a valid boundary field or fail to reject the
/// smallest possible overflow.
#[test]
fn new_restore_field_offset_overflow_boundary() {
    // offset == u64::MAX, zero-length hex: must not be flagged as overflow.
    match new_restore_field(&num("18446744073709551615"), &json!(""), "$.x") {
        Ok(f) => assert_eq!(f.offset, u64::MAX, "offset"),
        Err(err) => panic!("offset=MAX, len=0: unexpected error: {err}"),
    }

    // offset == u64::MAX-1, 1-byte hex: offset+len == u64::MAX exactly, still
    // must not overflow.
    if let Err(err) = new_restore_field(&num("18446744073709551614"), &json!("ff"), "$.x") {
        panic!("offset+len == MAX: unexpected error: {err}");
    }

    // offset == u64::MAX, 1-byte hex: offset+len overflows by exactly 1.
    let result = new_restore_field(&num("18446744073709551615"), &json!("ff"), "$.x");
    assert!(result.is_err(), "offset+len overflows by 1: want error, got Ok");
}

// Reject cases shared with the schema tests (duplicated from
// restore_schema/tests.rs)

/// One document mutation that must fail schema validation, with the JSON path
/// the resulting error must report.
struct RejectCase {
    name: &'static str,
    mutate: fn(&mut Value),
    want_path: &'static str,
}

fn restore_schema_reject_cases() -> Vec<RejectCase> {
    fn case(name: &'static str, mutate: fn(&mut Value), want_path: &'static str) -> RejectCase {
        RejectCase { name, mutate, want_path }
    }
    vec![
        case("missing frames", |doc| remove(doc, "frames"), "$.frames"),
        case("null frames", |doc| doc["frames"] = Value::Null, "$.frames"),
        case("empty frames", |doc| doc["frames"] = json!([]), "$.frames"),
        case("frames wrong type", |doc| doc["frames"] = json!("oops"), "$.frames"),
        case("unknown frame type", |doc| doc["frames"][0]["type"] = json!("future_frame"), "$.frames[0].type"),
        case("missing frame type", |doc| remove(&mut doc["frames"][0], "type"), "$.frames[0].type"),
        case("empty frame type", |doc| doc["frames"][0]["type"] = json!(""), "$.frames[0].type"),
        case("frame not an object", |doc| doc["frames"][0] = json!("oops"), "$.frames[0]"),
        case("missing magic_number", |doc| remove(&mut doc["frames"][0], "magic_number"), "$.frames[0].magic_number"),
        case(
            "magic_number wrong type",
            |doc| doc["frames"][0]["magic_number"] = json!("oops"),
            "$.frames[0].magic_number",
        ),
        case(
            "null frame_descriptor",
            |doc| doc["frames"][0]["frame_descriptor"] = Value::Null,
            "$.frames[0].frame_descriptor",
        ),
        case(
            "frame_descriptor wrong type",
            |doc| doc["frames"][0]["frame_descriptor"] = json!([]),
            "$.frames[0].frame_descriptor",
        ),
        case("missing flg", |doc| remove(fd0(doc), "flg"), "$.frames[0].frame_descriptor.flg"),
        case("null bd", |doc| fd0(doc)["bd"] = Value::Null, "$.frames[0].frame_descriptor.bd"),
        case(
            "missing header_checksum",
            |doc| remove(fd0(doc), "header_checksum"),
            "$.frames[0].frame_descriptor.header_checksum",
        ),
        case(
            "content_size wrong type when present",
            |doc| fd0(doc)["content_size"] = json!("oops"),
            "$.frames[0].frame_descriptor.content_size",
        ),
        case("missing data_blocks", |doc| remove(&mut doc["frames"][0], "data_blocks"), "$.frames[0].data_blocks"),
        case("null data_blocks", |doc| doc["frames"][0]["data_blocks"] = Value::Null, "$.frames[0].data_blocks"),
        case(
            "data_blocks element not an object",
            |doc| doc["frames"][0]["data_blocks"] = json!(["oops"]),
            "$.frames[0].data_blocks[0]",
        ),
        case("missing block_size", |doc| remove(block0(doc), "block_size"), "$.frames[0].data_blocks[0].block_size"),
        case("null block_size", |doc| block0(doc)["block_size"] = Value::Null, "$.frames[0].data_blocks[0].block_size"),
        case("data wrong type", |doc| block0(doc)["data"] = json!("oops"), "$.frames[0].data_blocks[0].data"),
        case(
            "block_checksum wrong type when present",
            |doc| block0(doc)["block_checksum"] = json!("oops"),
            "$.frames[0].data_blocks[0].block_checksum",
        ),
        case("empty data", |doc| block0(doc)["data"] = json!([]), "$.frames[0].data_blocks[0].data"),
        case("missing data", |doc| remove(block0(doc), "data"), "$.frames[0].data_blocks[0].data"),
        case("null data", |doc| block0(doc)["data"] = Value::Null, "$.frames[0].data_blocks[0].data"),
        case("missing end_mark", |doc| remove(&mut doc["frames"][0], "end_mark"), "$.frames[0].end_mark"),
        case("null end_mark", |doc| doc["frames"][0]["end_mark"] = Value::Null, "$.frames[0].end_mark"),
        case(
            "content_checksum wrong type when present",
            |doc| doc["frames"][0]["content_checksum"] = json!("oops"),
            "$.frames[0].content_checksum",
        ),
        case(
            "missing skippable magic_number",
            |doc| remove(&mut doc["frames"][1], "magic_number"),
            "$.frames[1].magic_number",
        ),
        case(
            "null skippable magic_number",
            |doc| doc["frames"][1]["magic_number"] = Value::Null,
            "$.frames[1].magic_number",
        ),
        case(
            "missing skippable frame_size",
            |doc| remove(&mut doc["frames"][1], "frame_size"),
            "$.frames[1].frame_size",
        ),
        case(
            "skippable frame_size wrong type",
            |doc| doc["frames"][1]["frame_size"] = json!([]),
            "$.frames[1].frame_size",
        ),
        case("null skippable data", |doc| doc["frames"][1]["data"] = Value::Null, "$.frames[1].data"),
        case("skippable data wrong type", |doc| doc["frames"][1]["data"] = json!([]), "$.frames[1].data"),
    ]
}
