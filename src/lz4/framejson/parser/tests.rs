use super::super::test_fixtures::{bd_byte, descriptor_fixture, flg_byte, u32_le, u64_le, valid_magic_bytes};
use super::*;

// --- fixture builders -------------------------------------------------

/// Parses a normal frame's Magic Number and Frame Descriptor in one call, for
/// tests that exercise both together. The Magic Number is read without
/// validating its value, since magic-number acceptance and rejection belong to
/// `parse` (covered by the stream tests), not to this field-layout helper.
pub(super) fn parse_header<R: Read>(
    fr: &mut FrameReader<R>,
) -> Result<(NumericField<u32>, FrameDescriptor), ParseError> {
    let magic = read_numeric_field(fr, 4, "magic_number", decode_u32_le)?;
    let fd = parse_frame_descriptor(fr)?;
    Ok((magic, fd))
}

/// Returns the magic number followed by a descriptor fixture.
fn header_input(descriptor: &[u8]) -> Vec<u8> {
    let mut input = valid_magic_bytes();
    input.extend_from_slice(descriptor);
    input
}

/// Asserts `err` is a `ParseError` at `offset` naming `path`.
fn assert_parse_error_at(err: &ParseError, offset: u64, path: &str, label: &str) {
    assert!(
        err.offset == offset && err.path == path,
        "{label}: err = {err}, want ParseError at offset {offset} path {path}"
    );
}

// --- Magic Number + Frame Descriptor variants --------------------------

#[test]
fn parse_frame_descriptor_variants() {
    let valid_bd = bd_byte(0, 4, 0);

    let cases: [(&str, Option<u64>, Option<u32>); 4] = [
        ("minimal", None, None),
        ("content size only", Some(3), None),
        ("dictionary id only", None, Some(0xAABBCCDD)),
        ("content size and dictionary id", Some(3), Some(0xAABBCCDD)),
    ];

    for (name, content_size, dictionary_id) in cases {
        let flg = flg_byte(1, true, false, content_size.is_some(), false, 0, dictionary_id.is_some());
        let descriptor = descriptor_fixture(flg, valid_bd, content_size, dictionary_id, 0x2a);
        let input = header_input(&descriptor);

        let mut fr = FrameReader::new(&input[..]);
        let (magic, fd) = parse_header(&mut fr).unwrap_or_else(|e| panic!("{name}: parse_header: {e}"));

        assert!(magic.value == MAGIC_NUMBER_LZ4 && magic.offset == 0, "{name}: magic = {magic:?}");
        assert_eq!(magic.hex, hex_string(&valid_magic_bytes()), "{name}: magic.hex");

        assert!(fd.flg.field.offset == 4 && fd.flg.field.value == flg, "{name}: FLG = {:?}", fd.flg);
        assert_eq!(fd.flg.field.hex, hex_string(&[flg]), "{name}: FLG.hex");
        assert!(fd.flg.block_independence && !fd.flg.block_checksum, "{name}: FLG flags decoded wrong: {:?}", fd.flg);
        assert_eq!(fd.flg.content_size_present, content_size.is_some(), "{name}: content_size_present");
        assert_eq!(fd.flg.dictionary_id_present, dictionary_id.is_some(), "{name}: dictionary_id_present");

        assert!(
            fd.bd.field.offset == 5 && fd.bd.block_max_size_code == 4 && fd.bd.block_max_size_bytes == 65536,
            "{name}: BD = {:?}",
            fd.bd
        );
        assert_eq!(fd.bd.field.hex, hex_string(&[valid_bd]), "{name}: BD.hex");

        let mut want_next_offset = 6u64;
        match (content_size, &fd.content_size) {
            (Some(want), Some(got)) => {
                assert!(
                    got.offset == want_next_offset && got.value == want,
                    "{name}: content_size = {got:?}, want offset {want_next_offset} value {want}"
                );
                assert_eq!(got.hex, hex_string(&u64_le(want)), "{name}: content_size.hex");
                want_next_offset += 8;
            }
            (None, None) => {}
            (want, got) => panic!("{name}: content_size = {got:?}, want {want:?}"),
        }

        match (dictionary_id, &fd.dictionary_id) {
            (Some(want), Some(got)) => {
                assert!(
                    got.offset == want_next_offset && got.value == want,
                    "{name}: dictionary_id = {got:?}, want offset {want_next_offset} value {want:#x}"
                );
                assert_eq!(got.hex, hex_string(&u32_le(want)), "{name}: dictionary_id.hex");
                want_next_offset += 4;
            }
            (None, None) => {}
            (want, got) => panic!("{name}: dictionary_id = {got:?}, want {want:?}"),
        }

        assert!(
            fd.header_checksum.offset == want_next_offset && fd.header_checksum.value == 0x2a,
            "{name}: header_checksum = {:?}, want offset {want_next_offset} value 0x2a",
            fd.header_checksum
        );
        assert_eq!(fd.header_checksum.hex, hex_string(&[0x2a]), "{name}: header_checksum.hex");

        assert_eq!(fr.offset, input.len() as u64, "{name}: reader offset (all input consumed)");
    }
}

/// Confirms the stored Header Checksum byte is read verbatim regardless of
/// value: this crate never recomputes or validates checksums.
#[test]
fn parse_frame_descriptor_header_checksum_not_recomputed() {
    let flg = flg_byte(1, true, false, false, false, 0, false);
    let bd = bd_byte(0, 4, 0);

    for checksum in [0x00u8, 0x2a, 0xff] {
        let input = header_input(&descriptor_fixture(flg, bd, None, None, checksum));

        let (_, fd) = parse_header(&mut FrameReader::new(&input[..]))
            .unwrap_or_else(|e| panic!("checksum {checksum:#x}: parse_header: {e}"));
        assert_eq!(fd.header_checksum.value, checksum, "checksum {checksum:#x}: header_checksum.value");
    }
}

/// Confirms Content Size decodes through the actual byte-reading path (not
/// just the model/JSON layer) without losing precision when every byte is
/// 0xff, i.e. the field holds `u64::MAX`.
#[test]
fn parse_frame_descriptor_content_size_max_uint64() {
    let flg = flg_byte(1, true, false, true, false, 0, false);
    let bd = bd_byte(0, 4, 0);
    let input = header_input(&descriptor_fixture(flg, bd, Some(u64::MAX), None, 0));

    let (_, fd) = parse_header(&mut FrameReader::new(&input[..])).expect("parse_header");
    let content_size = fd.content_size.expect("content_size present");
    assert_eq!(content_size.value, u64::MAX, "content_size.value");
    assert_eq!(content_size.hex, "ffffffffffffffff", "content_size.hex");
}

// --- FLG bit decomposition ---------------------------------------------

/// Exhaustively checks all 32 combinations of FLG's five independent boolean
/// fields (block independence, block checksum, content size present, content
/// checksum, dictionary ID present), rather than the handful of combinations
/// `parse_flg_fields` spot-checks.
#[test]
fn parse_flg_all_bit_combinations() {
    for bits in 0..32u32 {
        let block_independence = bits & (1 << 4) != 0;
        let block_checksum = bits & (1 << 3) != 0;
        let content_size_present = bits & (1 << 2) != 0;
        let content_checksum = bits & (1 << 1) != 0;
        let dictionary_id_present = bits & 1 != 0;

        let b = flg_byte(
            1,
            block_independence,
            block_checksum,
            content_size_present,
            content_checksum,
            0,
            dictionary_id_present,
        );
        let flg = parse_flg(&mut FrameReader::new(&[b][..])).unwrap_or_else(|e| panic!("bits {bits:#07b}: {e}"));
        assert_eq!(flg.block_independence, block_independence, "bits {bits:#07b}: block_independence");
        assert_eq!(flg.block_checksum, block_checksum, "bits {bits:#07b}: block_checksum");
        assert_eq!(flg.content_size_present, content_size_present, "bits {bits:#07b}: content_size_present");
        assert_eq!(flg.content_checksum, content_checksum, "bits {bits:#07b}: content_checksum");
        assert_eq!(flg.dictionary_id_present, dictionary_id_present, "bits {bits:#07b}: dictionary_id_present");
    }
}

#[test]
fn parse_flg_fields() {
    // (name, block_independence, block_checksum, content_size_present, content_checksum, dictionary_id_present)
    let cases: [(&str, bool, bool, bool, bool, bool); 7] = [
        ("all clear", false, false, false, false, false),
        ("all set", true, true, true, true, true),
        ("block independence only", true, false, false, false, false),
        ("block checksum only", false, true, false, false, false),
        ("content size only", false, false, true, false, false),
        ("content checksum only", false, false, false, true, false),
        ("dictionary id only", false, false, false, false, true),
    ];

    for (name, bi, bc, csp, cc, dip) in cases {
        let b = flg_byte(1, bi, bc, csp, cc, 0, dip);
        let flg = parse_flg(&mut FrameReader::new(&[b][..])).unwrap_or_else(|e| panic!("{name}: parse_flg: {e}"));
        assert_eq!(flg.version, 1, "{name}: version");
        assert_eq!(flg.block_independence, bi, "{name}: block_independence");
        assert_eq!(flg.block_checksum, bc, "{name}: block_checksum");
        assert_eq!(flg.content_size_present, csp, "{name}: content_size_present");
        assert_eq!(flg.content_checksum, cc, "{name}: content_checksum");
        assert_eq!(flg.reserved_bit, 0, "{name}: reserved_bit");
        assert_eq!(flg.dictionary_id_present, dip, "{name}: dictionary_id_present");
    }
}

#[test]
fn parse_flg_rejects_unsupported_version() {
    for version in [0u8, 2, 3] {
        let b = flg_byte(version, false, false, false, false, 0, false);
        let err = parse_flg(&mut FrameReader::new(&[b][..])).expect_err(&format!("version {version}: expected error"));
        assert_parse_error_at(&err, 0, "frame_descriptor.flg", &format!("version {version}"));
    }
}

#[test]
fn parse_flg_rejects_reserved_bit() {
    let b = flg_byte(1, false, false, false, false, 1, false);
    let err = parse_flg(&mut FrameReader::new(&[b][..])).expect_err("expected error");
    assert_parse_error_at(&err, 0, "frame_descriptor.flg", "reserved bit");
}

// --- BD bit decomposition -----------------------------------------------

#[test]
fn parse_bd_block_max_size_codes() {
    let cases: [(u8, u32); 4] = [(4, 64 * 1024), (5, 256 * 1024), (6, 1024 * 1024), (7, 4 * 1024 * 1024)];

    for (code, bytes) in cases {
        let b = bd_byte(0, code, 0);
        let bd = parse_bd(&mut FrameReader::new(&[b][..])).unwrap_or_else(|e| panic!("code {code}: parse_bd: {e}"));
        assert_eq!(bd.block_max_size_code, code, "code {code}: block_max_size_code");
        assert_eq!(bd.block_max_size_bytes, bytes, "code {code}: block_max_size_bytes");
        assert!(bd.reserved_bit_7 == 0 && bd.reserved_bits_3_0 == 0, "code {code}: reserved bits not zero: {bd:?}");
    }
}

#[test]
fn parse_bd_rejects_undefined_block_max_size_code() {
    for code in [0u8, 1, 2, 3] {
        let b = bd_byte(0, code, 0);
        let err = parse_bd(&mut FrameReader::new(&[b][..])).expect_err(&format!("code {code}: expected error"));
        assert_parse_error_at(&err, 0, "frame_descriptor.bd", &format!("code {code}"));
    }
}

#[test]
fn parse_bd_rejects_reserved_bits() {
    for (name, b) in [("bit 7", bd_byte(1, 4, 0)), ("bits 3-0", bd_byte(0, 4, 0x0F))] {
        let err = parse_bd(&mut FrameReader::new(&[b][..])).expect_err(&format!("{name}: expected error"));
        assert_parse_error_at(&err, 0, "frame_descriptor.bd", name);
    }
}

/// Decodes literal FLG byte values written directly in the test, rather than
/// through the `flg_byte` encoder helper used elsewhere: `flg_byte` mirrors
/// `parse_flg`'s own bit masks and shifts, so a shared misunderstanding
/// between the two could pass every case built through it without ever being
/// wrong.
#[test]
fn parse_flg_literal_bytes() {
    // (raw, block_independence, block_checksum, content_size_present, content_checksum, dictionary_id_present)
    let cases: [(u8, bool, bool, bool, bool, bool); 6] = [
        // 0x7c = 0b01111100, the documented JSON example's FLG.
        (0x7c, true, true, true, true, false),
        // 0x44 = 0b01000100: version 1, only content_checksum set.
        (0x44, false, false, false, true, false),
        // 0x41 = 0b01000001: version 1, only dictionary_id_present set.
        (0x41, false, false, false, false, true),
        // The next three cases pin bits 5, 4, and 3 individually: 0x7c above
        // is the only other case that exercises them, and it sets all three
        // at once, so a bug that permuted which named field each bit maps to
        // (in both parse_flg and flg_byte alike) would still pass every other
        // case in this table.
        // 0x60 = 0b01100000: version 1, only block_independence set.
        (0x60, true, false, false, false, false),
        // 0x50 = 0b01010000: version 1, only block_checksum set.
        (0x50, false, true, false, false, false),
        // 0x48 = 0b01001000: version 1, only content_size_present set.
        (0x48, false, false, true, false, false),
    ];

    for (raw, bi, bc, csp, cc, dip) in cases {
        let mut got = parse_flg(&mut FrameReader::new(&[raw][..])).unwrap_or_else(|e| panic!("raw {raw:#x}: {e}"));
        got.field = NumericField { offset: 0, value: 0, hex: String::new() };
        let want = Flg {
            field: NumericField { offset: 0, value: 0, hex: String::new() },
            version: 1,
            block_independence: bi,
            block_checksum: bc,
            content_size_present: csp,
            content_checksum: cc,
            reserved_bit: 0,
            dictionary_id_present: dip,
        };
        assert_eq!(got, want, "raw {raw:#x}: FLG");
    }
}

/// Decodes literal BD byte values written directly in the test, rather than
/// through the `bd_byte` encoder helper used elsewhere, for the same reason as
/// `parse_flg_literal_bytes`.
#[test]
fn parse_bd_literal_bytes() {
    let cases: [(u8, u8, u32); 2] = [
        (0x40, 4, 64 * 1024), // documented example
        (0x70, 7, 4 * 1024 * 1024),
    ];

    for (raw, want_code, want_max_size_bytes) in cases {
        let bd = parse_bd(&mut FrameReader::new(&[raw][..])).unwrap_or_else(|e| panic!("raw {raw:#x}: {e}"));
        assert!(
            bd.block_max_size_code == want_code && bd.block_max_size_bytes == want_max_size_bytes,
            "raw {raw:#x}: BD = {bd:?}, want code {want_code} bytes {want_max_size_bytes}"
        );
        assert!(bd.reserved_bit_7 == 0 && bd.reserved_bits_3_0 == 0, "raw {raw:#x}: reserved bits not zero: {bd:?}");
    }
}

// --- truncated input -----------------------------------------------------

/// Confirms every field that can be cut short reports a `ParseError` naming
/// the field, its offset, and the expected vs. actual byte counts, at every
/// possible cut point (not just "missing entirely" and one arbitrary partial
/// length): a position-specific short-read bug could otherwise pass with only
/// a couple of sampled cut points.
#[test]
fn parse_frame_descriptor_truncated_fields() {
    let flg = flg_byte(1, true, false, true, false, 0, true);
    let bd = bd_byte(0, 4, 0);
    let full = header_input(&descriptor_fixture(flg, bd, Some(3), Some(0xAABBCCDD), 0x2a));
    assert_eq!(full.len(), 19, "fixture length");

    let fields: [(&str, u64, usize); 6] = [
        ("magic_number", 0, 4),
        ("frame_descriptor.flg", 4, 1),
        ("frame_descriptor.bd", 5, 1),
        ("frame_descriptor.content_size", 6, 8),
        ("frame_descriptor.dictionary_id", 14, 4),
        ("frame_descriptor.header_checksum", 18, 1),
    ];

    for (path, offset, length) in fields {
        for read in 0..length {
            let label = format!("{path} cut at {read} of {length} bytes");
            let input = &full[..offset as usize + read];
            let err = parse_header(&mut FrameReader::new(input)).expect_err(&format!("{label}: expected error"));

            assert!(
                err.path == path && err.offset == offset,
                "{label}: err = {err}, want path {path:?} offset {offset}"
            );
            match err.cause {
                ParseCause::ShortRead { expected, actual } => assert!(
                    expected == length as u64 && actual == read as u64,
                    "{label}: ShortRead = {{expected:{expected} actual:{actual}}}, want {{expected:{length} actual:{read}}}"
                ),
                other => panic!("{label}: cause = {other:?}, want ShortRead"),
            }
        }
    }
}
