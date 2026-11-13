use std::io::Read;

use serde_json::{Map, Value};

use super::errors::{quote_string, SchemaError};
use super::model::{FRAME_TYPE_LZ4, FRAME_TYPE_SKIPPABLE};

type Object = Map<String, Value>;

/// Decodes `r` as a restore JSON document.
///
/// The root JSON value is decoded exactly once, with numbers kept as their
/// original text, and any non-whitespace value following it is rejected. The
/// decoded document is then validated against the current restore schema: a
/// JSON object with a non-empty "frames" array of normal and skippable
/// frames, each carrying the required objects and arrays the current schema
/// defines. Field values are not decoded or otherwise validated here, and
/// neither is offset continuity across physical fields: `restore` does that
/// once schema validation has confirmed the required containers are present.
/// Data Block "data" elements are only required to be a non-empty array; their
/// shape (whether a segment carries "type", which child fields it has) is not
/// validated here, since a compressed block's data holds sequence wrapper
/// objects that are not themselves physical fields alongside uncompressed
/// blocks' single physical-field element that is one. Any object anywhere in
/// the document that carries both "offset" and "hex" is later recognized and
/// restored as a physical field by the recursive walk, regardless of "type" or
/// its absence.
///
/// The returned map is the root JSON object, decoded generically rather than
/// into this module's inspect model types, so that future Block Format
/// segments and unknown segment types round-trip without a schema change to
/// this function.
pub fn decode_restore_document(r: impl Read) -> Result<Object, SchemaError> {
    let mut values = serde_json::Deserializer::from_reader(r).into_iter::<Value>();

    let doc = match values.next() {
        None => return Err(SchemaError::new("$", "empty input")),
        Some(Err(err)) => return Err(SchemaError::new("$", format!("invalid JSON: {err}"))),
        Some(Ok(doc)) => doc,
    };

    match values.next() {
        None => {}
        Some(Ok(_)) => return Err(SchemaError::new("$", "unexpected value after document root")),
        Some(Err(err)) => return Err(SchemaError::new("$", format!("unexpected content after document root: {err}"))),
    }

    validate_root(doc)
}

fn validate_root(doc: Value) -> Result<Object, SchemaError> {
    let Value::Object(root) = doc else {
        return Err(SchemaError::new("$", format!("must be an object, got {}", json_type_name(&doc))));
    };

    let frames = require_array(&root, "frames", "$")?;
    if frames.is_empty() {
        return Err(SchemaError::new("$.frames", "must not be empty"));
    }

    for (i, f) in frames.iter().enumerate() {
        validate_frame(f, &format!("$.frames[{i}]"))?;
    }

    Ok(root)
}

fn validate_frame(v: &Value, path: &str) -> Result<(), SchemaError> {
    let Value::Object(obj) = v else {
        return Err(SchemaError::new(path, format!("must be an object, got {}", json_type_name(v))));
    };

    let typ = require_non_empty_string(obj, "type", path)?;

    match typ {
        FRAME_TYPE_LZ4 => validate_normal_frame(obj, path),
        FRAME_TYPE_SKIPPABLE => validate_skippable_frame(obj, path),
        _ => Err(SchemaError::new(field_path(path, "type"), format!("unknown frame type {}", quote_string(typ)))),
    }
}

fn validate_normal_frame(obj: &Object, path: &str) -> Result<(), SchemaError> {
    require_object(obj, "magic_number", path)?;

    let fd = require_object(obj, "frame_descriptor", path)?;
    let fd_path = field_path(path, "frame_descriptor");
    require_object(fd, "flg", &fd_path)?;
    require_object(fd, "bd", &fd_path)?;
    allow_optional_object(fd, "content_size", &fd_path)?;
    allow_optional_object(fd, "dictionary_id", &fd_path)?;
    require_object(fd, "header_checksum", &fd_path)?;

    let blocks = require_array(obj, "data_blocks", path)?;
    for (i, b) in blocks.iter().enumerate() {
        let block_path = format!("{}[{i}]", field_path(path, "data_blocks"));
        validate_data_block(b, &block_path)?;
    }

    require_object(obj, "end_mark", path)?;
    allow_optional_object(obj, "content_checksum", path)
}

fn validate_data_block(v: &Value, path: &str) -> Result<(), SchemaError> {
    let Value::Object(obj) = v else {
        return Err(SchemaError::new(path, format!("must be an object, got {}", json_type_name(v))));
    };

    require_object(obj, "block_size", path)?;

    let data = require_array(obj, "data", path)?;
    if data.is_empty() {
        return Err(SchemaError::new(field_path(path, "data"), "must not be empty"));
    }

    allow_optional_object(obj, "block_checksum", path)
}

fn validate_skippable_frame(obj: &Object, path: &str) -> Result<(), SchemaError> {
    require_object(obj, "magic_number", path)?;
    require_object(obj, "frame_size", path)?;
    require_object(obj, "data", path)?;
    Ok(())
}

// --- generic object/array/field helpers --------------------------------

pub(super) fn field_path(parent: &str, key: &str) -> String {
    format!("{parent}.{key}")
}

/// Looks up `key` in `obj`, rejecting a missing or null value. The caller's
/// path must already identify `obj` itself; the returned path identifies the
/// field for further validation and error reporting.
fn require_field<'a>(obj: &'a Object, key: &str, parent: &str) -> Result<(&'a Value, String), SchemaError> {
    let p = field_path(parent, key);
    match obj.get(key) {
        None => Err(SchemaError::new(p, "missing required field")),
        Some(Value::Null) => Err(SchemaError::new(p, "must not be null")),
        Some(val) => Ok((val, p)),
    }
}

fn require_object<'a>(obj: &'a Object, key: &str, parent: &str) -> Result<&'a Object, SchemaError> {
    let (val, p) = require_field(obj, key, parent)?;
    match val {
        Value::Object(m) => Ok(m),
        _ => Err(SchemaError::new(p, format!("must be an object, got {}", json_type_name(val)))),
    }
}

fn require_array<'a>(obj: &'a Object, key: &str, parent: &str) -> Result<&'a Vec<Value>, SchemaError> {
    let (val, p) = require_field(obj, key, parent)?;
    match val {
        Value::Array(a) => Ok(a),
        _ => Err(SchemaError::new(p, format!("must be an array, got {}", json_type_name(val)))),
    }
}

fn require_non_empty_string<'a>(obj: &'a Object, key: &str, parent: &str) -> Result<&'a str, SchemaError> {
    let (val, p) = require_field(obj, key, parent)?;
    let Value::String(s) = val else {
        return Err(SchemaError::new(p, format!("must be a string, got {}", json_type_name(val))));
    };
    if s.is_empty() {
        return Err(SchemaError::new(p, "must not be empty"));
    }
    Ok(s)
}

/// Accepts a missing or null value; if `key` is present and non-null it must
/// be an object.
fn allow_optional_object(obj: &Object, key: &str, parent: &str) -> Result<(), SchemaError> {
    match obj.get(key) {
        None | Some(Value::Null) | Some(Value::Object(_)) => Ok(()),
        Some(val) => Err(SchemaError::new(
            field_path(parent, key),
            format!("must be an object or null, got {}", json_type_name(val)),
        )),
    }
}

pub(super) fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
    }
}

#[cfg(test)]
mod tests;
