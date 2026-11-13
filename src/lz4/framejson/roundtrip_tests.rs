//! Cross-cutting tests that drive real byte fixtures through `parse`,
//! `Root::to_json`, and `restore` together (as the `inspect` and `restore`
//! commands do) and check the contract those pieces jointly promise: every
//! fixture round-trips to identical bytes, and the JSON text they produce has
//! the documented shape, null fields, and full offset/hex coverage of the
//! input.

use serde_json::{json, Value};

use super::test_fixtures::{
    bd_byte, block_size_bytes, descriptor_fixture, end_mark_bytes, flg_byte, literal_only_block_bytes,
    minimal_normal_frame_bytes, skippable_frame_bytes, u32_le, valid_magic_bytes,
};
use super::*;

// --- fixture builders specific to round trips ---

/// Returns a single-block normal Frame with both Block Checksum and Content
/// Checksum present (Content Size and Dictionary ID absent), covering a
/// checksum combination none of the other fixture helpers exercise end to end.
fn frame_with_all_checksums_bytes() -> Vec<u8> {
    let flg = flg_byte(1, true, true, false, true, 0, false);
    let bd = bd_byte(0, 4, 0);
    let descriptor = descriptor_fixture(flg, bd, None, None, 0x2a);
    let data = literal_only_block_bytes(&[0x01, 0x02, 0x03]);
    [
        valid_magic_bytes(),
        descriptor,
        block_size_bytes(false, data.len() as u32),
        data,
        u32_le(0x12345678), // block checksum
        end_mark_bytes(),
        u32_le(0xf0debc9a), // content checksum
    ]
    .concat()
}

/// Returns a normal Frame with three Data Blocks: one compressed, one
/// uncompressed, and a trailing empty (size 0) uncompressed block sharing its
/// offset with the following EndMark.
fn multi_block_frame_bytes() -> Vec<u8> {
    let flg = flg_byte(1, true, false, false, false, 0, false);
    let bd = bd_byte(0, 4, 0);
    let descriptor = descriptor_fixture(flg, bd, None, None, 0x2a);
    let block0 = literal_only_block_bytes(&[0x01, 0x02, 0x03]);
    let block1 = vec![0xaa, 0xbb];
    [
        valid_magic_bytes(),
        descriptor,
        block_size_bytes(false, block0.len() as u32),
        block0,
        block_size_bytes(true, block1.len() as u32),
        block1,
        block_size_bytes(true, 0), // empty uncompressed block
        end_mark_bytes(),
    ]
    .concat()
}

/// Returns a normal Frame whose only Data Block is the empty (size 0)
/// uncompressed block, isolating that shape from the multi-block fixture.
fn empty_uncompressed_block_frame_bytes() -> Vec<u8> {
    let flg = flg_byte(1, true, false, false, false, 0, false);
    let bd = bd_byte(0, 4, 0);
    let descriptor = descriptor_fixture(flg, bd, None, None, 0x2a);
    [valid_magic_bytes(), descriptor, block_size_bytes(true, 0), end_mark_bytes()].concat()
}

/// Mixes a fully-checksummed normal Frame, a Skippable Frame carrying data, a
/// minimal normal Frame, and a size-0 Skippable Frame, exercising Frame
/// Concatenation across both Frame kinds in one stream.
fn concatenated_mixed_stream_bytes() -> Vec<u8> {
    [
        frame_with_all_checksums_bytes(),
        skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MIN, &[0x01]),
        minimal_normal_frame_bytes(),
        skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MAX, &[]),
    ]
    .concat()
}

// --- compressed block sequence fixtures ---

/// Returns a compressed block's data with two sequences chosen so every Block
/// Format physical segment type (token, literal_length_extension, literals,
/// match_offset, match_length_extension) appears at least once, and the final
/// sequence exercises the literal-only shape (null match_offset/
/// match_length_extension/match_length). Sequence 1: literal length 16 (token
/// code 15 + extension byte 0x01), match length 21 (token code 15 + extension
/// byte 0x02), match offset 100. Sequence 2 (final): literal length 5, no
/// match. Expanded size is 16+21+5=42; the match starts at offset 16, 26 bytes
/// before the expanded end, satisfying the >=12 End of block margin.
fn compressed_block_all_segment_types_bytes() -> Vec<u8> {
    let mut out = vec![0xff]; // token: literal code 15, match code 15
    out.push(0x01); // literal_length_extension: additional 1 (=16)
    out.extend_from_slice(b"ABCDEFGHIJKLMNOP"); // 16 literal bytes
    out.extend([0x64, 0x00]); // match_offset little-endian: 100
    out.push(0x02); // match_length_extension: additional 2 (=4+15+2=21)
    out.push(0x50); // token: literal code 5, match code 0 (final, literal-only)
    out.extend_from_slice(b"QRSTU"); // 5 literal bytes
    out
}

/// Returns a compressed block's data with a non-final sequence whose literal
/// length is 0, so its zero-size "literals" physical field shares its offset
/// with the following "match_offset" field, immediately followed by a final
/// literal-only sequence. Sequence 1: literal length 0, match length 4 (token
/// code 0), match offset 1. Sequence 2 (final): literal length 8. Expanded
/// size is 0+4+8=12; the match starts at offset 0, exactly 12 bytes before the
/// expanded end, the minimum allowed margin.
fn compressed_block_zero_literal_before_match_bytes() -> Vec<u8> {
    let mut out = vec![0x00]; // token: literal code 0, match code 0 (match length 4)
    out.extend([1, 0]); // match_offset little-endian: 1
    out.push(0x80); // token: literal code 8, match code 0 (final, literal-only)
    out.extend_from_slice(b"abcdefgh");
    out
}

/// Returns the Block Format's representation of an empty expanded block: a
/// single token byte (0x00) whose zero-size "literals" physical field shares
/// its offset with whatever immediately follows the compressed Data Block.
fn compressed_block_empty_bytes() -> Vec<u8> {
    vec![0x00]
}

/// Returns a single normal Frame whose three Data Blocks are, in order,
/// `compressed_block_all_segment_types_bytes`,
/// `compressed_block_zero_literal_before_match_bytes`, and
/// `compressed_block_empty_bytes`, each with a Block Checksum, so the empty
/// block's zero-size final literals field sits at the same offset as a
/// following physical field (its own Block Checksum) rather than only at
/// end-of-stream.
fn compressed_sequence_frame_bytes() -> Vec<u8> {
    let flg = flg_byte(1, true, true, false, false, 0, false);
    let bd = bd_byte(0, 4, 0);
    let descriptor = descriptor_fixture(flg, bd, None, None, 0x2a);

    let block0 = compressed_block_all_segment_types_bytes();
    let block1 = compressed_block_zero_literal_before_match_bytes();
    let block2 = compressed_block_empty_bytes();

    [
        valid_magic_bytes(),
        descriptor,
        block_size_bytes(false, block0.len() as u32),
        block0,
        u32_le(0x11111111),
        block_size_bytes(false, block1.len() as u32),
        block1,
        u32_le(0x22222222),
        block_size_bytes(false, block2.len() as u32),
        block2,
        u32_le(0x33333333),
        end_mark_bytes(),
    ]
    .concat()
}

// --- helpers ---

/// Restores `json` into a byte vector.
fn restore_to_vec(json: &[u8]) -> Result<Vec<u8>, RestoreError> {
    let mut buf = Vec::new();
    restore(&mut buf, json)?;
    Ok(buf)
}

/// Parses `input` and renders the inspect JSON text.
fn inspect(input: &[u8]) -> String {
    parse(input).expect("parse").to_json()
}

/// Decodes JSON text generically (not via the typed model).
fn to_generic_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("from_str: {e}"))
}

/// Returns the "data" array of the first Data Block of the first frame in a
/// generically decoded restore document.
fn data_array_of_first_block(doc: &mut Value) -> &mut Vec<Value> {
    doc.pointer_mut("/frames/0/data_blocks/0/data").and_then(Value::as_array_mut).expect("first block data array")
}

#[test]
fn round_trip_compressed_block_sequences() {
    let input = compressed_sequence_frame_bytes();
    let inspected = inspect(&input);

    let got = restore_to_vec(inspected.as_bytes()).expect("restore");
    assert_eq!(got, input, "round trip mismatch");
}

/// Confirms the array order check operates on the "data" array's sequence
/// elements themselves, not just on flat physical-field segments: reversing
/// `compressed_block_all_segment_types_bytes`' two sequences (whose nested
/// physical fields do not overlap in offset) is rejected before any bytes are
/// written.
#[test]
fn restore_rejects_reversed_sequence_order() {
    let input = compressed_sequence_frame_bytes();
    let mut doc = to_generic_json(&inspect(&input));
    let data = data_array_of_first_block(&mut doc);
    assert_eq!(data.len(), 2, "first block data array length");
    data.swap(0, 1);

    let mut buf = Vec::new();
    let err = restore(&mut buf, serde_json::to_vec(&doc).unwrap().as_slice())
        .expect_err("restore(reversed sequence order) = Ok, want SchemaError");
    let RestoreError::Schema(schema_err) = &err else { panic!("error is {err:?}, want RestoreError::Schema") };
    assert_eq!(
        schema_err.path, "$.frames[0].data_blocks[0].data[1]",
        "path (must be the array order check, not an unrelated rejection)"
    );
    assert!(buf.is_empty(), "restore(reversed sequence order) wrote {} bytes before failing, want 0", buf.len());
}

/// Confirms that editing a sequence's computed/descriptive attributes
/// (literal_length, match_length, token codes, literals.display,
/// match_offset.value, and block_size.uncompressed) leaves the restored bytes
/// unchanged, since only each physical field's own offset/hex decides what
/// restore writes.
#[test]
fn restore_ignores_sequence_computed_attributes() {
    let input = compressed_sequence_frame_bytes();
    let inspected = inspect(&input);

    let baseline = restore_to_vec(inspected.as_bytes()).expect("restore(baseline)");

    let original = to_generic_json(&inspected);
    let mut doc = original.clone();
    {
        let seq0 = &mut data_array_of_first_block(&mut doc)[0];
        seq0["literal_length"] = json!(999);
        seq0["match_length"] = json!(999);
        seq0["token"]["literal_length_code"] = json!(0);
        seq0["token"]["match_length_code"] = json!(0);
        seq0["literals"]["display"] = json!("not the real bytes");
        seq0["match_offset"]["value"] = json!(0);
    }
    doc["frames"][0]["data_blocks"][0]["block_size"]["uncompressed"] = json!(true);

    assert_ne!(original, doc, "mutations did not reach the document: mutated JSON is identical to the baseline");

    let mutated = restore_to_vec(&serde_json::to_vec(&doc).unwrap()).expect("restore(mutated)");
    assert_eq!(baseline, mutated, "restore(mutated) must be unchanged by computed attributes");
}

// --- fixture -> inspect JSON -> restore round trips ---

/// Confirms that for each named fixture, parsing it, rendering the result
/// exactly as inspect does, and restoring that JSON exactly as restore does
/// reproduces the original bytes precisely. It covers a minimal Frame, a Frame
/// with all checksums present, multiple Data Blocks, an empty non-compressed
/// block, a standalone Skippable Frame, and a concatenated stream mixing
/// normal and Skippable Frames.
#[test]
fn round_trip_fixtures() {
    let tests: Vec<(&str, Vec<u8>)> = vec![
        ("minimal frame", minimal_normal_frame_bytes()),
        ("frame with all checksums", frame_with_all_checksums_bytes()),
        ("multiple data blocks", multi_block_frame_bytes()),
        ("empty non-compressed block", empty_uncompressed_block_frame_bytes()),
        ("skippable frame alone", skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MIN, &[0xaa, 0xbb, 0xcc])),
        ("concatenated stream mixing normal and skippable frames", concatenated_mixed_stream_bytes()),
    ];

    for (name, input) in tests {
        let inspected = inspect(&input);
        let got = restore_to_vec(inspected.as_bytes()).unwrap_or_else(|e| panic!("{name}: restore: {e}"));
        assert_eq!(got, input, "{name}: round trip mismatch");
    }
}

// --- generic JSON schema, null fields, and offset/hex coverage ---

/// Walks decoded JSON and returns every object's `(offset, hex byte length)`
/// pair, treating any object with both keys as a physical field exactly as
/// restore's own tree walk does, but reimplemented independently here, over the
/// generic JSON shape rather than restore's internal types, so this test can
/// catch a bug shared by both the model and the restore code.
fn collect_generic_physical_fields(doc: &Value) -> Vec<(u64, usize)> {
    fn walk(v: &Value, fields: &mut Vec<(u64, usize)>) {
        match v {
            Value::Object(node) => {
                if let (Some(offset), Some(hex)) = (node.get("offset"), node.get("hex")) {
                    let offset = offset.as_u64().unwrap_or_else(|| panic!("offset has unexpected JSON value {offset}"));
                    let hex = hex.as_str().unwrap_or_else(|| panic!("hex has unexpected JSON value {hex}"));
                    fields.push((offset, hex.len() / 2));
                }
                for child in node.values() {
                    walk(child, fields);
                }
            }
            Value::Array(node) => {
                for child in node {
                    walk(child, fields);
                }
            }
            _ => {}
        }
    }
    let mut fields = Vec::new();
    walk(doc, &mut fields);
    fields
}

/// Independently re-derives physical field byte ranges from the raw inspect
/// JSON text, via generic decoding rather than the restore code, and confirms
/// that concatenating them in offset order accounts for every byte of the
/// input exactly once with no gap or overlap.
#[test]
fn inspect_json_physical_fields_cover_every_input_byte() {
    let input = concatenated_mixed_stream_bytes();

    let doc = to_generic_json(&inspect(&input));
    let mut fields = collect_generic_physical_fields(&doc);
    fields.sort_by_key(|f| f.0);

    // Group fields by offset before checking the cursor: a zero-length field
    // must still sit at the current cursor (it just does not advance it), so
    // skipping zero-length fields entirely would let one drift to a stale
    // offset unnoticed. At most one length>=1 field is allowed per offset
    // group.
    let mut cursor = 0u64;
    let mut i = 0;
    while i < fields.len() {
        let offset = fields[i].0;
        assert_eq!(offset, cursor, "physical fields have a gap or overlap at offset {offset} (cursor was at {cursor})");
        let mut non_zero_length = None;
        let mut j = i;
        while j < fields.len() && fields[j].0 == offset {
            if fields[j].1 > 0 {
                assert!(non_zero_length.is_none(), "offset {offset} has more than one length>=1 physical field");
                non_zero_length = Some(fields[j].1);
            }
            j += 1;
        }
        if let Some(len) = non_zero_length {
            cursor += len as u64;
        }
        i = j;
    }
    assert_eq!(
        cursor,
        input.len() as u64,
        "physical fields cover {cursor} bytes, want {} (full input length)",
        input.len()
    );
}

/// Asserts that `m` has `key` present with an explicit JSON null value.
fn require_explicit_null(m: &Value, key: &str) {
    let v = m.get(key).unwrap_or_else(|| panic!("{key} is absent from JSON, want present with value null"));
    assert!(v.is_null(), "{key} = {v}, want null");
}

/// Decodes real inspect JSON output generically and confirms it has the
/// documented shape: both frame types carry their required keys, and optional
/// fields absent from the input encode as explicit JSON null rather than being
/// omitted or defaulting to zero values.
#[test]
fn inspect_json_schema_shape_and_null_fields() {
    let got = to_generic_json(&inspect(&frame_with_all_checksums_bytes()));

    let frames = got["frames"].as_array().filter(|f| f.len() == 1).unwrap_or_else(|| {
        panic!("frames = {}, want a one-element array", got["frames"]);
    });
    let f = &frames[0];
    assert_eq!(f["type"], FRAME_TYPE_LZ4, "type");
    for key in ["magic_number", "frame_descriptor", "data_blocks", "end_mark", "content_checksum"] {
        assert!(f.get(key).is_some(), "normal frame is missing {key:?}");
    }
    assert!(
        !f["content_checksum"].is_null(),
        "content_checksum = null, want populated (fixture has a Content Checksum)"
    );

    let fd = &f["frame_descriptor"];
    assert!(fd.is_object(), "frame_descriptor = {fd}, want an object");
    require_explicit_null(fd, "content_size");
    require_explicit_null(fd, "dictionary_id");
    for key in ["flg", "bd", "header_checksum"] {
        assert!(fd.get(key).is_some(), "frame_descriptor is missing {key:?}");
    }

    let blocks = f["data_blocks"].as_array().filter(|b| b.len() == 1).unwrap_or_else(|| {
        panic!("data_blocks = {}, want a one-element array", f["data_blocks"]);
    });
    assert!(blocks[0].is_object(), "data_blocks[0] = {}, want an object", blocks[0]);
    assert!(
        !blocks[0]["block_checksum"].is_null(),
        "block_checksum = null, want populated (fixture has a Block Checksum)"
    );

    // A standalone Skippable Frame exercises the other frame shape and its own
    // required keys, distinct from the normal-frame shape above.
    let skip_got = to_generic_json(&inspect(&skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MIN, &[])));
    let skip_frames = skip_got["frames"].as_array().filter(|f| f.len() == 1).unwrap_or_else(|| {
        panic!("frames = {}, want a one-element array", skip_got["frames"]);
    });
    let sf = &skip_frames[0];
    assert_eq!(sf["type"], FRAME_TYPE_SKIPPABLE, "type");
    for key in ["magic_number", "frame_size", "data"] {
        assert!(sf.get(key).is_some(), "skippable frame is missing {key:?}");
    }
}
