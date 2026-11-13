use serde_json::{Map, Value};

use super::*;

/// A compressed-data sequence example in which match_offset (reusing
/// `NumericField<u16>` as-is) carries no "type" field, only offset/value/hex.
const BLOCK_FORMAT_EXAMPLE_JSON: &str = r#"[
  {
    "literal_length": 4,
    "match_length": 19,
    "token": {
      "type": "token",
      "offset": 19,
      "size": 1,
      "hex": "4f",
      "literal_length_code": 4,
      "match_length_code": 15
    },
    "literal_length_extension": null,
    "literals": {
      "type": "literals",
      "offset": 20,
      "size": 4,
      "hex": "61626364",
      "display": "abcd"
    },
    "match_offset": {
      "offset": 24,
      "value": 4,
      "hex": "0400"
    },
    "match_length_extension": {
      "type": "match_length_extension",
      "offset": 26,
      "size": 1,
      "hex": "00",
      "additional_length": 0
    }
  },
  {
    "literal_length": 5,
    "match_length": null,
    "token": {
      "type": "token",
      "offset": 27,
      "size": 1,
      "hex": "50",
      "literal_length_code": 5,
      "match_length_code": 0
    },
    "literal_length_extension": null,
    "literals": {
      "type": "literals",
      "offset": 28,
      "size": 5,
      "hex": "6566676869",
      "display": "efghi"
    },
    "match_offset": null,
    "match_length_extension": null
  }
]"#;

fn build_block_format_example() -> Vec<BlockData> {
    let first = Sequence {
        literal_length: 4,
        match_length: Some(19),
        token: TokenSegment {
            kind: SEGMENT_TYPE_TOKEN,
            offset: 19,
            size: 1,
            hex: "4f".to_string(),
            literal_length_code: 4,
            match_length_code: 15,
        },
        literal_length_extension: None,
        literals: LiteralsSegment {
            kind: SEGMENT_TYPE_LITERALS,
            offset: 20,
            size: 4,
            hex: "61626364".to_string(),
            display: display_string(b"abcd"),
        },
        match_offset: Some(NumericField::new(24, 4u16, &[0x04, 0x00])),
        match_length_extension: Some(LengthExtensionSegment {
            kind: SEGMENT_TYPE_MATCH_LENGTH_EXTENSION,
            offset: 26,
            size: 1,
            hex: "00".to_string(),
            additional_length: 0,
        }),
    };

    let second = Sequence {
        literal_length: 5,
        match_length: None,
        token: TokenSegment {
            kind: SEGMENT_TYPE_TOKEN,
            offset: 27,
            size: 1,
            hex: "50".to_string(),
            literal_length_code: 5,
            match_length_code: 0,
        },
        literal_length_extension: None,
        literals: LiteralsSegment {
            kind: SEGMENT_TYPE_LITERALS,
            offset: 28,
            size: 5,
            hex: "6566676869".to_string(),
            display: display_string(b"efghi"),
        },
        match_offset: None,
        match_length_extension: None,
    };

    vec![BlockData::Sequence(Box::new(first)), BlockData::Sequence(Box::new(second))]
}

/// Parses `data` into a generic JSON value.
fn to_generic_json(data: &str) -> Value {
    serde_json::from_str(data).unwrap_or_else(|err| panic!("from_str: {err}"))
}

/// Serializes `value` and parses the result into a generic JSON value.
fn serialize_generic<T: serde::Serialize>(value: &T) -> Value {
    to_generic_json(&serde_json::to_string(value).expect("to_string"))
}

/// Fails unless `m` has `key` present with a JSON null value; see the helper
/// of the same purpose in the sibling test module.
fn require_explicit_null(m: &Map<String, Value>, key: &str) {
    match m.get(key) {
        None => panic!("{key} is absent from JSON, want present with value null"),
        Some(v) => assert!(v.is_null(), "{key} = {v}, want null"),
    }
}

/// Fails unless `obj` has exactly the keys in `want_keys`.
fn require_exact_keys(what: &str, obj: &Map<String, Value>, want_keys: &[&str]) {
    assert_eq!(obj.len(), want_keys.len(), "{what} JSON has {} keys, want {}: {obj:?}", obj.len(), want_keys.len());
    for k in want_keys {
        assert!(obj.contains_key(*k), "{what} JSON missing key {k:?}: {obj:?}");
    }
}

/// Confirms the full compressed sequence shape: Sequence omits "type", field
/// names/values match, and match_offset carries only offset/value/hex.
#[test]
fn sequence_serialize_matches_block_format_example() {
    let data = build_block_format_example();

    let out = serde_json::to_string(&data).expect("to_string");
    let got = to_generic_json(&out);
    let want = to_generic_json(BLOCK_FORMAT_EXAMPLE_JSON);
    assert_eq!(
        got, want,
        "serialized JSON does not match block-format example.\ngot:  {out}\nwant: {BLOCK_FORMAT_EXAMPLE_JSON}"
    );
}

/// Confirms a serialized Sequence's top-level JSON object has no "type" key:
/// Sequence is not itself a physical field.
#[test]
fn sequence_omits_type_field() {
    let arr = serialize_generic(&build_block_format_example());

    for (i, elem) in arr.as_array().expect("not an array").iter().enumerate() {
        let obj = elem.as_object().expect("element is not an object");
        assert!(!obj.contains_key("type"), "sequence[{i}] has a \"type\" key, want none: {obj:?}");
    }
}

/// Confirms the last literal-only sequence's match_length, match_offset, and
/// match_length_extension serialize as explicit JSON null (present with value
/// null), not an omitted key.
#[test]
fn sequence_final_match_null_fields() {
    let arr = serialize_generic(&build_block_format_example());

    let last = arr.as_array().unwrap().last().unwrap().as_object().expect("last is not an object");
    for key in ["match_length", "match_offset", "match_length_extension"] {
        require_explicit_null(last, key);
    }
}

/// Confirms literal_length_extension serializes as explicit null when the
/// token's upper nibble is below 15 (no extension bytes present).
#[test]
fn sequence_literal_length_extension_null() {
    let arr = serialize_generic(&build_block_format_example());

    for elem in arr.as_array().expect("not an array") {
        require_explicit_null(elem.as_object().expect("element is not an object"), "literal_length_extension");
    }
}

/// Confirms a serialized TokenSegment carries only
/// type/offset/size/hex/literal_length_code/match_length_code, with no
/// "value" key.
#[test]
fn token_segment_has_no_value() {
    let tok = TokenSegment {
        kind: SEGMENT_TYPE_TOKEN,
        offset: 19,
        size: 1,
        hex: "4f".to_string(),
        literal_length_code: 4,
        match_length_code: 15,
    };

    let out = serialize_generic(&tok);
    let obj = out.as_object().expect("token is not an object");
    assert!(!obj.contains_key("value"), "token JSON has a \"value\" key, want none: {obj:?}");
    require_exact_keys("token", obj, &["type", "offset", "size", "hex", "literal_length_code", "match_length_code"]);
}

/// Confirms match_offset (a `NumericField<u16>`) carries only offset/value/hex: no "size" and no "type".
#[test]
fn match_offset_has_only_offset_value_hex() {
    let field = NumericField::new(24, 4u16, &[0x04, 0x00]);

    let out = serialize_generic(&field);
    require_exact_keys("match_offset", out.as_object().expect("not an object"), &["offset", "value", "hex"]);
}

/// Confirms a size-0 LiteralsSegment's display serializes as "" (not null, not
/// omitted).
#[test]
fn literals_display_empty_for_zero_size() {
    let seg = LiteralsSegment {
        kind: SEGMENT_TYPE_LITERALS,
        offset: 5,
        size: 0,
        hex: String::new(),
        display: display_string(&[]),
    };

    let out = serialize_generic(&seg);
    let obj = out.as_object().expect("literals is not an object");
    let display = obj.get("display").unwrap_or_else(|| panic!("literals JSON missing \"display\" key: {obj:?}"));
    assert_eq!(display, &Value::String(String::new()), "display, want empty string");
}

/// Confirms an uncompressed_data segment with size 0 still serializes as an
/// element (not omitted) with an empty display string.
#[test]
fn uncompressed_data_segment_size_zero() {
    let seg = UncompressedDataSegment {
        kind: SEGMENT_TYPE_UNCOMPRESSED_DATA,
        offset: 19,
        size: 0,
        hex: String::new(),
        display: display_string(&[]),
    };
    let data = vec![BlockData::Uncompressed(seg)];

    let out = serialize_generic(&data);
    let arr = out.as_array().expect("not an array");
    assert_eq!(arr.len(), 1, "len(data)");
    let obj = arr[0].as_object().expect("element is not an object");
    require_exact_keys("uncompressed_data", obj, &["type", "offset", "size", "hex", "display"]);
    assert_eq!(obj["type"], Value::String(SEGMENT_TYPE_UNCOMPRESSED_DATA.to_string()), "type");
    assert_eq!(obj["display"], Value::String(String::new()), "display, want empty string");
}

/// Confirms `display_string` follows the fixed rule: backslash and double-quote
/// escape, printable ASCII passes through, LF/CR/TAB use their C escapes, and
/// every other byte becomes uppercase-hex \xHH -- never a lossy placeholder.
#[test]
fn display_string_escaping() {
    let cases: [(&str, &[u8], &str); 10] = [
        ("empty", b"", ""),
        ("printable ASCII passthrough", b"Hello, World! 0-9", r"Hello, World! 0-9"),
        ("backslash", b"\\", r"\\"),
        ("double quote", b"\"", r#"\""#),
        ("newline", b"hello\nworld", r"hello\nworld"),
        ("carriage return", b"\r", r"\r"),
        ("tab", b"\t", r"\t"),
        ("NUL", &[0x00], r"\x00"),
        ("high byte", &[0xff], r"\xFF"),
        ("space boundary", &[0x1f, 0x20, 0x7e, 0x7f], "\\x1F ~\\x7F"),
    ];

    for (name, raw, want) in cases {
        let got = display_string(raw);
        assert_eq!(got, want, "{name}: display_string({raw:02x?})");
    }
}

/// Confirms `DataBlock::data`, typed as `Vec<BlockData>`, serializes each
/// element per its concrete variant (Sequence vs UncompressedDataSegment).
#[test]
fn block_data_enum_serializes_concrete_type() {
    let block = DataBlock {
        block_size: BlockSize { field: NumericField::new(0, 0u32, &[0, 0, 0, 0]), data_size: 5, uncompressed: true },
        data: vec![BlockData::Uncompressed(UncompressedDataSegment {
            kind: SEGMENT_TYPE_UNCOMPRESSED_DATA,
            offset: 19,
            size: 5,
            hex: "68656c6c6f".to_string(),
            display: display_string(b"hello"),
        })],
        block_checksum: None,
    };

    let out = serialize_generic(&block);
    let elem = &out["data"][0];
    assert_eq!(elem["type"], Value::String(SEGMENT_TYPE_UNCOMPRESSED_DATA.to_string()), "data[0].type");
    assert_eq!(elem["display"], Value::String("hello".to_string()), "data[0].display");
}
