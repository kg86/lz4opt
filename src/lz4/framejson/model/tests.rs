use serde_json::{json, Map, Value};

use super::*;

/// Confirms hex encodes the raw bytes in the order they appeared in the input,
/// not a big-endian rendering of the value.
#[test]
fn numeric_field_little_endian_hex() {
    let raw = [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01];
    let value = decode_u64_le(&raw);

    const WANT: u64 = 0x0102030405060708;
    assert_eq!(value, WANT, "decode_u64_le({raw:02x?}) = {value:#x}, want {WANT:#x}");

    let field = NumericField::new(0, value, &raw);
    assert_eq!(field.hex, "0807060504030201", "hex");
}

/// Confirms `u64::MAX` round-trips through JSON encoding without losing
/// precision (i.e. it is not routed through f64).
#[test]
fn numeric_field_max_uint64() {
    let raw = [0xff; 8];
    let field = NumericField::new(6, u64::MAX, &raw);

    let out = serde_json::to_string(&field).expect("to_string");

    const WANT: &str = r#"{"offset":6,"value":18446744073709551615,"hex":"ffffffffffffffff"}"#;
    assert_eq!(out, WANT, "serialized field");

    let decoded: Value = serde_json::from_str(&out).expect("from_str");
    assert_eq!(decoded["value"].as_u64(), Some(u64::MAX), "decoded value");
}

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

/// Constructs the model equivalent of `EXAMPLE_JSON`, field for
/// field.
fn build_example() -> Root {
    let content_size = NumericField::new(6, 3u64, &[0x03, 0, 0, 0, 0, 0, 0, 0]);
    let block_checksum = NumericField::new(22, 0x12345678u32, &[0x78, 0x56, 0x34, 0x12]);
    let content_checksum = NumericField::new(30, 0x9abcdef0u32, &[0xf0, 0xde, 0xbc, 0x9a]);

    let normal = NormalFrame {
        magic_number: NumericField::new(0, 0x184D2204u32, &[0x04, 0x22, 0x4d, 0x18]),
        frame_descriptor: FrameDescriptor {
            flg: Flg {
                field: NumericField::new(4, 0x7cu8, &[0x7c]),
                version: 1,
                block_independence: true,
                block_checksum: true,
                content_size_present: true,
                content_checksum: true,
                reserved_bit: 0,
                dictionary_id_present: false,
            },
            bd: Bd {
                field: NumericField::new(5, 0x40u8, &[0x40]),
                reserved_bit_7: 0,
                block_max_size_code: 4,
                block_max_size_bytes: 65536,
                reserved_bits_3_0: 0,
            },
            content_size: Some(content_size),
            dictionary_id: None,
            header_checksum: NumericField::new(14, 42u8, &[0x2a]),
        },
        data_blocks: vec![DataBlock {
            block_size: BlockSize {
                field: NumericField::new(15, 0x80000003u32, &[0x03, 0x00, 0x00, 0x80]),
                data_size: 3,
                uncompressed: true,
            },
            data: vec![BlockData::Uncompressed(UncompressedDataSegment {
                kind: SEGMENT_TYPE_UNCOMPRESSED_DATA,
                offset: 19,
                size: 3,
                hex: "010203".to_string(),
                display: display_string(&[0x01, 0x02, 0x03]),
            })],
            block_checksum: Some(block_checksum),
        }],
        end_mark: NumericField::new(26, 0u32, &[0, 0, 0, 0]),
        content_checksum: Some(content_checksum),
    };

    let skippable = SkippableFrame {
        magic_number: NumericField::new(34, 0x184D2A50u32, &[0x50, 0x2a, 0x4d, 0x18]),
        frame_size: NumericField::new(38, 3u32, &[0x03, 0, 0, 0]),
        data: SkippableData { offset: 42, size: 3, hex: "aabbcc".to_string() },
    };

    Root { frames: vec![Frame::Normal(Box::new(normal)), Frame::Skippable(skippable)] }
}

/// Parses `data` into a generic JSON value.
fn to_generic_json(data: &str) -> Value {
    serde_json::from_str(data).unwrap_or_else(|err| panic!("from_str: {err}"))
}

/// Confirms the model can express `EXAMPLE_JSON` exactly,
/// and that no excluded fields (raw, checksum valid) leak into the output.
#[test]
fn model_matches_example() {
    let root = build_example();

    let out = serde_json::to_string(&root).expect("to_string");
    let got = to_generic_json(&out);
    let want = to_generic_json(EXAMPLE_JSON);
    assert_eq!(got, want, "serialized JSON does not match EXAMPLE_JSON.\ngot:  {out}\nwant: {EXAMPLE_JSON}");

    // The indented rendering must carry the same document.
    let pretty = root.to_json();
    assert!(pretty.ends_with("}\n"), "to_json() must end with a newline: {pretty:?}");
    assert_eq!(to_generic_json(&pretty), want, "to_json() does not match EXAMPLE_JSON");
    assert_eq!(pretty, format!("{EXAMPLE_JSON}\n"), "to_json() formatting changed");

    for excluded in [r#""raw""#, r#""valid""#] {
        assert!(!out.contains(excluded), "serialized JSON contains excluded field {excluded}");
    }
}

/// Confirms Content Size, Dictionary ID, Block Checksum, and Content Checksum
/// encode as JSON null when absent.
#[test]
fn optional_fields_encode_as_null() {
    let frame = NormalFrame {
        magic_number: NumericField::new(0, 0x184D2204u32, &[0x04, 0x22, 0x4d, 0x18]),
        frame_descriptor: FrameDescriptor {
            flg: Flg {
                field: NumericField::new(4, 0u8, &[0]),
                version: 0,
                block_independence: false,
                block_checksum: false,
                content_size_present: false,
                content_checksum: false,
                reserved_bit: 0,
                dictionary_id_present: false,
            },
            bd: Bd {
                field: NumericField::new(5, 0x40u8, &[0x40]),
                reserved_bit_7: 0,
                block_max_size_code: 4,
                block_max_size_bytes: 65536,
                reserved_bits_3_0: 0,
            },
            content_size: None,
            dictionary_id: None,
            header_checksum: NumericField::new(6, 0u8, &[0]),
        },
        data_blocks: vec![DataBlock {
            block_size: BlockSize {
                field: NumericField::new(7, 0x80000000u32, &[0, 0, 0, 0x80]),
                data_size: 0,
                uncompressed: true,
            },
            data: vec![BlockData::Uncompressed(UncompressedDataSegment {
                kind: SEGMENT_TYPE_UNCOMPRESSED_DATA,
                offset: 11,
                size: 0,
                hex: String::new(),
                display: String::new(),
            })],
            block_checksum: None,
        }],
        end_mark: NumericField::new(11, 0u32, &[0, 0, 0, 0]),
        content_checksum: None,
    };

    let out = serde_json::to_string(&frame).expect("to_string");

    let got = to_generic_json(&out);
    let got = got.as_object().expect("frame is not an object");
    require_explicit_null(got, "content_checksum");

    let fd = got["frame_descriptor"].as_object().expect("frame_descriptor is not an object");
    require_explicit_null(fd, "content_size");
    require_explicit_null(fd, "dictionary_id");

    let block = got["data_blocks"][0].as_object().expect("data_blocks[0] is not an object");
    require_explicit_null(block, "block_checksum");
}

/// Fails unless `m` has `key` present with a JSON null value. A plain
/// `m[key].is_null()` check cannot distinguish "key absent" from "key present
/// with value null", since indexing a `Map` yields null for both; `get` is
/// required.
fn require_explicit_null(m: &Map<String, Value>, key: &str) {
    match m.get(key) {
        None => panic!("{key} is absent from JSON, want present with value null"),
        Some(v) => assert!(v.is_null(), "{key} = {v}, want null"),
    }
}

/// Confirms `Root::frames`, typed as the `Frame` enum, serializes each element
/// per its concrete variant.
#[test]
fn frame_enum_serializes_concrete_type() {
    let root = Root {
        frames: vec![Frame::Skippable(SkippableFrame {
            magic_number: NumericField::new(0, 0x184D2A50u32, &[0x50, 0x2a, 0x4d, 0x18]),
            frame_size: NumericField::new(4, 0u32, &[0, 0, 0, 0]),
            data: SkippableData { offset: 8, size: 0, hex: String::new() },
        })],
    };

    let out = serde_json::to_string(&root).expect("to_string");
    assert!(out.contains(r#""type":"skippable_frame""#), "to_string = {out}, want it to contain skippable_frame type");
    assert!(
        !out.contains(r#""frame_descriptor""#),
        "to_string = {out}, skippable frame must not carry normal-frame fields"
    );
}

/// Confirms `Root::to_json` escapes `<`, `>`, `&`, U+2028, and U+2029 in
/// strings, and indents with two spaces.
#[test]
fn to_json_escapes_html() {
    let root = Root {
        frames: vec![Frame::Skippable(SkippableFrame {
            magic_number: NumericField::new(0, 0x184D2A50u32, &[0x50, 0x2a, 0x4d, 0x18]),
            frame_size: NumericField::new(4, 0u32, &[0, 0, 0, 0]),
            data: SkippableData { offset: 8, size: 0, hex: "<>&\u{2028}\u{2029}".to_string() },
        })],
    };

    let out = root.to_json();
    let want = ["\\u003c", "\\u003e", "\\u0026", "\\u2028", "\\u2029"].concat();
    assert!(out.contains(&format!("\"hex\": \"{want}\"")), "to_json() = {out}");
    assert!(out.starts_with("{\n  \"frames\": [\n    {\n"), "to_json() indentation = {out}");
    assert_eq!(to_generic_json(&out)["frames"][0]["data"]["hex"], json!("<>&\u{2028}\u{2029}"), "round-tripped hex");
}
