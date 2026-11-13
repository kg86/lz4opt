use super::super::test_fixtures::{block_size_bytes, end_mark_bytes, literal_only_block_bytes, u32_le};
use super::*;

// --- fixture builders ---------------------------------------------------

/// Builds a `Flg` carrying only the two checksum flags `parse_data_blocks`
/// reads; its own field and the remaining flags are never consulted there.
fn flg_with(block_checksum: bool, content_checksum: bool) -> Flg {
    Flg {
        field: NumericField { offset: 0, value: 0, hex: String::new() },
        version: 0,
        block_independence: false,
        block_checksum,
        content_size_present: false,
        content_checksum,
        reserved_bit: 0,
        dictionary_id_present: false,
    }
}

/// Builds a BD carrying only the Block Maximum Size that `decode_block_size`
/// validates against; `parse_data_blocks` never reads BD's own field, so
/// tests that exercise it directly can leave that zero.
fn bd_with_max_size(max: u32) -> Bd {
    Bd {
        field: NumericField { offset: 0, value: 0, hex: String::new() },
        reserved_bit_7: 0,
        block_max_size_code: 0,
        block_max_size_bytes: max,
        reserved_bits_3_0: 0,
    }
}

/// Asserts `d` holds a `Sequence` (a compressed block's `BlockData`).
fn as_sequence<'a>(d: &'a BlockData, label: &str) -> &'a Sequence {
    match d {
        BlockData::Sequence(seq) => seq,
        other => panic!("{label}: data segment = {other:?}, want Sequence"),
    }
}

/// Asserts `d` holds the `UncompressedDataSegment` `parse_data_blocks`
/// produces for an uncompressed block.
fn as_uncompressed_data_segment<'a>(d: &'a BlockData, label: &str) -> &'a UncompressedDataSegment {
    match d {
        BlockData::Uncompressed(seg) => seg,
        other => panic!("{label}: data segment = {other:?}, want UncompressedDataSegment"),
    }
}

/// The pieces of a successfully parsed frame tail.
type Tail = (Vec<DataBlock>, NumericField<u32>, Option<NumericField<u32>>);

/// Runs `parse_data_blocks` over `input` and returns the tail's pieces as the
/// 4-tuple (blocks, end mark, content checksum, reader offset) the tests
/// destructure.
fn parse_data_blocks_tuple(input: &[u8], flg: &Flg, bd: &Bd) -> (Result<Tail, ParseError>, u64) {
    let mut fr = FrameReader::new(input);
    let res = parse_data_blocks(&mut fr, flg, bd).map(|t| (t.data_blocks, t.end_mark, t.content_checksum));
    (res, fr.offset)
}

// --- compressed/uncompressed blocks, multiple blocks ---------------------

/// Confirms `parse_data_blocks` branches on `block_size.uncompressed`: a
/// compressed block's data decodes to a single-element sequence array
/// (delegated to the Block Format parser) while an uncompressed block's data
/// decodes to a single `UncompressedDataSegment` covering
/// `block_size.data_size`, with no checksums. The uncompressed data is chosen
/// to exercise the NUL, double-quote, and backslash display escapes alongside
/// a printable byte.
#[test]
fn parse_data_blocks_compressed_and_uncompressed() {
    let literal_payload = [0x61u8, 0x62, 0x63];
    let compressed_data = literal_only_block_bytes(&literal_payload);
    let uncompressed_data = [0x00u8, 0x22, 0x5c, 0x41];

    let mut input = Vec::new();
    input.extend(block_size_bytes(false, compressed_data.len() as u32));
    input.extend(&compressed_data);
    input.extend(block_size_bytes(true, uncompressed_data.len() as u32));
    input.extend(uncompressed_data);
    input.extend(end_mark_bytes());

    let (res, offset) = parse_data_blocks_tuple(&input, &flg_with(false, false), &bd_with_max_size(65536));
    let (blocks, end_mark, content_checksum) = res.expect("parse_data_blocks");
    assert!(content_checksum.is_none(), "content_checksum = {content_checksum:?}, want None");
    assert_eq!(blocks.len(), 2, "blocks.len()");

    let b0 = &blocks[0];
    assert!(
        b0.block_size.field.offset == 0
            && b0.block_size.data_size == compressed_data.len() as u32
            && !b0.block_size.uncompressed,
        "block 0 = {b0:?}"
    );
    assert_eq!(b0.data.len(), 1, "block 0 data = {:?}, want 1 sequence", b0.data);
    let seq0 = as_sequence(&b0.data[0], "block 0");
    assert!(
        seq0.token.offset == 4 && seq0.literals.offset == 5 && seq0.literals.hex == hex_string(&literal_payload),
        "block 0 sequence = {seq0:?}"
    );
    assert!(
        seq0.literal_length == literal_payload.len() as u64 && seq0.match_length.is_none(),
        "block 0 sequence lengths = literal_length {} match_length {:?}, want {} None",
        seq0.literal_length,
        seq0.match_length,
        literal_payload.len()
    );
    assert!(b0.block_checksum.is_none(), "block 0 checksum = {:?}, want None", b0.block_checksum);

    let want_b1_offset = (4 + compressed_data.len()) as u64;
    let b1 = &blocks[1];
    assert!(
        b1.block_size.field.offset == want_b1_offset
            && b1.block_size.data_size == uncompressed_data.len() as u32
            && b1.block_size.uncompressed,
        "block 1 = {b1:?}"
    );
    let seg1 = as_uncompressed_data_segment(&b1.data[0], "block 1");
    let want_seg1_offset = want_b1_offset + 4;
    assert!(
        b1.data.len() == 1
            && seg1.offset == want_seg1_offset
            && seg1.size == uncompressed_data.len() as u64
            && seg1.hex == hex_string(&uncompressed_data),
        "block 1 data = {:?}",
        b1.data
    );
    assert_eq!(seg1.display, r#"\x00\"\\A"#, "block 1 data display");

    let want_end_mark_offset = want_seg1_offset + uncompressed_data.len() as u64;
    assert!(
        end_mark.offset == want_end_mark_offset && end_mark.value == 0,
        "end_mark = {end_mark:?}, want offset {want_end_mark_offset} value 0"
    );
    assert_eq!(offset, input.len() as u64, "reader offset (all input consumed)");
}

/// Confirms a Frame with no Data Blocks (Block Size field immediately
/// 0x00000000) parses to an empty block list and an EndMark at offset 0, with
/// and without a following Content Checksum.
#[test]
fn parse_data_blocks_empty_frame_end_mark() {
    for content_checksum_present in [false, true] {
        let name = if content_checksum_present { "content checksum" } else { "no content checksum" };
        let mut input = end_mark_bytes();
        if content_checksum_present {
            input.extend(u32_le(0x9abcdef0));
        }

        let flg = flg_with(false, content_checksum_present);
        let (res, offset) = parse_data_blocks_tuple(&input, &flg, &bd_with_max_size(65536));
        let (blocks, end_mark, content_checksum) = res.unwrap_or_else(|e| panic!("{name}: parse_data_blocks: {e}"));
        assert_eq!(blocks.len(), 0, "{name}: blocks.len()");
        // An empty block list serialize to [], which the documented
        // schema requires for data_blocks.
        assert_eq!(serde_json::to_string(&blocks).expect("serialize blocks"), "[]", "{name}: blocks JSON");
        assert!(end_mark.offset == 0 && end_mark.value == 0, "{name}: end_mark = {end_mark:?}, want offset 0 value 0");
        if content_checksum_present {
            let cc = content_checksum.unwrap_or_else(|| panic!("{name}: content_checksum = None, want Some"));
            assert!(
                cc.offset == 4 && cc.value == 0x9abcdef0,
                "{name}: content_checksum = {cc:?}, want offset 4 value 0x9abcdef0"
            );
        } else {
            assert!(content_checksum.is_none(), "{name}: content_checksum = {content_checksum:?}, want None");
        }
        assert_eq!(offset, input.len() as u64, "{name}: reader offset");
    }
}

/// Confirms 0x00000000 ends the block loop with no block created, while
/// 0x80000000 (an empty uncompressed block) creates one block whose
/// zero-length data segment and following Block Checksum share the same
/// offset.
#[test]
fn parse_data_blocks_distinguishes_end_mark_from_empty_uncompressed_block() {
    // 0x00000000 is EndMark.
    let input = end_mark_bytes();
    let (res, _) = parse_data_blocks_tuple(&input, &flg_with(false, false), &bd_with_max_size(65536));
    let (blocks, end_mark, _) = res.expect("parse_data_blocks");
    assert_eq!(blocks.len(), 0, "EndMark: blocks.len()");
    assert_eq!(end_mark.value, 0, "EndMark: end_mark.value");

    // 0x80000000 is an empty uncompressed block sharing offset with block checksum.
    let mut input = Vec::new();
    input.extend(block_size_bytes(true, 0));
    input.extend(u32_le(0x12345678)); // block checksum
    input.extend(end_mark_bytes());

    let (res, _) = parse_data_blocks_tuple(&input, &flg_with(true, false), &bd_with_max_size(65536));
    let (blocks, end_mark, _) = res.expect("parse_data_blocks");
    assert_eq!(blocks.len(), 1, "empty uncompressed: blocks.len()");

    let b = &blocks[0];
    assert!(
        b.block_size.uncompressed && b.block_size.data_size == 0,
        "block size = {:?}, want uncompressed data_size 0",
        b.block_size
    );
    let seg = as_uncompressed_data_segment(&b.data[0], "empty uncompressed");
    assert!(
        b.data.len() == 1 && seg.size == 0 && seg.hex.is_empty(),
        "data = {:?}, want single zero-length segment",
        b.data
    );
    let checksum = b.block_checksum.as_ref().expect("block_checksum = None, want present");
    assert_eq!(seg.offset, checksum.offset, "data offset != block checksum offset, want equal");
    assert_eq!(checksum.value, 0x12345678, "block_checksum.value");
    assert_eq!(end_mark.offset, checksum.offset + 4, "end_mark.offset");
}

// --- checksums ------------------------------------------------------------

/// Confirms a run of several blocks, each followed by a Block Checksum, plus a
/// trailing Content Checksum, decode with correct offsets and are unaffected
/// by the stored checksum values (this crate never recomputes or validates
/// them).
#[test]
fn parse_data_blocks_multiple_blocks_with_checksums() {
    for checksum in [0x00000000u32, 0x12345678, 0xffffffff] {
        let literal_payloads: [&[u8]; 3] = [&[0x01], &[0x02, 0x03], &[0x04, 0x05, 0x06]];
        let block_data: Vec<Vec<u8>> = literal_payloads.iter().map(|p| literal_only_block_bytes(p)).collect();

        let mut input = Vec::new();
        for d in &block_data {
            input.extend(block_size_bytes(false, d.len() as u32));
            input.extend(d);
            input.extend(u32_le(checksum));
        }
        input.extend(end_mark_bytes());
        input.extend(u32_le(checksum));

        let (res, _) = parse_data_blocks_tuple(&input, &flg_with(true, true), &bd_with_max_size(65536));
        let (blocks, end_mark, content_checksum) =
            res.unwrap_or_else(|e| panic!("checksum {checksum:#x}: parse_data_blocks: {e}"));
        assert_eq!(blocks.len(), block_data.len(), "checksum {checksum:#x}: blocks.len()");

        let mut offset = 0u64;
        for (i, d) in block_data.iter().enumerate() {
            let label = format!("checksum {checksum:#x}: block {i}");
            let b = &blocks[i];
            assert_eq!(b.block_size.field.offset, offset, "{label}: block_size offset (block = {b:?})");
            offset += 4;
            let seq = as_sequence(&b.data[0], &label);
            assert!(
                seq.token.offset == offset
                    && seq.literals.offset == offset + 1
                    && seq.literals.hex == hex_string(literal_payloads[i]),
                "{label}: data = {:?}",
                b.data[0]
            );
            offset += d.len() as u64;
            let bc = b.block_checksum.as_ref().unwrap_or_else(|| panic!("{label}: block_checksum = None"));
            assert!(
                bc.offset == offset && bc.value == checksum,
                "{label}: checksum = {bc:?}, want offset {offset} value {checksum:#x}"
            );
            // The last sequence's own last physical field (literals, since a
            // literal-only sequence's token+literals are its only fields) ends
            // exactly where the following Block Checksum begins.
            assert_eq!(seq.literals.offset + seq.literals.size, bc.offset, "{label}: literals end != checksum offset");
            offset += 4;
        }

        assert_eq!(end_mark.offset, offset, "checksum {checksum:#x}: end_mark.offset");
        offset += 4;
        let cc = content_checksum.unwrap_or_else(|| panic!("checksum {checksum:#x}: content_checksum = None"));
        assert!(
            cc.offset == offset && cc.value == checksum,
            "checksum {checksum:#x}: content_checksum = {cc:?}, want offset {offset} value {checksum:#x}"
        );
    }
}

// --- Block Maximum Size ----------------------------------------------------

/// Confirms a Block Size whose data size exceeds the Frame Descriptor's Block
/// Maximum Size is rejected, for both compressed and uncompressed blocks.
#[test]
fn parse_data_blocks_rejects_block_max_size_exceeded() {
    for (name, uncompressed) in [("compressed", false), ("uncompressed", true)] {
        let input = block_size_bytes(uncompressed, 5); // exceeds max of 4
        let (res, _) = parse_data_blocks_tuple(&input, &flg_with(false, false), &bd_with_max_size(4));
        let err = res.expect_err(&format!("{name}: expected error"));
        assert!(
            err.offset == 0 && err.path == "data_blocks[0].block_size",
            "{name}: err = {err}, want ParseError at offset 0 path data_blocks[0].block_size"
        );
    }
}

/// Confirms a data size exactly equal to the Block Maximum Size is accepted
/// (the limit is inclusive), not just sizes strictly below it. This guards
/// against `decode_block_size`'s bound check regressing from `>` to `>=`. It
/// covers both branches: `block_size.data_size` (the encoded byte count
/// `decode_block_size` checks) is what's pinned at the limit in both cases;
/// for the compressed case the sequence's own expanded size (3, one byte less
/// than data_size's 4) stays safely under the same limit too, so this test
/// does not also exercise the independent Block Maximum Size check on expanded
/// size.
#[test]
fn parse_data_blocks_accepts_block_max_size_exactly() {
    // uncompressed
    let data = [0x42u8; 4];
    let mut input = block_size_bytes(true, 4);
    input.extend(data);
    input.extend(end_mark_bytes());

    let (res, _) = parse_data_blocks_tuple(&input, &flg_with(false, false), &bd_with_max_size(4));
    let (blocks, _, _) = res.expect("uncompressed: parse_data_blocks");
    assert!(blocks.len() == 1 && blocks[0].block_size.data_size == 4, "uncompressed: blocks = {blocks:?}");

    // compressed
    let data = literal_only_block_bytes(&[0x42, 0x42, 0x42]); // 1 token byte + 3 literal bytes = 4
    let mut input = block_size_bytes(false, data.len() as u32);
    input.extend(&data);
    input.extend(end_mark_bytes());

    let (res, _) = parse_data_blocks_tuple(&input, &flg_with(false, false), &bd_with_max_size(data.len() as u32));
    let (blocks, _, _) = res.expect("compressed: parse_data_blocks");
    assert!(
        blocks.len() == 1 && blocks[0].block_size.data_size == data.len() as u32,
        "compressed: blocks = {blocks:?}, want one block with data_size {}",
        data.len()
    );
}

/// Confirms the Block Maximum Size boundary check is exact for every valid BD
/// code (4-7), not just the arbitrary max of 4 used by
/// `parse_data_blocks_accepts_block_max_size_exactly` and
/// `parse_data_blocks_rejects_block_max_size_exceeded`: a data size equal to
/// the code's real byte limit is accepted, one byte over is rejected. It calls
/// `decode_block_size` directly rather than `parse_data_blocks`, since the
/// declared size is what's under test, not the block's actual data bytes
/// (which `decode_block_size` never reads).
#[test]
fn decode_block_size_boundary_all_block_max_size_codes() {
    for code in 4u8..=7 {
        let max = block_max_size_bytes(code).unwrap_or_else(|| panic!("code {code}: block_max_size_bytes undefined"));
        let bd = bd_with_max_size(max);

        let field = NumericField::new(0, max, &u32_le(max));
        let bs = decode_block_size(field, &bd, "data_blocks[0].block_size")
            .unwrap_or_else(|e| panic!("code {code} exact: decode_block_size: {e}"));
        assert_eq!(bs.data_size, max, "code {code} exact: data_size");

        let over = max + 1;
        let field = NumericField::new(0, over, &u32_le(over));
        let err = decode_block_size(field, &bd, "data_blocks[0].block_size")
            .expect_err(&format!("code {code} exceeded by 1: expected error"));
        assert!(
            err.offset == 0 && err.path == "data_blocks[0].block_size",
            "code {code} exceeded by 1: err = {err}, want ParseError at offset 0 path data_blocks[0].block_size"
        );
    }
}

/// Decodes literal Block Size bytes (0x00000000 and 0x80000000) written
/// directly in the test, rather than through the `block_size_bytes` helper
/// used elsewhere: `block_size_bytes` shares `BLOCK_UNCOMPRESSED_BIT` with the
/// production code under test, so a shared misunderstanding of that constant's
/// value could pass every case built through it without ever being wrong.
/// Mirrors the `parse_flg_literal_bytes`/`parse_bd_literal_bytes` pattern.
#[test]
fn parse_data_blocks_literal_end_mark_and_empty_uncompressed_block_bytes() {
    // 00 00 00 00 is EndMark, no block.
    let input = [0x00u8, 0x00, 0x00, 0x00];
    let (res, _) = parse_data_blocks_tuple(&input, &flg_with(false, false), &bd_with_max_size(65536));
    let (blocks, end_mark, _) = res.expect("EndMark: parse_data_blocks");
    assert!(
        blocks.is_empty() && end_mark.value == 0,
        "blocks = {blocks:?}, end_mark = {end_mark:?}, want no blocks and EndMark value 0"
    );

    // 00 00 00 80 is an empty uncompressed block, not EndMark.
    let mut input = vec![0x00u8, 0x00, 0x00, 0x80]; // little-endian 0x80000000
    input.extend(end_mark_bytes());
    let (res, _) = parse_data_blocks_tuple(&input, &flg_with(false, false), &bd_with_max_size(65536));
    let (blocks, _, _) = res.expect("empty uncompressed: parse_data_blocks");
    assert_eq!(blocks.len(), 1, "empty uncompressed: blocks.len()");
    assert!(
        blocks[0].block_size.uncompressed && blocks[0].block_size.data_size == 0,
        "block size = {:?}, want uncompressed data_size 0",
        blocks[0].block_size
    );
}

// --- truncated input -------------------------------------------------------

/// Confirms the Block Size/EndMark field, block data, Block Checksum, and
/// Content Checksum each report a `ParseError` naming their offset and
/// expected/actual byte counts when cut short. It uses an uncompressed block
/// so the "data" cut exercises `parse_data_blocks`' own `read_data`
/// truncation, not the Block Format parser's (covered separately in the block
/// parser tests).
#[test]
fn parse_data_blocks_truncated_fields() {
    let block_data = [0x01u8, 0x02, 0x03];
    let mut full = block_size_bytes(true, block_data.len() as u32);
    full.extend(block_data);
    full.extend(u32_le(0x12345678)); // block checksum
    full.extend(end_mark_bytes());
    full.extend(u32_le(0x9abcdef0)); // content checksum
    assert_eq!(full.len(), 19, "fixture length");

    // The 4-byte field is read before it is known to be a real Block Size or
    // the terminating 0x00000000, so a cut in the EndMark is reported under the
    // path of the block size that would have come next.
    let fields: [(&str, u64, usize); 5] = [
        ("data_blocks[0].block_size", 0, 4),
        ("data_blocks[0].data", 4, 3),
        ("data_blocks[0].block_checksum", 7, 4),
        ("data_blocks[1].block_size", 11, 4),
        ("content_checksum", 15, 4),
    ];

    for (path, offset, length) in fields {
        for read in 0..length {
            let label = format!("{path} cut at {read} of {length} bytes");
            let input = &full[..offset as usize + read];
            let (res, _) = parse_data_blocks_tuple(input, &flg_with(true, true), &bd_with_max_size(65536));
            let err = res.expect_err(&format!("{label}: expected error"));

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

// --- read_chunked chunk boundaries -------------------------------------

/// Returns deterministic, non-repeating-byte data of the given length, so a
/// chunking bug that drops, duplicates, or reorders bytes across a
/// `read_chunked` chunk boundary would change the output.
fn chunk_pattern_data(n: usize) -> Vec<u8> {
    (0..n).map(|i| i as u8).collect()
}

/// Confirms a single uncompressed block's data larger than `DATA_CHUNK_SIZE`
/// round-trips correctly across multiple chunked reads, with no buffer-reuse
/// corruption and no cumulative-offset drift. (The Block Format parser's own
/// chunked literal reading is covered separately in the block parser tests.)
#[test]
fn parse_data_blocks_data_across_chunk_boundaries() {
    let size = DATA_CHUNK_SIZE * 2 + 137;
    let data = chunk_pattern_data(size);

    let mut input = block_size_bytes(true, size as u32);
    input.extend(&data);
    input.extend(end_mark_bytes());

    let (res, offset) = parse_data_blocks_tuple(&input, &flg_with(false, false), &bd_with_max_size(4 * 1024 * 1024));
    let (blocks, end_mark, _) = res.expect("parse_data_blocks");
    assert_eq!(blocks.len(), 1, "blocks.len()");

    let seg = as_uncompressed_data_segment(&blocks[0].data[0], "block 0");
    assert!(
        seg.offset == 4 && seg.size == size as u64,
        "segment offset/size = {}/{}, want 4/{size}",
        seg.offset,
        seg.size
    );
    assert!(seg.hex == hex_string(&data), "segment hex does not match input data across chunk boundaries");
    assert_eq!(end_mark.offset, (4 + size) as u64, "end_mark.offset");
    assert_eq!(offset, input.len() as u64, "reader offset");
}

/// Confirms a short read that happens after `read_data` has already completed
/// at least one full chunk still reports the `ParseError` at the data field's
/// start offset, with expected/actual byte counts for the whole field rather
/// than just the final partial chunk.
#[test]
fn parse_data_blocks_truncated_data_after_full_chunk() {
    let declared_size = DATA_CHUNK_SIZE + 137;
    let actual_size = DATA_CHUNK_SIZE + 50; // one full chunk, then a partial second chunk

    let mut input = block_size_bytes(true, declared_size as u32);
    input.extend(chunk_pattern_data(actual_size));

    let (res, offset) = parse_data_blocks_tuple(&input, &flg_with(false, false), &bd_with_max_size(4 * 1024 * 1024));
    let err = res.expect_err("expected error");

    assert_eq!(err.offset, 4, "err = {err}, want ParseError at offset 4 (data field start)");
    match err.cause {
        ParseCause::ShortRead { expected, actual } => assert!(
            expected == declared_size as u64 && actual == actual_size as u64,
            "ShortRead = {{expected:{expected} actual:{actual}}}, want {{expected:{declared_size} actual:{actual_size}}}"
        ),
        other => panic!("cause = {other:?}, want ShortRead"),
    }
    assert_eq!(offset, (4 + actual_size) as u64, "reader offset (advanced by bytes actually read)");
}

// --- compressed block error boundary ---------------------------------------

/// Confirms a Block Format error inside a compressed block never advances the
/// reader past the block's declared `data_size`, so the following Block
/// Checksum is left exactly where it belongs rather than being misread as
/// block data. The fixture packs a valid literal-only sequence (5 literal
/// bytes, satisfying the End of block condition on its own) plus one stray
/// trailing byte still inside the declared `data_size`: since 5 literal bytes
/// don't exhaust the declared data, the sequence parser tries to read a 2-byte
/// match offset but only 1 byte remains, and the boundary check rejects it
/// without ever reading that byte or anything past the block's 7 declared
/// bytes into the checksum that follows.
#[test]
fn parse_data_blocks_compressed_error_stops_at_block_boundary() {
    let mut compressed = literal_only_block_bytes(&[0x01, 0x02, 0x03, 0x04, 0x05]);
    compressed.push(0x00);
    let checksum_bytes = u32_le(0xdeadbeef);

    let mut input = Vec::new();
    input.extend(block_size_bytes(false, compressed.len() as u32));
    input.extend(&compressed);
    input.extend(&checksum_bytes);
    input.extend(end_mark_bytes());

    let data_start = 4u64;
    let data_end = data_start + compressed.len() as u64;

    let mut fr = FrameReader::new(&input[..]);
    let res = parse_data_blocks(&mut fr, &flg_with(true, false), &bd_with_max_size(65536));
    assert!(res.is_err(), "expected error, got Ok");
    assert!(
        fr.offset < data_end,
        "reader offset = {} after error, want strictly < {data_end} (block boundary): the failed read must leave \
         bytes unconsumed inside the block, and checksum bytes must stay unread",
        fr.offset
    );

    // Skip whatever the failed parse left unread within the block's declared
    // range, then confirm the Block Checksum bytes are exactly where they
    // should be, not consumed or shifted by the failed compressed-block parse.
    let remaining = (data_end - fr.offset) as usize;
    fr.read_exact(remaining, "test-skip-to-block-end").expect("reading remainder of declared block data");
    let (got_checksum, _) =
        fr.read_exact(4, "test-block-checksum").expect("reading block checksum after failed compressed block parse");
    assert_eq!(got_checksum, checksum_bytes, "block checksum bytes after failed compressed block parse");
}
