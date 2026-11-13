//! Deterministic fuzz tests for the block parser: a seed
//! corpus plus pseudo-random and mutated inputs from fixed seeds.

use std::io::Cursor;

use super::tests::BLOCK_TRAILER;
use super::*;
use crate::testutil::Rng;

/// One physical field's absolute offset, declared size, and hex encoding, used
/// to reconstruct a parsed block's bytes from `Sequence`'s public fields
/// without reusing the parser's own coverage helpers, so the check also
/// catches bugs in those helpers. `size` is compared with the decoded length
/// of `hex` explicitly, so a segment whose size drifts from its hex is caught.
struct HexSegment {
    name: &'static str,
    offset: u64,
    size: u64,
    hex: String,
}

/// Lists `seq`'s physical fields in encoded order: token, optional
/// literal_length_extension, literals, optional match_offset, optional
/// match_length_extension. match_offset is always exactly 2 bytes.
fn sequence_hex_segments(seq: &Sequence) -> Vec<HexSegment> {
    let seg = |name, offset, size, hex: &str| HexSegment { name, offset, size, hex: hex.to_string() };
    let mut segs = vec![seg("token", seq.token.offset, seq.token.size, &seq.token.hex)];
    if let Some(e) = &seq.literal_length_extension {
        segs.push(seg("literal_length_extension", e.offset, e.size, &e.hex));
    }
    segs.push(seg("literals", seq.literals.offset, seq.literals.size, &seq.literals.hex));
    if let Some(m) = &seq.match_offset {
        segs.push(seg("match_offset", m.offset, 2, &m.hex));
    }
    if let Some(e) = &seq.match_length_extension {
        segs.push(seg("match_length_extension", e.offset, e.size, &e.hex));
    }
    segs
}

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len() % 2 == 0, "odd-length hex {s:?}");
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex")).collect()
}

/// Feeds `parse_sequence_loop` the block `data` bounded to `declared` bytes,
/// with `BLOCK_TRAILER` after `data` so an overread consumes real bytes
/// rather than hitting a convenient EOF, and checks the invariants:
///
/// 1. it never panics;
/// 2. it never reads past the declared block boundary, measured from the
///    underlying cursor independently of `FrameReader::offset`;
/// 3. on success, the sequences' physical fields cover exactly
///    `[0, declared)` with no gap or overlap, and their hex concatenated in
///    encoded order reproduces the declared block bytes;
/// 4. on failure, the error is a `ParseError`, which always carries an
///    offset (enforced by the type).
fn check_block(data: &[u8], declared: u32) {
    let mut stream = data.to_vec();
    stream.extend_from_slice(&BLOCK_TRAILER);
    let mut cursor = Cursor::new(stream.as_slice());
    let label = format!("data={data:02x?} declared={declared}");

    let (result, fr_offset) = {
        let mut fr = FrameReader::new(&mut cursor);
        let mut br = BlockReader::new(&mut fr, u64::from(declared), 0);
        let bd = Bd {
            field: NumericField::new(0, 0, &[0]),
            reserved_bit_7: 0,
            block_max_size_code: 0,
            block_max_size_bytes: u32::MAX,
            reserved_bits_3_0: 0,
        };
        let result = parse_sequence_loop(&mut br, &bd);
        (result, fr.offset)
    };

    let consumed = cursor.position();
    assert!(
        consumed <= u64::from(declared),
        "{label}: block reader physically read past the declared boundary: consumed {consumed}"
    );
    assert_eq!(fr_offset, consumed, "{label}: FrameReader::offset disagrees with the bytes consumed from the stream");

    let Ok(sequences) = result else { return };

    assert_eq!(consumed, u64::from(declared), "{label}: succeeded but consumed {consumed}, want the declared size");

    let (mut hex, mut next) = (String::new(), 0u64);
    for (i, seq) in sequences.iter().enumerate() {
        for seg in sequence_hex_segments(seq) {
            assert_eq!(seg.offset, next, "{label}: sequence {i}: {} gap or overlap", seg.name);
            assert_eq!(
                unhex(&seg.hex).len() as u64,
                seg.size,
                "{label}: sequence {i}: {} size disagrees with its hex",
                seg.name
            );
            hex.push_str(&seg.hex);
            next += seg.size;
        }
    }
    assert_eq!(next, u64::from(declared), "{label}: segments must cover the declared size");
    assert_eq!(hex, hex_string(&stream[..declared as usize]), "{label}: hex concatenation");
}

/// The seed corpus: block bytes and the declared block size.
fn seeds() -> Vec<(&'static str, Vec<u8>, u32)> {
    let mut literal_ext_single = vec![0xf0, 0x0a];
    literal_ext_single.resize(literal_ext_single.len() + 25, b'A');
    let mut literal_ext_multi = vec![0xf0, 0xff, 0xff, 0x01];
    literal_ext_multi.resize(literal_ext_multi.len() + 526, b'B');
    vec![
        ("empty_block", vec![0x00], 1),
        ("literal_only_min", vec![0x50, b'h', b'e', b'l', b'l', b'o'], 6),
        (
            "match_extension_then_last_literals",
            vec![0x4f, b'a', b'b', b'c', b'd', 0x04, 0x00, 0x00, 0x50, b'e', b'f', b'g', b'h', b'i'],
            14,
        ),
        ("literal_length_extension_single_byte", literal_ext_single, 27),
        ("literal_length_extension_multi_byte", literal_ext_multi, 530),
        ("invalid_match_offset_zero", vec![0x14, b'a', 0x00, 0x00], 4),
        ("unterminated_length_extension", vec![0xf0, 0xff, 0xff, 0xff], 4),
        ("declared_size_exceeds_stream", vec![0x50], 100),
        ("match_at_block_end", vec![0x00, 0x01, 0x00], 3),
        ("zero_declared_size", vec![], 0),
    ]
}

#[test]
fn block_parser_seeds() {
    for (_, data, declared) in seeds() {
        check_block(&data, declared);
    }

    // The valid seeds must parse.
    for name in [
        "empty_block",
        "literal_only_min",
        "match_extension_then_last_literals",
        "literal_length_extension_single_byte",
    ] {
        let (_, data, declared) = seeds().into_iter().find(|(n, _, _)| *n == name).unwrap();
        let mut fr = FrameReader::new(Cursor::new(data));
        let mut br = BlockReader::new(&mut fr, u64::from(declared), 0);
        let bd = Bd {
            field: NumericField::new(0, 0, &[0]),
            reserved_bit_7: 0,
            block_max_size_code: 0,
            block_max_size_bytes: u32::MAX,
            reserved_bits_3_0: 0,
        };
        assert!(parse_sequence_loop(&mut br, &bd).is_ok(), "seed {name} must parse");
    }
}

/// Returns a mutated copy of `data` and a (possibly changed) declared size.
fn mutate(rng: &mut Rng, data: &[u8], declared: u32) -> (Vec<u8>, u32) {
    let mut data = data.to_vec();
    let mut declared = declared;
    for _ in 0..=rng.below(3) {
        match rng.below(7) {
            0 if !data.is_empty() => {
                let i = rng.below(data.len());
                data[i] = rng.next_u64() as u8;
            }
            1 if !data.is_empty() => {
                let i = rng.below(data.len());
                data[i] ^= 1 << rng.below(8);
            }
            2 if !data.is_empty() => {
                let n = rng.below(data.len());
                data.truncate(n);
            }
            3 => data.insert(rng.below(data.len() + 1), rng.next_u64() as u8),
            4 => declared = declared.saturating_add(1 + rng.below(4) as u32),
            5 => declared = declared.saturating_sub(1 + rng.below(4) as u32),
            _ => declared = rng.next_u64() as u32,
        }
    }
    (data, declared)
}

#[test]
fn block_parser_random_and_mutated_inputs() {
    let mut rng = Rng(0x1b10_c4a2_5eed_0001);
    let seeds = seeds();

    for iteration in 0..6000 {
        let (data, declared) = if iteration % 2 == 0 {
            // Random bytes, biased toward extension-continuation and zero
            // bytes, with a declared size near the data length.
            let len = rng.below(48);
            let data: Vec<u8> = (0..len)
                .map(|_| match rng.below(4) {
                    0 => 0xff,
                    1 => 0x00,
                    _ => rng.next_u64() as u8,
                })
                .collect();
            let declared = (len + rng.below(4)).saturating_sub(rng.below(2)) as u32;
            (data, declared)
        } else {
            let (_, data, declared) = &seeds[rng.below(seeds.len())];
            mutate(&mut rng, data, *declared)
        };
        // A huge declared size only exercises the short-read path, and each
        // such case would copy through the chunk buffer once: keep it rare.
        let declared = if declared > 1 << 20 { declared % 1000 } else { declared };
        check_block(&data, declared);
    }
}

#[test]
fn block_parser_assembled_blocks() {
    // Randomly assembled well-formed sequences: the parser either accepts them
    // with exact coverage or rejects them with an error, never anything else.
    let mut rng = Rng(0x1b10_c4a2_5eed_0002);
    for _ in 0..1500 {
        let mut data = Vec::new();
        for _ in 0..=rng.below(3) {
            let lit = rng.below(40);
            let match_code = rng.below(16) as u8;
            data.push((lit.min(15) as u8) << 4 | match_code);
            if lit >= 15 {
                data.resize(data.len() + (lit - 15) / 255, 0xff);
                data.push(((lit - 15) % 255) as u8);
            }
            data.extend((0..lit).map(|_| rng.next_u64() as u8));
            data.extend_from_slice(&[1 + rng.below(255) as u8, 0]);
            if match_code == 15 {
                data.push(rng.below(255) as u8);
            }
        }
        // Finish with a literal-only sequence of 5..=24 literals.
        let lit = 5 + rng.below(20);
        data.push((lit.min(15) as u8) << 4);
        if lit >= 15 {
            data.push((lit - 15) as u8);
        }
        data.extend((0..lit).map(|_| rng.next_u64() as u8));
        let declared = data.len() as u32;
        check_block(&data, declared);
    }
}
