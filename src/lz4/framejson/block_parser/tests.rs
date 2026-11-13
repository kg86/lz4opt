//! Tests for the Block Format parser, driving `BlockReader`,
//! `parse_sequence` and `parse_sequence_loop` directly over in-memory
//! readers.

use std::io::Cursor;

use super::*;

/// The absolute offset at which `offset_block` places a block, standing in
/// for prior frame content: a nonzero base distinguishes an absolute stream
/// offset from a block-relative one.
pub(super) const BLOCK_OFFSET_BASE: u64 = 100;

/// Stands in for the Block Checksum that follows a compressed block's data,
/// so a parser that overreads consumes real, distinguishable bytes.
pub(super) const BLOCK_TRAILER: [u8; 4] = [0xAA, 0xBB, 0xCC, 0xDD];

/// Returns a frame reader over `data` followed by `BLOCK_TRAILER`, positioned
/// at `BLOCK_OFFSET_BASE`.
fn offset_block(data: &[u8]) -> FrameReader<Cursor<Vec<u8>>> {
    let mut full = data.to_vec();
    full.extend_from_slice(&BLOCK_TRAILER);
    let mut fr = FrameReader::new(Cursor::new(full));
    fr.offset = BLOCK_OFFSET_BASE;
    fr
}

/// Returns a frame reader over `data` positioned at offset 0.
fn plain_reader(data: &[u8]) -> FrameReader<Cursor<Vec<u8>>> {
    FrameReader::new(Cursor::new(data.to_vec()))
}

/// Checks that parsing stopped exactly at the block's declared end (offset
/// `BLOCK_OFFSET_BASE + data_size`, never beyond) and that the trailer
/// stand-in is still there, unread.
fn assert_trailer_untouched(fr: &mut FrameReader<Cursor<Vec<u8>>>, data_size: u64) {
    let want = BLOCK_OFFSET_BASE + data_size;
    assert_eq!(fr.offset, want, "fr.offset must be exactly the block's declared end, trailer untouched");
    let (tail, _) = fr.read_exact(BLOCK_TRAILER.len(), "check").expect("reading the trailer stand-in");
    assert_eq!(tail, BLOCK_TRAILER, "trailer bytes must be untouched by block parsing");
}

/// Returns a Frame Descriptor BD whose only meaningful field is the Block
/// Maximum Size.
fn bd_with_max_size(max: u32) -> Bd {
    Bd {
        field: NumericField::new(0, 0, &[0]),
        reserved_bit_7: 0,
        block_max_size_code: 0,
        block_max_size_bytes: max,
        reserved_bits_3_0: 0,
    }
}

/// Runs `parse_sequence_loop` over `data` placed at `BLOCK_OFFSET_BASE` and
/// checks that the trailer stand-in is left untouched whether it succeeds or
/// fails. Returns the result and the reader's remaining byte count.
fn run_loop(data: &[u8], max: u32) -> (Result<Vec<Sequence>, ParseError>, u64) {
    let mut fr = offset_block(data);
    let mut br = BlockReader::new(&mut fr, data.len() as u64, 0);
    let result = parse_sequence_loop(&mut br, &bd_with_max_size(max));
    let remaining = br.remaining_bytes();
    assert_trailer_untouched(&mut fr, data.len() as u64);
    (result, remaining)
}

/// Like `run_loop`, but requires an error.
fn run_loop_err(data: &[u8], max: u32) -> ParseError {
    match run_loop(data, max).0 {
        Ok(seqs) => panic!("parse_sequence_loop({data:02x?}) = {seqs:?}, want an error"),
        Err(err) => err,
    }
}

/// Joins every sequence's physical fields' hex in encoded order.
fn concat_sequence_hex(sequences: &[Sequence]) -> String {
    let mut out = String::new();
    for seq in sequences {
        out.push_str(&seq.token.hex);
        if let Some(e) = &seq.literal_length_extension {
            out.push_str(&e.hex);
        }
        out.push_str(&seq.literals.hex);
        if let Some(m) = &seq.match_offset {
            out.push_str(&m.hex);
        }
        if let Some(e) = &seq.match_length_extension {
            out.push_str(&e.hex);
        }
    }
    out
}

// BlockReader boundary behavior

#[test]
fn block_reader_rejects_reads_past_remaining() {
    let tail = [0xAA, 0xBB, 0xCC, 0xDD]; // stands in for Block Checksum bytes
    let mut fr = plain_reader(&[&[0x01, 0x02, 0x03][..], &tail].concat());
    let mut br = BlockReader::new(&mut fr, 3, 0);

    let err = br.read_bytes(4, "token").expect_err("read_bytes(4) over 3 remaining");
    assert_eq!(err.offset, 0, "boundary error offset: current cursor, no read attempted");
    assert_eq!(err.path, "data_blocks[0].data[0].token");

    // The underlying reader must be untouched: a direct read now returns the
    // block's own 3 bytes, not the tail.
    assert_eq!(fr.offset, 0, "fr.offset after the rejected read");
    let (raw, _) = fr.read_exact(3, "check").expect("read_exact after the rejected boundary read");
    assert_eq!(raw, [0x01, 0x02, 0x03], "bytes after the rejected read");
}

#[test]
fn block_reader_read_literal_rejects_past_remaining() {
    let mut fr = plain_reader(&[0x01, 0x02, 0x03, 0x04, 0x05]);
    let mut br = BlockReader::new(&mut fr, 2, 0);

    assert!(br.read_literal(3, "literals").is_err(), "read_literal(3) over 2 remaining must fail");
    assert_eq!(fr.offset, 0, "fr.offset after the rejected read_literal");
}

#[test]
fn block_reader_mid_stream_eof() {
    let mut fr = plain_reader(&[0x01, 0x02]);
    let mut br = BlockReader::new(&mut fr, 5, 2); // declares 5 bytes of block data, only 2 exist
    br.set_sequence_index(1);

    let err = br.read_literal(5, "literals").expect_err("read_literal across mid-stream EOF");
    assert_eq!(err.offset, 0, "start of the truncated read");
    assert_eq!(err.path, "data_blocks[2].data[1].literals");
    match err.cause {
        ParseCause::ShortRead { expected, actual } => assert_eq!((expected, actual), (5, 2), "short read counts"),
        other => panic!("cause = {other:?}, want ShortRead"),
    }
}

#[test]
fn block_reader_only_block_reads_advance_offset() {
    let mut fr = plain_reader(&[0xAA, 0xBB, 0xCC]);
    fr.offset = 100; // simulate having already read 100 bytes of prior frame content
    let mut br = BlockReader::new(&mut fr, 3, 0);

    let (b, offset) = br.read_byte("token").expect("read_byte");
    assert_eq!((b, offset), (0xAA, 100), "read_byte");
    assert_eq!(br.remaining_bytes(), 2, "remaining_bytes()");
    assert_eq!(fr.offset, 101, "fr.offset after read_byte");
}

#[test]
fn block_reader_remaining_tracks_partial_read_on_error() {
    let mut fr = plain_reader(&[0x01, 0x02]); // only 2 bytes exist
    let mut br = BlockReader::new(&mut fr, 5, 0); // block declares 5

    assert!(br.read_literal(5, "literals").is_err(), "read_literal across mid-stream EOF must fail");
    assert_eq!(br.remaining_bytes(), 3, "remaining after a partial read: 5 declared - 2 actually consumed");
    assert_eq!(br.remaining, 3, "remaining field");
}

// read_literal: chunk-boundary correctness

#[test]
fn block_reader_read_literal_across_chunk_boundary() {
    // The fixed copy buffer is 32 KiB.
    let n = 32 * 1024 + 10;
    // Varied, deterministic, mostly non-ASCII bytes.
    let data: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();

    let mut fr = plain_reader(&data);
    let mut br = BlockReader::new(&mut fr, n as u64, 0);

    let (hex, display, offset) = br.read_literal(n as u64, "literals").expect("read_literal");
    assert_eq!(offset, 0, "offset");
    assert_eq!(hex, hex_string(&data), "hex across the chunk boundary");
    assert_eq!(display, display_string(&data), "display across the chunk boundary");
    assert_eq!(br.remaining_bytes(), 0, "remaining_bytes()");
    assert_eq!(fr.offset, n as u64, "fr.offset");
}

#[test]
fn block_reader_read_literal_size_zero() {
    let mut fr = plain_reader(&[0x01]);
    let mut br = BlockReader::new(&mut fr, 1, 0);

    let (hex, display, offset) = br.read_literal(0, "literals").expect("read_literal(0)");
    assert_eq!((hex.as_str(), display.as_str()), ("", ""), "read_literal(0) must return empty strings");
    assert_eq!(offset, 0, "offset");
    assert_eq!(br.remaining_bytes(), 1, "remaining_bytes() changed by a size-0 read");
    assert_eq!(fr.offset, 0, "fr.offset changed by a size-0 read");
}

// Variable-length length extension helper

#[test]
fn block_reader_read_length_extension_nibble_below_15() {
    let mut fr = plain_reader(&[0xFF]); // must not be read
    let mut br = BlockReader::new(&mut fr, 1, 0);

    let ext = br.read_length_extension(14, "literal_length_extension").expect("read_length_extension(14)");
    assert!(ext.is_none(), "nibble < 15 must yield no extension segment");
    assert_eq!(br.remaining_bytes(), 1, "nibble < 15 must not read");
    assert_eq!(fr.offset, 0, "nibble < 15 must not read");
}

#[test]
fn block_reader_read_length_extension_cases() {
    let tests: [(&str, &[u8], u64, u64, &str); 4] = [
        ("terminator zero", &[0x00], 1, 0, "00"),
        ("single below 255", &[0x21], 1, 0x21, "21"),
        ("one continuation then terminator", &[0xFF, 0x0A], 2, 265, "ff0a"),
        ("multiple continuations", &[0xFF, 0xFF, 0xFF, 0x00], 4, 765, "ffffff00"),
    ];
    for (name, bytes, want_size, want_add, want_hex) in tests {
        let mut fr = plain_reader(bytes);
        let mut br = BlockReader::new(&mut fr, bytes.len() as u64, 3);
        br.set_sequence_index(2);

        let ext = br
            .read_length_extension(15, "match_length_extension")
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .unwrap_or_else(|| panic!("{name}: no result"));
        assert_eq!(ext.offset, 0, "{name}: offset");
        assert_eq!(ext.size, want_size, "{name}: size");
        assert_eq!(ext.hex, want_hex, "{name}: hex");
        assert_eq!(ext.additional_length, want_add, "{name}: additional_length");
        assert_eq!(br.remaining_bytes(), 0, "{name}: all extension bytes consumed");
        assert_eq!(fr.offset, bytes.len() as u64, "{name}: fr.offset");
    }
}

#[test]
fn block_reader_read_length_extension_unterminated_at_block_end() {
    let tail = [0x00, 0x00]; // must not be consumed by the failed extension
    let mut fr = plain_reader(&[0xFF, 0xFF, 0xFF, 0x00, 0x00]);
    let mut br = BlockReader::new(&mut fr, 3, 1);
    br.set_sequence_index(4);

    let err = br.read_length_extension(15, "literal_length_extension").err().expect("all-0xff run to block end");
    assert_eq!(err.path, "data_blocks[1].data[4].literal_length_extension");
    assert!(err.to_string().contains("not terminated"), "error = {err}");
    // All 3 declared block-data bytes were consumed but no more.
    assert_eq!(fr.offset, 3, "only the block's own bytes read");
    let (raw, _) = fr.read_exact(2, "tail-check").expect("read_exact tail");
    assert_eq!(raw, tail, "tail bytes must be untouched");
}

#[test]
fn block_reader_read_length_extension_truncated_before_any_byte() {
    let mut fr = plain_reader(&[]);
    let mut br = BlockReader::new(&mut fr, 0, 0);

    let err = br.read_length_extension(15, "match_length_extension").err().expect("remaining = 0");
    assert!(!err.to_string().contains("not terminated"), "error = {err}");
    assert!(err.to_string().contains("truncated"), "error = {err}");
}

#[test]
fn block_reader_read_length_extension_mid_stream_eof() {
    let mut fr = plain_reader(&[0xFF, 0xFF]); // stream ends after 2 bytes
    let mut br = BlockReader::new(&mut fr, 5, 0); // block declares 5

    let err = br.read_length_extension(15, "literal_length_extension").err().expect("mid-stream EOF");
    assert!(matches!(err.cause, ParseCause::ShortRead { .. }), "cause = {:?}, want ShortRead", err.cause);
    assert!(!err.to_string().contains("not terminated"), "error = {err}");
}

#[test]
fn block_reader_add_length_extension_byte_overflow() {
    assert!(add_length_extension_byte(u64::MAX, 0x01).is_err(), "(MAX, 1) must overflow");
    assert!(add_length_extension_byte(u64::MAX - 254, 0xFF).is_err(), "(MAX-254, 0xff) must overflow");
    // One below the boundary must still succeed.
    assert_eq!(add_length_extension_byte(u64::MAX - 255, 0xFF), Ok((u64::MAX, false)), "at the boundary");
}

// parse_sequence: token and literal decomposition

#[test]
fn parse_sequence_literal_length_codes() {
    struct Case {
        name: &'static str,
        token: u8,
        extension: &'static [u8],
        want_additional: u64,
        literal: Vec<u8>,
    }
    let cases = [
        Case { name: "code 0", token: 0x0f, extension: &[], want_additional: 0, literal: vec![] },
        Case { name: "code 14", token: 0xef, extension: &[], want_additional: 0, literal: vec![b'A'; 14] },
        Case {
            name: "code 15 with extension",
            token: 0xff,
            extension: &[0x0a],
            want_additional: 10,
            literal: vec![b'B'; 25],
        },
    ];
    for tc in cases {
        let mut data = vec![tc.token];
        data.extend_from_slice(tc.extension);
        data.extend_from_slice(&tc.literal);
        let mut fr = plain_reader(&data);
        let mut br = BlockReader::new(&mut fr, data.len() as u64, 0);

        let literal_len = tc.literal.len() as u64;
        let seq = parse_sequence(&mut br).unwrap_or_else(|e| panic!("{}: {e}", tc.name));
        assert_eq!(seq.literal_length, literal_len, "{}: literal_length", tc.name);
        assert_eq!(seq.literals.size, literal_len, "{}: literals size", tc.name);
        assert_eq!(seq.literals.hex.len() as u64 / 2, literal_len, "{}: literals hex", tc.name);
        if literal_len == 0 {
            assert_eq!(seq.literals.display, "", "{}: size-0 literals display", tc.name);
        }

        assert_eq!(seq.literal_length_extension.is_some(), !tc.extension.is_empty(), "{}: extension presence", tc.name);
        let mut extension_hex = String::new();
        if let Some(ext) = &seq.literal_length_extension {
            assert_eq!(ext.kind, SEGMENT_TYPE_LITERAL_LENGTH_EXTENSION, "{}: ext kind", tc.name);
            assert_eq!(ext.offset, seq.token.offset + seq.token.size, "{}: ext offset (right after token)", tc.name);
            assert_eq!(ext.size, tc.extension.len() as u64, "{}: ext size", tc.name);
            assert_eq!(ext.hex, hex_string(tc.extension), "{}: ext hex", tc.name);
            assert_eq!(ext.additional_length, tc.want_additional, "{}: ext additional_length", tc.name);
            extension_hex = ext.hex.clone();
        }

        assert_eq!(
            format!("{}{}{}", seq.token.hex, extension_hex, seq.literals.hex),
            hex_string(&data),
            "{}: concatenated child hex must be the whole fixture",
            tc.name
        );
        assert!(
            seq.match_length.is_none() && seq.match_offset.is_none() && seq.match_length_extension.is_none(),
            "{}: literal-only sequence has match fields: {seq:?}",
            tc.name
        );
        assert_eq!(br.remaining_bytes(), 0, "{}: remaining_bytes()", tc.name);
    }
}

#[test]
fn parse_sequence_match_length_codes() {
    struct Case {
        name: &'static str,
        token: u8,
        offset: [u8; 2],
        extra: &'static [u8],
        match_len: u64,
        want_off: u16,
        want_additional: u64,
    }
    let cases = [
        Case {
            name: "code 0",
            token: 0x00,
            offset: [0x01, 0x00],
            extra: &[],
            match_len: 4,
            want_off: 1,
            want_additional: 0,
        },
        Case {
            name: "code 14",
            token: 0x0e,
            offset: [0xff, 0xff],
            extra: &[],
            match_len: 18,
            want_off: 65535,
            want_additional: 0,
        },
        Case {
            name: "code 15 with extension",
            token: 0x0f,
            offset: [0x04, 0x00],
            extra: &[0x0a],
            match_len: 29,
            want_off: 4,
            want_additional: 10,
        },
        Case {
            name: "asymmetric offset bytes",
            token: 0x00,
            offset: [0x34, 0x12],
            extra: &[],
            match_len: 4,
            want_off: 0x1234,
            want_additional: 0,
        },
    ];
    for tc in cases {
        let mut data = vec![tc.token];
        data.extend_from_slice(&tc.offset);
        data.extend_from_slice(tc.extra);
        let mut fr = plain_reader(&data);
        let mut br = BlockReader::new(&mut fr, data.len() as u64, 0);

        let seq = parse_sequence(&mut br).unwrap_or_else(|e| panic!("{}: {e}", tc.name));
        assert_eq!(seq.match_length, Some(tc.match_len), "{}: match_length", tc.name);
        assert_eq!(seq.match_offset.as_ref().map(|m| m.value), Some(tc.want_off), "{}: match_offset", tc.name);
        assert_eq!(seq.match_length_extension.is_some(), !tc.extra.is_empty(), "{}: extension presence", tc.name);
        if let Some(ext) = &seq.match_length_extension {
            assert_eq!(ext.kind, SEGMENT_TYPE_MATCH_LENGTH_EXTENSION, "{}: ext kind", tc.name);
            assert_eq!(ext.size, tc.extra.len() as u64, "{}: ext size", tc.name);
            assert_eq!(ext.hex, hex_string(tc.extra), "{}: ext hex", tc.name);
            assert_eq!(ext.additional_length, tc.want_additional, "{}: ext additional_length", tc.name);
        }
    }
}

#[test]
fn parse_sequence_overlap_match_not_rejected() {
    let data = [0x0e, 0x01, 0x00]; // literal 0, match code 14 (len 18); offset 1
    let mut fr = plain_reader(&data);
    let mut br = BlockReader::new(&mut fr, data.len() as u64, 0);

    let seq = parse_sequence(&mut br).expect("parse_sequence");
    assert_eq!(seq.match_offset.as_ref().map(|m| m.value), Some(1), "match_offset");
    assert_eq!(seq.match_length, Some(18), "match_length (> offset 1, overlap must not be rejected)");
}

#[test]
fn parse_sequence_truncated_match_offset() {
    let data = [0x00, 0xAB]; // token (literal 0, match code 0), then 1 stray byte
    let mut fr = plain_reader(&data);
    let mut br = BlockReader::new(&mut fr, data.len() as u64, 0);

    let err = parse_sequence(&mut br).expect_err("truncated match offset");
    assert_eq!(err.path, "data_blocks[0].data[0].match_offset");
    assert_eq!(err.offset, 1, "cursor when the boundary check ran, no bytes read for match_offset");
    assert_eq!(br.fr.offset, 1, "only the token consumed");
    assert_eq!(br.remaining_bytes(), 1, "remaining must be untouched by the rejected read");
}

#[test]
fn parse_sequence_invalid_match_offset_zero() {
    let data = [0x0f, 0x00, 0x00, 0x05]; // token (literal 0, match code 15), offset 0, unread extension byte
    let mut fr = plain_reader(&data);
    let mut br = BlockReader::new(&mut fr, data.len() as u64, 0);

    let err = parse_sequence(&mut br).expect_err("match offset 0");
    assert_eq!(err.path, "data_blocks[0].data[0].match_offset");
    assert_eq!(err.offset, 1, "match_offset field start");
    // Rejection happens before the extension byte is read.
    assert_eq!(br.fr.offset, 3, "token+offset read, extension byte left untouched");
}

#[test]
fn parse_sequence_literal_length_exceeds_remaining() {
    let data = [0x50, b'a', b'b', b'c']; // literal length 5, but only 3 bytes follow
    let mut fr = plain_reader(&data);
    let mut br = BlockReader::new(&mut fr, data.len() as u64, 0);

    let err = parse_sequence(&mut br).expect_err("over-long literal length");
    assert_eq!(err.path, "data_blocks[0].data[0].literals");
    assert_eq!(err.offset, 1, "cursor when the boundary check ran, no literal bytes read");
    assert_eq!(br.fr.offset, 1, "only the token consumed");
    assert_eq!(br.remaining_bytes(), 3, "remaining must be untouched by the rejected read");
}

#[test]
fn parse_sequence_match_extension_then_last_literals() {
    let data = [0x4f, 0x61, 0x62, 0x63, 0x64, 0x04, 0x00, 0x00, 0x50, 0x65, 0x66, 0x67, 0x68, 0x69];
    let mut fr = plain_reader(&data);
    fr.offset = 19; // the same base offset as the first block's data in the README example
    let mut br = BlockReader::new(&mut fr, data.len() as u64, 0);

    let seq1 = parse_sequence(&mut br).expect("parse_sequence (first)");
    br.set_sequence_index(1);
    let seq2 = parse_sequence(&mut br).expect("parse_sequence (second)");
    assert_eq!(br.remaining_bytes(), 0, "remaining_bytes() after both sequences");

    // match_offset carries no "type" key.
    let want1 = serde_json::json!({
        "literal_length": 4,
        "match_length": 19,
        "token": {"type": "token", "offset": 19, "size": 1, "hex": "4f", "literal_length_code": 4, "match_length_code": 15},
        "literal_length_extension": null,
        "literals": {"type": "literals", "offset": 20, "size": 4, "hex": "61626364", "display": "abcd"},
        "match_offset": {"offset": 24, "value": 4, "hex": "0400"},
        "match_length_extension": {"type": "match_length_extension", "offset": 26, "size": 1, "hex": "00", "additional_length": 0}
    });
    let want2 = serde_json::json!({
        "literal_length": 5,
        "match_length": null,
        "token": {"type": "token", "offset": 27, "size": 1, "hex": "50", "literal_length_code": 5, "match_length_code": 0},
        "literal_length_extension": null,
        "literals": {"type": "literals", "offset": 28, "size": 5, "hex": "6566676869", "display": "efghi"},
        "match_offset": null,
        "match_length_extension": null
    });
    assert_eq!(serde_json::to_value(&seq1).unwrap(), want1, "sequence 1");
    assert_eq!(serde_json::to_value(&seq2).unwrap(), want2, "sequence 2");
}

#[test]
fn parse_sequence_effective_length_overflow() {
    let overflowing = LengthExtension { offset: 0, size: 1, hex: String::new(), additional_length: u64::MAX };
    assert!(effective_literal_length(1, Some(&overflowing)).is_err(), "literal length must overflow");
    assert!(effective_match_length(1, Some(&overflowing)).is_err(), "match length must overflow");
    assert_eq!(effective_literal_length(3, None), Ok(3), "effective_literal_length(3, None)");
    assert_eq!(effective_match_length(3, None), Ok(7), "effective_match_length(3, None)");
}

// parse_sequence_loop

#[test]
fn parse_sequence_loop_multiple_sequences_and_final_literal_only() {
    let data = [0x4f, 0x61, 0x62, 0x63, 0x64, 0x04, 0x00, 0x00, 0x50, 0x65, 0x66, 0x67, 0x68, 0x69];
    let (result, remaining) = run_loop(&data, 1000);
    let sequences = result.expect("parse_sequence_loop");
    assert_eq!(sequences.len(), 2, "sequence count");
    assert_eq!((sequences[0].literal_length, sequences[0].match_length), (4, Some(19)), "sequences[0]");
    assert_eq!((sequences[1].literal_length, sequences[1].match_length), (5, None), "sequences[1]");
    assert_eq!(sequences[0].token.offset, BLOCK_OFFSET_BASE, "absolute, not block-relative");
    assert_eq!(remaining, 0, "remaining_bytes()");
    assert_eq!(concat_sequence_hex(&sequences), hex_string(&data), "physical fields must concatenate to the input");
}

#[test]
fn parse_sequence_loop_single_zero_token() {
    let (result, _) = run_loop(&[0x00], 1000);
    let sequences = result.expect("parse_sequence_loop");
    assert_eq!(sequences.len(), 1, "sequence count");
    let seq = &sequences[0];
    assert_eq!((seq.literal_length, seq.literals.size, seq.literals.display.as_str()), (0, 0, ""), "literals");
    assert!(
        seq.match_length.is_none() && seq.match_offset.is_none() && seq.match_length_extension.is_none(),
        "{seq:?}"
    );
}

#[test]
fn parse_sequence_loop_final_token_non_zero_low_nibble_ignored() {
    // Literal code 5, match code 7 (nonzero, must be ignored).
    let mut data = vec![0x57];
    data.extend_from_slice(b"HELLO");
    let (result, remaining) = run_loop(&data, 1000);
    let sequences = result.expect("parse_sequence_loop");
    assert_eq!(sequences.len(), 1, "sequence count");
    let seq = &sequences[0];
    assert_eq!(seq.token.match_length_code, 7, "decoded but unused");
    assert!(
        seq.match_length.is_none() && seq.match_offset.is_none() && seq.match_length_extension.is_none(),
        "{seq:?}"
    );
    assert_eq!(remaining, 0, "remaining_bytes()");
}

#[test]
fn parse_sequence_loop_expanded_size_block_max_size_boundary() {
    // Literal code 5 and a single literal-only sequence "abcde": expanded
    // size 5.
    let mut data = vec![0x50];
    data.extend_from_slice(b"abcde");

    // Exact boundary accepted.
    let sequences = run_loop(&data, 5).0.expect("at the exact Block Maximum Size");
    assert!(sequences.len() == 1 && sequences[0].literal_length == 5, "sequences = {sequences:?}");

    // Exceeded by 1 rejected.
    let err = run_loop_err(&data, 4);
    assert!(err.to_string().contains("Block Maximum Size"), "error = {err}");
    // literal_end = token(1) + literal(5), at the block base.
    assert_eq!(err.offset, BLOCK_OFFSET_BASE + 6, "absolute, right after the literal");
}

#[test]
fn parse_sequence_loop_rejects_match_at_block_end() {
    // Literal code 0, match code 4 (length 8); no literal bytes; match offset
    // 1; the block ends right there.
    let data = [0x04, 0x01, 0x00];
    let err = run_loop_err(&data, 100);
    assert_eq!(err.path, "data_blocks[0].data[0].match_offset");
    assert!(err.to_string().contains("no final literal-only sequence"), "error = {err}");
    assert_eq!(err.offset, BLOCK_OFFSET_BASE + data.len() as u64, "whole block consumed before detection");
}

#[test]
fn parse_sequence_loop_rejects_last_literal_under_5() {
    // seq0: literal code 5, match code 6 (length 10), literal "ABCDE", offset
    // 1; seq1 (final): literal code 2, "XY". Expanded size 17, the match
    // starts 12 bytes before the end, so only the final-literal condition
    // fails.
    let data = [0x56, b'A', b'B', b'C', b'D', b'E', 0x01, 0x00, 0x20, b'X', b'Y'];
    let err = run_loop_err(&data, 100);
    assert_eq!(err.path, "data_blocks[0].data[1].literals");
    // seq1's literals start after seq0 (8 bytes) and seq1's token.
    assert_eq!(err.offset, BLOCK_OFFSET_BASE + 9, "seq1's literals field");
}

#[test]
fn parse_sequence_loop_rejects_match_too_close_to_end() {
    // seq0: match length 4, offset 1; seq1: literal "PQRST". Expanded size
    // 9, the match starts 9 bytes before the end (< 12).
    let data = [0x00, 0x01, 0x00, 0x50, b'P', b'Q', b'R', b'S', b'T'];
    let err = run_loop_err(&data, 100);
    assert_eq!(err.path, "data_blocks[0].data[0].match_offset");
    assert_eq!(err.offset, BLOCK_OFFSET_BASE + 1, "seq0's match_offset field");
}

#[test]
fn parse_sequence_loop_rejects_multiple_sequences_under_min_expanded_size() {
    // seq0: match length 4, offset 1; seq1: empty. Expanded size 4 (< 5)
    // reached through 2 sequences.
    let data = [0x00, 0x01, 0x00, 0x00];
    let err = run_loop_err(&data, 100);
    assert!(err.to_string().contains("exactly 1 sequence"), "error = {err}");
    assert_eq!(err.offset, BLOCK_OFFSET_BASE + 3, "seq1's token");
}

#[test]
fn verify_sequence_coverage_detects_gap_and_overlap() {
    let mut fr = plain_reader(&[]);
    let br = BlockReader::new(&mut fr, 10, 0);

    let sequence = |literals: LiteralsSegment| Sequence {
        literal_length: 0,
        match_length: None,
        token: TokenSegment::new(0, 0x00),
        literal_length_extension: None,
        literals,
        match_offset: None,
        match_length_extension: None,
    };

    // The literals should be at offset 1, not 5.
    let gap = [sequence(LiteralsSegment::new(5, String::new(), String::new()))];
    let err = verify_sequence_coverage(&br, &gap, 0, 5).expect_err("a gap");
    assert!(err.to_string().contains("segment coverage broken"), "gap: error = {err}");

    // The literals should be at offset 1, not 0: they overlap the token.
    let overlap = [sequence(LiteralsSegment::new(0, "aabb".to_string(), "\\xAA\\xBB".to_string()))];
    let err = verify_sequence_coverage(&br, &overlap, 0, 3).expect_err("an overlap");
    assert!(err.to_string().contains("segment coverage broken"), "overlap: error = {err}");

    // Declared data size is 10, but the fields only cover 3 bytes.
    let short = [sequence(LiteralsSegment::new(1, "aabb".to_string(), "\\xAA\\xBB".to_string()))];
    let err = verify_sequence_coverage(&br, &short, 0, 10).expect_err("a total size mismatch");
    assert!(err.to_string().contains("segment coverage totals"), "total mismatch: error = {err}");

    // A consistent layout passes.
    let ok = [sequence(LiteralsSegment::new(1, "aabb".to_string(), "\\xAA\\xBB".to_string()))];
    assert!(verify_sequence_coverage(&br, &ok, 0, 3).is_ok(), "a contiguous layout must pass");
}
