use serde_json::{json, Value};

use super::super::errors::RestoreError;
use super::super::model::*;
use super::super::restore::restore;
use super::super::{MAGIC_NUMBER_LZ4, MAGIC_NUMBER_SKIPPABLE_MIN};
use super::*;

// Fixture builders

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

/// Decodes `body` and returns the schema error it must produce.
fn decode_schema_err(body: &[u8]) -> SchemaError {
    match decode_restore_document(body) {
        Ok(_) => panic!("decode_restore_document succeeded, want a SchemaError"),
        Err(err) => err,
    }
}

/// Checks that `restore` rejects `body` with a schema error and writes nothing.
fn assert_restore_rejects_untouched(body: &[u8], label: &str) {
    let mut buf = Vec::new();
    match restore(&mut buf, body) {
        Err(RestoreError::Schema(_)) => {}
        other => panic!("{label}: restore() = {other:?}, want a schema error"),
    }
    assert!(buf.is_empty(), "{label}: restore() wrote {} bytes before failing, want 0", buf.len());
}

// Accept

#[test]
fn restore_schema_accepts_actual_model_output() {
    let content_size = NumericField::new(6, 3u64, &[0x03, 0, 0, 0, 0, 0, 0, 0]);
    let dictionary_id = NumericField::new(14, 1u32, &[0x01, 0, 0, 0]);
    let block_checksum = NumericField::new(22, 0x1234_5678u32, &[0x78, 0x56, 0x34, 0x12]);
    let content_checksum = NumericField::new(30, 0x9abc_def0u32, &[0xf0, 0xde, 0xbc, 0x9a]);

    let root = Root {
        frames: vec![
            Frame::Normal(Box::new(NormalFrame {
                magic_number: NumericField::new(0, MAGIC_NUMBER_LZ4, &[0x04, 0x22, 0x4d, 0x18]),
                frame_descriptor: FrameDescriptor {
                    flg: Flg {
                        field: NumericField::new(4, 0x7c, &[0x7c]),
                        version: 0,
                        block_independence: false,
                        block_checksum: false,
                        content_size_present: false,
                        content_checksum: false,
                        reserved_bit: 0,
                        dictionary_id_present: false,
                    },
                    bd: Bd {
                        field: NumericField::new(5, 0x40, &[0x40]),
                        reserved_bit_7: 0,
                        block_max_size_code: 0,
                        block_max_size_bytes: 0,
                        reserved_bits_3_0: 0,
                    },
                    content_size: Some(content_size),
                    dictionary_id: Some(dictionary_id),
                    header_checksum: NumericField::new(14, 0x2a, &[0x2a]),
                },
                data_blocks: vec![DataBlock {
                    block_size: BlockSize {
                        field: NumericField::new(15, 0x8000_0003, &[0x03, 0x00, 0x00, 0x80]),
                        data_size: 0,
                        uncompressed: false,
                    },
                    data: vec![BlockData::Uncompressed(UncompressedDataSegment::new(19, &[0x01, 0x02, 0x03]))],
                    block_checksum: Some(block_checksum),
                }],
                end_mark: NumericField::new(26, 0, &[0, 0, 0, 0]),
                content_checksum: Some(content_checksum),
            })),
            Frame::Skippable(SkippableFrame {
                magic_number: NumericField::new(34, MAGIC_NUMBER_SKIPPABLE_MIN, &[0x50, 0x2a, 0x4d, 0x18]),
                frame_size: NumericField::new(38, 3, &[0x03, 0, 0, 0]),
                data: SkippableData { offset: 42, size: 3, hex: "aabbcc".to_string() },
            }),
        ],
    };

    let data = root.to_json();
    if let Err(err) = decode_restore_document(data.as_bytes()) {
        panic!("decode_restore_document(actual model output) error = {err}, want Ok");
    }
}

#[test]
fn restore_schema_accepts() {
    struct Case {
        name: &'static str,
        mutate: fn(&mut Value),
    }
    let cases = [
        Case { name: "standard inspect schema", mutate: |_| {} },
        Case { name: "empty data_blocks", mutate: |doc| doc["frames"][0]["data_blocks"] = json!([]) },
        Case {
            name: "multiple unknown segment types",
            mutate: |doc| {
                block0(doc)["data"] = json!([
                    {"type": "token", "offset": 11, "size": 1, "hex": "01"},
                    {"type": "literal", "offset": 12, "size": 2, "hex": "0203"},
                ]);
            },
        },
        Case {
            // Deliberate legacy-schema probe: the "opaque" segment type is
            // removed from the current inspect schema, but restore never
            // rejected on "type" content and must keep accepting old
            // documents that still use it.
            name: "legacy opaque segment type",
            mutate: |doc| seg0(doc)["type"] = json!("opaque"),
        },
        Case {
            // Data Block "data" elements' shape is not validated at the
            // schema layer at all: a compressed block's sequence wrapper has
            // no "type" of its own, so requiring one here would reject
            // inspect's own current output.
            name: "segment missing type",
            mutate: |doc| remove(seg0(doc), "type"),
        },
        Case { name: "segment empty type", mutate: |doc| seg0(doc)["type"] = json!("") },
        Case { name: "segment non-string type", mutate: |doc| seg0(doc)["type"] = json!(1) },
        Case { name: "segment null size", mutate: |doc| seg0(doc)["size"] = Value::Null },
        Case {
            // A "data" array element need not even be an object: schema
            // validation only requires "data" itself to be a non-empty array.
            // The physical-field walk simply finds no offset/hex pair in a
            // bare string and treats it as non-physical, same as any other
            // descriptive value.
            name: "segment not an object",
            mutate: |doc| block0(doc)["data"] = json!(["oops"]),
        },
        Case {
            // A compressed block's sequence wrapper: no "type"/"offset"/"hex"
            // of its own, its child objects (token/literals) carry the
            // physical fields instead.
            name: "nested sequence-shaped data element",
            mutate: |doc| {
                block0(doc)["data"] = json!([{
                    "literal_length": 3,
                    "match_length": null,
                    "token": {"type": "token", "offset": 11, "size": 1, "hex": "30"},
                    "literals": {"type": "literals", "offset": 12, "size": 3, "hex": "010203"},
                    "match_offset": null,
                }]);
            },
        },
        Case {
            name: "optional fields present and non-null",
            mutate: |doc| {
                fd0(doc)["content_size"] = numeric_field_json(6, "0300000000000000");
                fd0(doc)["dictionary_id"] = numeric_field_json(14, "01000000");
            },
        },
        Case {
            // block_size.uncompressed is not used to decide anything at the
            // schema layer, so its absence must not be rejected. Populate
            // block_size with the extra "data_size" key real inspect output
            // carries before deleting "uncompressed", so this case exercises
            // the actual inspect-shaped document.
            name: "block_size missing uncompressed field",
            mutate: |doc| {
                let bs = &mut block0(doc)["block_size"];
                bs["data_size"] = json!(3);
                remove(bs, "uncompressed");
            },
        },
        Case {
            // Likewise, block_size.uncompressed's type is not checked.
            name: "block_size uncompressed wrong type",
            mutate: |doc| block0(doc)["block_size"]["uncompressed"] = json!("not a bool"),
        },
        Case {
            name: "block_checksum present and non-null",
            mutate: |doc| block0(doc)["block_checksum"] = numeric_field_json(15, "78563412"),
        },
        Case {
            name: "content_checksum present and non-null",
            mutate: |doc| doc["frames"][0]["content_checksum"] = numeric_field_json(15, "f0debc9a"),
        },
    ];

    for case in cases {
        let mut doc = valid_restore_doc();
        (case.mutate)(&mut doc);
        let data = must_serialize(&doc);

        match decode_restore_document(&data[..]) {
            Ok(got) => assert!(got.contains_key("frames"), "{}: decoded root has no frames", case.name),
            Err(err) => panic!("{}: decode_restore_document() error = {err}, want Ok", case.name),
        }
    }
}

// Reject

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

#[test]
fn restore_schema_rejects() {
    for case in restore_schema_reject_cases() {
        let mut doc = valid_restore_doc();
        (case.mutate)(&mut doc);
        let data = must_serialize(&doc);

        let err = decode_schema_err(&data);
        assert_eq!(err.path, case.want_path, "{}: path (err: {err})", case.name);
    }
}

#[test]
fn restore_schema_rejects_root_type() {
    let cases = [
        ("array root", "[]"),
        ("string root", r#""oops""#),
        ("number root", "1"),
        ("bool root", "true"),
        ("null root", "null"),
    ];

    for (name, body) in cases {
        let err = decode_schema_err(body.as_bytes());
        assert_eq!(err.path, "$", "{name}: path (err: {err})");
        assert_restore_rejects_untouched(body.as_bytes(), name);
    }
}

#[test]
fn restore_json_empty_input() {
    let err = decode_schema_err(b"");
    assert_eq!(err.path, "$", "path (err: {err})");
    assert_restore_rejects_untouched(b"", "empty input");
}

#[test]
fn restore_json_trailing_value() {
    let mut data = must_serialize(&valid_restore_doc());
    data.extend_from_slice(b" {}");

    let err = decode_schema_err(&data);
    assert_eq!(err.path, "$", "path (err: {err})");
    assert!(err.message.starts_with("unexpected value after document root"), "message = {:?}", err.message);
    assert_restore_rejects_untouched(&data, "trailing value");
}

/// Malformed JSON syntax on the initial decode (as opposed to well-formed JSON
/// of the wrong shape) is rejected as a schema error at the root path, with
/// the writer untouched. Only the stable message prefix is checked: the rest
/// of serde_json's syntax error text is not stable.
#[test]
fn restore_json_malformed_syntax() {
    let cases = [
        ("truncated object", r#"{"frames":"#),
        ("unterminated string", r#"{"frames": ""#),
        ("comma without element", r#"{"frames": [,]}"#),
    ];

    for (name, body) in cases {
        let err = decode_schema_err(body.as_bytes());
        assert_eq!(err.path, "$", "{name}: path (err: {err})");
        assert!(err.message.starts_with("invalid JSON:"), "{name}: message = {:?}", err.message);
        assert_restore_rejects_untouched(body.as_bytes(), name);
    }
}

/// Trailing bytes that are not themselves valid JSON (as opposed to a
/// well-formed but unexpected second value) are rejected too, through the
/// decode-error branch rather than the successfully-decoded-value branch.
#[test]
fn restore_json_malformed_trailing_content() {
    let mut data = must_serialize(&valid_restore_doc());
    data.extend_from_slice(b" ]");

    let err = decode_schema_err(&data);
    assert_eq!(err.path, "$", "path (err: {err})");
    assert!(err.message.starts_with("unexpected content after document root:"), "message = {:?}", err.message);
    assert_restore_rejects_untouched(&data, "malformed trailing content");
}

#[test]
fn restore_json_trailing_whitespace_accepted() {
    let mut data = must_serialize(&valid_restore_doc());
    data.extend_from_slice(b"\n\t  \n");

    if let Err(err) = decode_restore_document(&data[..]) {
        panic!("decode_restore_document() error = {err}, want Ok");
    }
}

#[test]
fn restore_schema_error_message() {
    let err = SchemaError::new("$.frames[0].data_blocks", "missing required field");

    const WANT: &str = "invalid restore JSON ($.frames[0].data_blocks): missing required field";
    assert_eq!(err.to_string(), WANT, "to_string()");
}
