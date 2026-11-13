use std::io::{self, Read};

use super::test_fixtures::{
    bd_byte, block_size_bytes, descriptor_fixture, end_mark_bytes, flg_byte, minimal_normal_frame_bytes,
    skippable_frame_bytes, u32_le, valid_magic_bytes,
};
use super::*;

// --- fixture builders ---

/// Decodes and concatenates hex strings.
fn hex_concat(parts: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        assert!(p.len() % 2 == 0, "odd-length hex {p:?}");
        for i in (0..p.len()).step_by(2) {
            out.push(u8::from_str_radix(&p[i..i + 2], 16).unwrap_or_else(|e| panic!("bad hex {p:?}: {e}")));
        }
    }
    out
}

/// Reader that yields its data and then fails with an `io::Error`, so any
/// `Err` must be reported as an I/O failure rather than swallowed as the end
/// of the stream, since Rust's `Read` reports a clean EOF as `Ok(0)`.
struct ErrAfterDataReader {
    data: Vec<u8>,
    sent: bool,
}

impl Read for ErrAfterDataReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.sent {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short read"));
        }
        let n = self.data.len().min(buf.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.sent = true;
        Ok(n)
    }
}

/// Asserts that `err` is a `ParseError` at `offset` naming `path`.
fn assert_parse_error_at(err: &ParseError, offset: u64, path: &str, label: &str) {
    assert_eq!((err.offset, err.path.as_str()), (offset, path), "{label}: err = {err}");
}

/// Asserts that `err` is a short read of `expected` bytes with `actual` read.
fn assert_short_read(err: &ParseError, expected: u64, actual: u64, label: &str) {
    match err.cause {
        ParseCause::ShortRead { expected: e, actual: a } => {
            assert_eq!((e, a), (expected, actual), "{label}: err = {err}")
        }
        _ => panic!("{label}: err = {err}, want ShortRead{{expected:{expected} actual:{actual}}}"),
    }
}

// --- empty input, truncated/invalid magic ---

#[test]
fn parse_stream_rejects_empty_input() {
    let err = parse(&[][..]).expect_err("expected error, got Ok");
    assert_parse_error_at(&err, 0, "magic_number", "empty input");
}

#[test]
fn parse_stream_rejects_truncated_magic_number() {
    let full = u32_le(MAGIC_NUMBER_LZ4);
    for n in 1..=3usize {
        let label = format!("{n} bytes");
        let err = parse(&full[..n]).expect_err("expected error, got Ok");
        assert_parse_error_at(&err, 0, "magic_number", &label);
        assert_short_read(&err, 4, n as u64, &label);
    }
}

/// Confirms a Magic Number read that fails after returning fewer than 4 bytes
/// is reported as an I/O error at the Magic Number, not silently accepted as a
/// clean end of stream that drops the trailing bytes.
#[test]
fn parse_stream_reports_magic_number_read_error_after_short_read() {
    let r = ErrAfterDataReader { data: vec![0x04, 0x22], sent: false };

    let err = parse(r).expect_err("expected error, got Ok");
    assert_parse_error_at(&err, 0, "magic_number", "short read then error");
    match &err.cause {
        ParseCause::Io(e) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof, "err = {err}"),
        _ => panic!("err = {err}, want ParseCause::Io"),
    }
}

#[test]
fn parse_stream_rejects_truncated_magic_number_mid_stream() {
    let frame = minimal_normal_frame_bytes();
    let input = [frame.clone(), u32_le(MAGIC_NUMBER_LZ4)[..2].to_vec()].concat();

    let err = parse(&input[..]).expect_err("expected error, got Ok");
    assert_parse_error_at(&err, frame.len() as u64, "magic_number", "mid stream");
    assert_short_read(&err, 4, 2, "mid stream");
}

/// Confirms both a Legacy Frame magic and a magic number matching neither the
/// normal, Skippable, nor Legacy ranges are rejected identically whether they
/// appear as the first Frame or between two completed Frames, and that the two
/// cases produce distinguishable diagnostics.
#[test]
fn parse_stream_rejects_legacy_or_unknown_magic_number() {
    let frame = minimal_normal_frame_bytes();

    let cases: [(&str, u32, &str); 2] =
        [("legacy", MAGIC_NUMBER_LEGACY, "Legacy"), ("unknown", 0x12345678, "unknown magic number")];

    for (name, magic, want_error_sub) in cases {
        let label = format!("{name}/first frame");
        let err = parse(&u32_le(magic)[..]).expect_err("expected error, got Ok");
        assert_parse_error_at(&err, 0, "magic_number", &label);
        let text = err.to_string();
        assert!(text.contains(want_error_sub), "{label}: err = {text:?}, want it to contain {want_error_sub:?}");

        let label = format!("{name}/mid stream");
        let input = [frame.clone(), u32_le(magic)].concat();
        let err = parse(&input[..]).expect_err("expected error, got Ok");
        assert_parse_error_at(&err, frame.len() as u64, "magic_number", &label);
        let text = err.to_string();
        assert!(text.contains(want_error_sub), "{label}: err = {text:?}, want it to contain {want_error_sub:?}");
    }

    assert_ne!(cases[0].2, cases[1].2, "test setup: legacy and unknown cases must assert distinct diagnostics");
}

// --- Skippable Frame ---

#[test]
fn parse_skippable_frame_all_magic_numbers() {
    for magic in MAGIC_NUMBER_SKIPPABLE_MIN..=MAGIC_NUMBER_SKIPPABLE_MAX {
        let label = format!("{magic:#010x}");
        let data = [0xaa, 0xbb, 0xcc];
        let input = skippable_frame_bytes(magic, &data);

        let root = parse(&input[..]).unwrap_or_else(|e| panic!("{label}: parse: {e}"));
        assert_eq!(root.frames.len(), 1, "{label}: frames.len()");
        let Frame::Skippable(sf) = &root.frames[0] else {
            panic!("{label}: frame = {:?}, want Skippable", root.frames[0]);
        };
        assert_eq!((sf.magic_number.offset, sf.magic_number.value), (0, magic), "{label}: magic_number");
        assert_eq!(sf.frame_size.value, data.len() as u32, "{label}: frame_size.value");
        assert_eq!(sf.data.hex, hex_string(&data), "{label}: data.hex");
    }
}

#[test]
fn parse_skippable_frame_size_zero_shares_offset_with_next_magic() {
    let first = skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MIN, &[]);
    let second = skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MIN + 1, &[0x01]);
    let input = [first.clone(), second].concat();

    let root = parse(&input[..]).expect("parse");
    assert_eq!(root.frames.len(), 2, "frames.len()");

    let Frame::Skippable(sf0) = &root.frames[0] else { panic!("frame 0 is not skippable") };
    assert!(sf0.data.size == 0 && sf0.data.hex.is_empty(), "sf0.data = {:?}, want size 0 empty hex", sf0.data);
    let want_offset = first.len() as u64;
    assert_eq!(sf0.data.offset, want_offset, "sf0.data.offset");

    let Frame::Skippable(sf1) = &root.frames[1] else { panic!("frame 1 is not skippable") };
    assert_eq!(sf1.magic_number.offset, want_offset, "sf1.magic_number.offset (same as preceding size-0 data)");
}

#[test]
fn parse_skippable_frame_truncated_data() {
    let mut input = u32_le(MAGIC_NUMBER_SKIPPABLE_MIN);
    input.extend(u32_le(10)); // declares 10 bytes of data
    input.extend([0x01, 0x02, 0x03]);

    let err = parse(&input[..]).expect_err("expected error, got Ok");
    assert_parse_error_at(&err, 8, "data", "truncated data");
    assert_short_read(&err, 10, 3, "truncated data");
}

/// Confirms the Frame Size field itself (as opposed to the User Data it
/// declares) reports a `ParseError` naming its offset and expected/actual byte
/// counts at every cut point.
#[test]
fn parse_skippable_frame_size_truncated() {
    let full = [u32_le(MAGIC_NUMBER_SKIPPABLE_MIN), u32_le(5), vec![0x01, 0x02, 0x03, 0x04, 0x05]].concat();

    for n in 0..=3usize {
        let label = format!("{n} bytes");
        let err = parse(&full[..4 + n]).expect_err("expected error, got Ok");
        assert_parse_error_at(&err, 4, "frame_size", &label);
        assert_short_read(&err, 4, n as u64, &label);
    }
}

/// Size of the fixed chunk the parser copies Data Block content and Skippable
/// Frame User Data through. The parser's own constant is private to its
/// module, so this mirrors its value.
const DATA_CHUNK_SIZE: usize = 32 * 1024;

/// Wraps a reader and panics if any single `read` call requests a buffer
/// larger than the given bound. It proves the parser never issues a single
/// `read` call sized off a declared field value: only off its own fixed chunk
/// size, regardless of how large the declared total is.
struct MaxReadLenReader<R> {
    r: R,
    bound: usize,
}

impl<R: Read> Read for MaxReadLenReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        assert!(
            buf.len() <= self.bound,
            "read requested {} bytes, want <= {} (declared size must not drive a single large read call)",
            buf.len(),
            self.bound
        );
        self.r.read(buf)
    }
}

/// Builds a Skippable Frame declaring a Frame Size of `u32::MAX` (unlike Data
/// Block size, Frame Size has no format-level maximum that would let the
/// parser reject it structurally before reading) followed by only 3 actual
/// data bytes.
fn huge_declared_skippable_frame_body() -> (Vec<u8>, usize) {
    let actual = [0x01, 0x02, 0x03];
    let mut body = u32_le(MAGIC_NUMBER_SKIPPABLE_MIN);
    body.extend(u32_le(u32::MAX));
    body.extend(actual);
    (body, actual.len())
}

/// Confirms a huge declared Skippable Frame Size combined with a short actual
/// input fails with a precise short-read error, and does so without ever
/// asking the underlying reader to fill a buffer bigger than the chunk size.
#[test]
fn parse_skippable_frame_huge_declared_size_short_input_no_large_read_call() {
    let (body, actual_data_len) = huge_declared_skippable_frame_body();
    let guarded = MaxReadLenReader { r: &body[..], bound: DATA_CHUNK_SIZE };
    let err = parse(guarded).expect_err("expected error, got Ok");
    assert_parse_error_at(&err, 8, "data", "huge declared size");
    assert_short_read(&err, u32::MAX as u64, actual_data_len as u64, "huge declared size");
}

// Total allocation for a huge declared size with a short input is not
// measured, because that needs a counting global allocator for the whole test
// binary; the no-large-read-call test above covers the read-size half.

// --- Frame concatenation ---

#[test]
fn parse_stream_multiple_normal_frames() {
    let frame = minimal_normal_frame_bytes();
    let input = [frame.clone(), frame.clone(), frame.clone()].concat();

    let root = parse(&input[..]).expect("parse");
    assert_eq!(root.frames.len(), 3, "frames.len()");
    for (i, f) in root.frames.iter().enumerate() {
        let Frame::Normal(nf) = f else { panic!("frame {i} = {f:?}, want Normal") };
        let want_offset = (i * frame.len()) as u64;
        assert_eq!(nf.magic_number.offset, want_offset, "frame {i} magic_number.offset");
    }
}

#[test]
fn parse_stream_mixed_frame_types_preserves_order() {
    let normal = minimal_normal_frame_bytes();
    let skip1 = skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MIN, &[0x01, 0x02]);
    let skip2 = skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MAX, &[]);
    let input = [skip1, normal.clone(), skip2, normal].concat();

    let root = parse(&input[..]).expect("parse");
    let want_types = [FRAME_TYPE_SKIPPABLE, FRAME_TYPE_LZ4, FRAME_TYPE_SKIPPABLE, FRAME_TYPE_LZ4];
    assert_eq!(root.frames.len(), want_types.len(), "frames.len()");
    for (i, f) in root.frames.iter().enumerate() {
        let got_type = match f {
            Frame::Normal(_) => FRAME_TYPE_LZ4,
            Frame::Skippable(_) => FRAME_TYPE_SKIPPABLE,
        };
        assert_eq!(got_type, want_types[i], "frame {i} type");
    }
}

// --- physical field concatenation ---

/// Walks a parsed `Root` over its known model shape and returns every
/// physical field's `(offset, hex)` pair. It exists only to verify that the
/// parser accounts for every input byte.
fn collect_physical_field_hex(root: &Root) -> Vec<(u64, String)> {
    let mut out: Vec<(u64, String)> = Vec::new();
    let mut add = |offset: u64, hex: &str| out.push((offset, hex.to_string()));

    for f in &root.frames {
        match f {
            Frame::Normal(v) => {
                add(v.magic_number.offset, &v.magic_number.hex);
                let fd = &v.frame_descriptor;
                add(fd.flg.field.offset, &fd.flg.field.hex);
                add(fd.bd.field.offset, &fd.bd.field.hex);
                if let Some(cs) = &fd.content_size {
                    add(cs.offset, &cs.hex);
                }
                if let Some(did) = &fd.dictionary_id {
                    add(did.offset, &did.hex);
                }
                add(fd.header_checksum.offset, &fd.header_checksum.hex);
                for block in &v.data_blocks {
                    add(block.block_size.field.offset, &block.block_size.field.hex);
                    for seg in &block.data {
                        match seg {
                            BlockData::Uncompressed(s) => add(s.offset, &s.hex),
                            BlockData::Sequence(s) => {
                                add(s.token.offset, &s.token.hex);
                                if let Some(ext) = &s.literal_length_extension {
                                    add(ext.offset, &ext.hex);
                                }
                                add(s.literals.offset, &s.literals.hex);
                                if let Some(mo) = &s.match_offset {
                                    add(mo.offset, &mo.hex);
                                }
                                if let Some(ext) = &s.match_length_extension {
                                    add(ext.offset, &ext.hex);
                                }
                            }
                        }
                    }
                    if let Some(bc) = &block.block_checksum {
                        add(bc.offset, &bc.hex);
                    }
                }
                add(v.end_mark.offset, &v.end_mark.hex);
                if let Some(cc) = &v.content_checksum {
                    add(cc.offset, &cc.hex);
                }
            }
            Frame::Skippable(v) => {
                add(v.magic_number.offset, &v.magic_number.hex);
                add(v.frame_size.offset, &v.frame_size.hex);
                add(v.data.offset, &v.data.hex);
            }
        }
    }
    out
}

/// Builds a stream mixing a fully-populated normal Frame (Content Size,
/// Dictionary ID, two Data Blocks including a size-0 uncompressed block
/// sharing its offset with a following Block Checksum, and a Content
/// Checksum), a minimal normal Frame with every optional field absent, and two
/// Skippable Frames (one with size-0 data). It confirms every physical field's
/// offset lands exactly where the preceding fields' lengths place it (so a
/// systematic offset bug that still preserved sort order could not slip
/// through) and that concatenating every field's hex in offset order
/// reconstructs the input exactly.
#[test]
fn concatenated_physical_field_hex_matches_input() {
    let flg = flg_byte(1, true, true, true, true, 0, true);
    let bd = bd_byte(0, 4, 0);
    let descriptor = descriptor_fixture(flg, bd, Some(5), Some(0xdeadbeef), 0x2a);

    // Two sequences, the first carrying a match_offset and
    // match_length_extension, so the Sequence arm of the collector exercises
    // those child-field branches, not just token+literals.
    let block0_data = hex_concat(&["4f", "61626364", "0400", "00", "50", "6566676869"]);
    let full = [
        valid_magic_bytes(),
        descriptor,
        block_size_bytes(false, block0_data.len() as u32),
        block0_data,
        u32_le(0x12345678),        // block 0 checksum
        block_size_bytes(true, 0), // block 1: empty uncompressed block
        u32_le(0x9abcdef0),        // block 1 checksum, same offset as block 1's 0-length data
        end_mark_bytes(),
        u32_le(0xf0debc9a), // content checksum
    ]
    .concat();

    let minimal = minimal_normal_frame_bytes();
    let skip_zero = skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MIN, &[]);
    let skip_data = skippable_frame_bytes(MAGIC_NUMBER_SKIPPABLE_MAX, &[0xaa, 0xbb]);

    let input = [full, minimal, skip_zero, skip_data].concat();

    let root = parse(&input[..]).expect("parse");
    assert_eq!(root.frames.len(), 4, "frames.len()");

    let mut fields = collect_physical_field_hex(&root);
    fields.sort_by_key(|f| f.0); // stable

    let mut all_hex = String::new();
    let mut cursor = 0u64;
    for (offset, hex) in &fields {
        assert_eq!(
            *offset, cursor,
            "field at reported offset {offset}, want {cursor} (previous fields cover [0,{cursor}))"
        );
        cursor += hex.len() as u64 / 2;
        all_hex.push_str(hex);
    }
    assert_eq!(cursor, input.len() as u64, "fields cover [0,{cursor}), want [0,{}) (all of input)", input.len());

    let got = hex_concat(&[&all_hex]);
    assert_eq!(got, input, "reconstructed bytes");
}
