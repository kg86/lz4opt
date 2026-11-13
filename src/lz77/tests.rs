use std::io::{self, Cursor, Read, Write};

use miniz_oxide::deflate::core::{create_comp_flags_from_zip_params, CompressionStrategy, CompressorOxide};
use miniz_oxide::inflate::decompress_to_vec;

use super::format::{deflate_huffman_only, put_uvarint, Field, BLOCK_TOKENS};
use super::*;
use crate::testutil::Rng;

// Parser

/// Uses direct text comparisons, independently of the tree.
fn reference(input: &[u8], window: usize, max_length: usize) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start = 0;
    while start < input.len() {
        let (mut best, mut distance) = (0, 0);
        for pos in start.saturating_sub(window)..start {
            let mut length = 0;
            while start + length + 1 < input.len()
                && length < max_length
                && input[pos + length] == input[start + length]
            {
                length += 1;
            }
            if length > best {
                (best, distance) = (length, start - pos);
            }
        }
        // The bound above leaves a literal even for a copy reaching the end.
        tokens.push(Token { distance, length: best, next: input[start + best] });
        start += best + 1;
    }
    tokens
}

fn check_parser(input: &[u8], opts: Options) {
    let mut p = Parser::new(opts);
    let (window, max_length) = (p.window(), p.max_length());
    let mut got = Vec::new();
    let mut phrase_start = 0;
    let mut pos = 0;
    while pos < input.len() {
        let (token, consumed) = p.push(&input[pos..]);
        pos += consumed;
        if let Some(token) = token {
            assert!(
                token.next == input[pos - 1] && token.length == pos - 1 - phrase_start,
                "push token = {token:?} at pos {pos}, phrase start {phrase_start}"
            );
            got.push(token);
            phrase_start = pos;
        }
        let tree = p.tree();
        assert!(
            tree.end() == pos && tree.begin() == phrase_start.saturating_sub(window),
            "retained [{},{}) at pos {pos}, phrase start {phrase_start}, window {window}",
            tree.begin(),
            tree.end()
        );
    }
    if let Some(token) = p.finish() {
        assert!(
            token.next == input[input.len() - 1] && token.length == input.len() - 1 - phrase_start,
            "invalid final token {token:?}, phrase start {phrase_start}"
        );
        got.push(token);
    }
    assert_eq!(p.finish(), None, "repeated finish");
    let want = reference(input, window, max_length);
    assert_eq!(got.len(), want.len(), "input {input:?} {opts:?}: tokens = {got:?}, want {want:?}");
    let mut decoded = Vec::new();
    for (i, token) in got.iter().enumerate() {
        // Equally long sources may differ between implementations.
        assert!(
            token.length == want[i].length && token.next == want[i].next,
            "input {input:?} {opts:?} token {i}: got {token:?}, want length/literal from {:?}",
            want[i]
        );
        assert!(
            token.distance <= window.min(decoded.len())
                && token.length <= max_length
                && (token.distance == 0) == (token.length == 0),
            "invalid distance in {token:?} at position {}",
            decoded.len()
        );
        for _ in 0..token.length {
            decoded.push(decoded[decoded.len() - token.distance]);
        }
        decoded.push(token.next);
    }
    assert_eq!(decoded, input, "window {window}");
}

#[test]
fn parser_examples() {
    let inputs: [&[u8]; 8] =
        [b"", b"x", b"abcdef", b"abcabc", b"abcabcX", b"aaabaaaaaaaa", b"abacabadabacaba", b"\x00\xff\x00\xff\x00"];
    for input in inputs {
        for window in [0, 1, 2, 3, 8, 32] {
            for max_length in [0, 1, 2, 3, 5] {
                check_parser(input, Options { window, max_length });
            }
        }
    }
}

#[test]
fn parser_exhaustive() {
    for n in 0..=9 {
        for bits in 0..1u32 << n {
            let input: Vec<u8> = (0..n).map(|i| (bits >> i & 1) as u8).collect();
            for window in [1, 2, 3, 5] {
                for max_length in [0, 1, 2, 4] {
                    check_parser(&input, Options { window, max_length });
                }
            }
        }
    }
}

#[test]
fn parser_random() {
    let mut rng = Rng(91);
    for run in 0..100 {
        let alphabet = [2, 7, 256][run % 3];
        let input: Vec<u8> = (0..128).map(|_| rng.below(alphabet) as u8).collect();
        check_parser(&input, Options { window: rng.below(33), max_length: rng.below(9) });
    }
}

#[test]
fn parser_long_phrases() {
    for pattern in [&b"a"[..], b"abc"] {
        let mut input = pattern.repeat(4096);
        // The mismatch forces contraction of a long pending phrase, followed
        // by more input to exercise buffer wrapping and node-slot reuse.
        input.push(b'X');
        input.extend(pattern.repeat(128));
        check_parser(&input, Options { window: pattern.len(), max_length: 0 });
        check_parser(&input, Options { window: pattern.len(), max_length: 1000 });
    }
}

#[test]
fn parser_all_bytes() {
    let input: Vec<u8> = (0..=255u8).flat_map(|i| [i, i, 0]).collect();
    check_parser(&input, Options { window: 8, max_length: 0 });
}

#[test]
fn parser_lifecycle() {
    let p = Parser::new(Options::default());
    assert_eq!((p.window(), p.max_length()), (isize::MAX as usize, isize::MAX as usize), "wrong defaults");
    let p = Parser::new(Options { window: usize::MAX, max_length: isize::MAX as usize + 1 });
    assert_eq!((p.window(), p.max_length()), (isize::MAX as usize, isize::MAX as usize), "limits not lowered");
    let mut p = Parser::new(Options { window: 3, max_length: 4 });
    assert_eq!((p.window(), p.max_length()), (3, 4), "wrong options");
    assert_eq!(p.finish(), None);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.push(b"a")));
    assert!(result.is_err(), "push after finish did not panic");
}

#[test]
fn parser_example() {
    let mut p = Parser::new(Options { window: 3, max_length: 0 });
    let mut data = &b"abcabcX"[..];
    let mut tokens = Vec::new();
    while !data.is_empty() {
        let (token, consumed) = p.push(data);
        data = &data[consumed..];
        tokens.extend(token);
    }
    tokens.extend(p.finish());
    let lit = |next| Token { distance: 0, length: 0, next };
    assert_eq!(tokens, [lit(b'a'), lit(b'b'), lit(b'c'), Token { distance: 3, length: 3, next: b'X' }]);
}

#[test]
fn fit_to_size_limits() {
    let o = |window, max_length| Options { window, max_length };
    for (opts, size, want) in [
        (o(UNBOUNDED, UNBOUNDED), 1000, o(999, 999)),
        (o(usize::MAX, usize::MAX), 1000, o(999, 999)),
        (o(0, 0), 1000, o(999, 999)),
        (o(100, 5000), 1000, o(100, 999)),
        (o(100, 10), 1000, o(100, 10)),
        (o(0, 0), 1, o(1, 1)),
        (o(0, 0), 0, o(1, 1)),
    ] {
        assert_eq!(opts.fit_to_size(size), want, "{opts:?}.fit_to_size({size})");
    }
}

/// With varint fields, only the header may change when limits are fitted.
#[test]
fn fit_to_size_keeps_parse() {
    for input in ["", "a", "aa", "abcabcX", "aaaaaaaaaa", "abababababab", "abcabcabcab"] {
        let encode = |options| {
            let mut encoded = Vec::new();
            let opts =
                EncodeOptions { options, distance: Coding::Varint, length: Coding::Varint, ..Default::default() };
            super::encode(&mut encoded, input.as_bytes(), &opts).unwrap();
            encoded
        };
        const UNBOUNDED_HEADER: usize = 5 + 9 + 9 + 3; // magic, two max-int uvarints, codings
        let want = encode(Options::default())[UNBOUNDED_HEADER..].to_vec();
        let fitted = encode(Options::default().fit_to_size(input.len() as u64));
        assert_eq!(&fitted[fitted.len() - want.len()..], &want[..], "{input:?}");
    }
}

// Format

/// Reads at most half of what is asked for, so that short reads are exercised.
struct HalfReader<R>(R);

impl<R: Read> Read for HalfReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().div_ceil(2);
        self.0.read(&mut buf[..n])
    }
}

/// Reads one byte at a time, so that short reads are exercised.
struct OneByteReader<R>(R);

impl<R: Read> Read for OneByteReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(1);
        self.0.read(&mut buf[..n])
    }
}

struct FailingReader;

impl Read for FailingReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("failure"))
    }
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("failure"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn opts(window: usize, max_length: usize) -> EncodeOptions {
    EncodeOptions { options: Options { window, max_length }, ..Default::default() }
}

fn round_trip(input: &[u8], opts: &EncodeOptions) -> Vec<u8> {
    let mut encoded = Vec::new();
    encode(&mut encoded, HalfReader(input), opts).unwrap();
    let mut decoded = Vec::new();
    decode(&mut decoded, OneByteReader(&encoded[..])).unwrap_or_else(|err| panic!("{opts:?}: {err}"));
    assert!(decoded == input, "{opts:?}: round trip of {} bytes returned {} bytes", input.len(), decoded.len());
    encoded
}

fn is_format_error(r: Result<(), Error>) -> bool {
    matches!(r, Err(Error::Format(_)))
}

/// The uvarint of the default (unbounded) window and maximum length.
const MAX_INT_UVARINT: [u8; 9] = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

#[test]
fn encode_layout() {
    let magic = [b'L', b'Z', b'7', b'7', 1];
    let cases: Vec<(&[u8], EncodeOptions, Vec<u8>)> = vec![
        // One-byte fixed distances (stored as distance-1), varint lengths.
        (b"abcabcX", opts(3, 3), cat(&[&magic, &[3, 3, 1, 2, 0, 0, b'a', 0, b'b', 0, b'c', 3, 2, b'X']])),
        // A copy is cut at max_length and the next byte becomes its literal.
        (b"abcabcX", opts(3, 2), cat(&[&magic, &[3, 2, 1, 2, 0, 0, b'a', 0, b'b', 0, b'c', 2, 2, b'c', 0, b'X']])),
        // A copy reaching the end is shortened so that its last byte is a literal.
        (b"aaaa", opts(200, 5), cat(&[&magic, &[0xc8, 0x01, 5, 1, 2, 0, 0, b'a', 2, 0, b'a']])),
        // Two-byte fixed lengths (max_length 256), varint distances.
        (
            b"abab",
            EncodeOptions { distance: Coding::Varint, length: Coding::Fixed, ..opts(300, 256) },
            cat(&[&magic, &[0xac, 0x02, 0x80, 0x02, 2, 1, 0, 0, 0, b'a', 0, 0, b'b', 1, 0, 1, b'b']]),
        ),
        // Two-byte fixed distances (window 300).
        (b"abab", opts(300, 256), cat(&[&magic, &[0xac, 0x02, 0x80, 0x02, 1, 2, 0, 0, b'a', 0, b'b', 1, 1, 0, b'b']])),
        (b"", EncodeOptions::default(), cat(&[&magic, &MAX_INT_UVARINT, &MAX_INT_UVARINT, &[1, 2, 0]])),
        (
            b"",
            EncodeOptions { entropy: Entropy::Huffman, ..Default::default() },
            cat(&[&magic, &MAX_INT_UVARINT, &MAX_INT_UVARINT, &[1, 2, 1]]),
        ),
    ];
    for (input, opts, want) in cases {
        let got = round_trip(input, &opts);
        assert!(got == want, "{input:?} {opts:?} encoded {got:02x?}, want {want:02x?}");
    }
}

#[test]
fn field_width() {
    for (max, width) in
        [(0, 1), (255, 1), (256, 2), (65535, 2), (65536, 4), (u32::MAX as u64, 4), (1 << 32, 8), (isize::MAX as u64, 8)]
    {
        assert_eq!(Field::new("", Coding::Fixed, max).width, width, "width for max {max}");
    }
}

#[test]
fn round_trips() {
    let mut rng = Rng(5);
    let mut inputs: Vec<Vec<u8>> = vec![vec![], b"x".to_vec(), vec![0, 0, 0], b"ab".repeat(100000)];
    for alphabet in [2, 16, 256] {
        inputs.push((0..200000).map(|_| rng.below(alphabet) as u8).collect());
    }
    let huffman = |o: EncodeOptions| EncodeOptions { entropy: Entropy::Huffman, ..o };
    let variants = [
        opts(1, 0),
        EncodeOptions { length: Coding::Fixed, ..opts(7, 3) },
        EncodeOptions { distance: Coding::Varint, ..opts(1000, 0) },
        EncodeOptions { distance: Coding::Fixed, length: Coding::Fixed, ..opts(1 << 16, 1 << 16) },
        EncodeOptions { distance: Coding::Varint, length: Coding::Varint, ..opts(0, 100) },
        EncodeOptions::default(),
        opts(usize::MAX, usize::MAX),
    ];
    for input in &inputs {
        for variant in variants {
            round_trip(input, &variant);
            round_trip(input, &huffman(variant));
        }
    }
}

#[test]
fn decode_invalid() {
    // Window 2, max length 3, one-byte fixed distances, varint lengths.
    let header = [b'L', b'Z', b'7', b'7', 1, 2, 3, 1, 2, 0];
    let with = |tail: &[u8]| cat(&[&header, tail]);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("bad magic", vec![b'L', b'Z', b'7', b'8', 1, 2, 3, 1, 2, 0]),
        ("bad version", vec![b'L', b'Z', b'7', b'7', 2, 2, 3, 1, 2, 0]),
        ("no max length", vec![b'L', b'Z', b'7', b'7', 1, 2]),
        ("no codings", vec![b'L', b'Z', b'7', b'7', 1, 2, 3, 1]),
        ("no entropy", vec![b'L', b'Z', b'7', b'7', 1, 2, 3, 1, 2]),
        ("bad coding", vec![b'L', b'Z', b'7', b'7', 1, 2, 3, 0, 2, 0]),
        ("bad entropy", vec![b'L', b'Z', b'7', b'7', 1, 2, 3, 1, 2, 2]),
        ("zero window", vec![b'L', b'Z', b'7', b'7', 1, 0, 3, 1, 2, 0]),
        ("zero max length", vec![b'L', b'Z', b'7', b'7', 1, 2, 0, 1, 2, 0]),
        (
            "huge window",
            vec![b'L', b'Z', b'7', b'7', 1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01, 3, 1, 2, 0],
        ),
        (
            "huge max length",
            vec![b'L', b'Z', b'7', b'7', 1, 2, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01, 1, 2, 0],
        ),
        ("too long", with(&[0, b'a', 4, 0, b'b'])),
        ("truncated literal", with(&[0])),
        ("truncated varint", with(&[0x80])),
        ("missing distance", with(&[0, b'a', 1])),
        ("missing literal", with(&[0, b'a', 1, 0])),
        ("distance too far", with(&[0, b'a', 1, 1, b'b'])),
        ("beyond window", with(&[0, b'a', 0, b'b', 0, b'c', 1, 2, b'd'])),
        ("varint overflow", with(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01])),
        // Window 1000 takes two-byte fixed distances.
        ("truncated fixed", vec![b'L', b'Z', b'7', b'7', 1, 0xe8, 0x07, 3, 1, 2, 0, 0, b'a', 1, 0]),
    ];
    for (name, input) in cases {
        assert!(is_format_error(decode(Vec::new(), &input[..])), "{name}: decode did not return a format error");
    }
}

/// Compresses `raw` as one stream of a Huffman block, prefixed by its size.
fn huffman_stream(raw: &[u8]) -> Vec<u8> {
    let flags = create_comp_flags_from_zip_params(1, 0, CompressionStrategy::HuffmanOnly as i32);
    let compressed = deflate_huffman_only(&mut CompressorOxide::new(flags), raw);
    let mut out = Vec::new();
    put_uvarint(&mut out, compressed.len() as u64);
    out.extend_from_slice(&compressed);
    out
}

/// Checks the block layout by decompressing each stream, since the Huffman
/// codes themselves depend on the deflate implementation.
#[test]
fn huffman_layout() {
    let input = b"abcabcX";
    // Window 300 gives two-byte fixed distances; lengths are varints.
    let encoded = round_trip(input, &EncodeOptions { entropy: Entropy::Huffman, ..opts(300, 300) });
    let mut r = &encoded[5 + 2 + 2 + 3..];
    let read_uvarint = |r: &mut &[u8]| {
        let (mut x, mut shift) = (0u64, 0);
        loop {
            let b = r[0];
            *r = &r[1..];
            x |= ((b & 0x7f) as u64) << shift;
            shift += 7;
            if b < 0x80 {
                return x as usize;
            }
        }
    };
    assert_eq!(read_uvarint(&mut r), 4, "block size");
    let want: [&[u8]; 4] = [&[0, 0, 0, 3], &[2], &[0], b"abcX"];
    for (i, want) in want.iter().enumerate() {
        let size = read_uvarint(&mut r);
        let (compressed, rest) = r.split_at(size);
        r = rest;
        let got = decompress_to_vec(compressed).unwrap();
        assert!(got == *want, "stream {i} = {got:02x?}; want {want:02x?}");
    }
    assert!(r.is_empty(), "{} bytes after the block", r.len());
}

/// Encodes a block with Huffman streams for one-byte fixed distances and
/// varint lengths.
fn huffman_block_bytes(tokens: u64, lengths: &[u8], distances: &[u8], literals: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    put_uvarint(&mut b, tokens);
    for s in [lengths, distances, literals] {
        b.extend(huffman_stream(s));
    }
    b
}

#[test]
fn decode_invalid_huffman() {
    // Window 2, max length 3, one-byte fixed distances, varint lengths, Huffman.
    let header = [b'L', b'Z', b'7', b'7', 1, 2, 3, 1, 2, 1];
    let valid = huffman_block_bytes(2, &[0, 1], &[0], b"ab");
    assert!(decode(Vec::new(), &cat(&[&header, &valid])[..]).is_ok(), "valid block");
    let stream = huffman_stream(b"a");
    let with_size = |size: usize, data: &[u8]| {
        let mut b = Vec::new();
        put_uvarint(&mut b, size as u64);
        b.extend_from_slice(data);
        b
    };
    let mut too_many = Vec::new();
    put_uvarint(&mut too_many, BLOCK_TOKENS as u64 + 1);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("zero tokens", cat(&[&header, &[0]])),
        ("too many tokens", cat(&[&header, &too_many])),
        ("truncated size", cat(&[&header, &[1]])),
        ("truncated stream", cat(&[&header, &[1], &stream[..stream.len() - 1]])),
        ("missing stream", cat(&[&header, &[1], &huffman_stream(&[0]), &huffman_stream(&[])])),
        ("corrupt stream", cat(&[&header, &[1, 2, 0xff, 0xff]])),
        ("bytes after data", cat(&[&header, &[1], &with_size(stream.len(), &stream[1..]), &with_size(1, &[0])])),
        ("too long literals", cat(&[&header, &huffman_block_bytes(1, &[0], &[], b"ab")])),
        ("too long fixed", cat(&[&header, &huffman_block_bytes(2, &[0, 1], &[0, 0, 0], b"ab")])),
        ("unused varint", cat(&[&header, &huffman_block_bytes(1, &[0, 0], &[], b"a")])),
        ("unused distance", cat(&[&header, &huffman_block_bytes(1, &[0], &[0], b"a")])),
        ("missing literal", cat(&[&header, &huffman_block_bytes(2, &[0, 1], &[0], b"a")])),
        ("missing distance", cat(&[&header, &huffman_block_bytes(2, &[0, 1], &[], b"ab")])),
        ("length too long", cat(&[&header, &huffman_block_bytes(2, &[0, 4], &[0], b"ab")])),
        ("distance too far", cat(&[&header, &huffman_block_bytes(2, &[0, 1], &[1], b"ab")])),
        ("truncated block", cat(&[&header, &valid[..valid.len() - 1]])),
        ("trailing byte", cat(&[&header, &valid, &[1]])),
    ];
    for (name, input) in cases {
        assert!(is_format_error(decode(Vec::new(), &input[..])), "{name}: decode did not return a format error");
    }
}

#[test]
fn io_errors() {
    assert!(matches!(decode(Vec::new(), FailingReader), Err(Error::Io(_))), "read error");
    let mut encoded = Vec::new();
    encode(&mut encoded, &b"abc"[..], &EncodeOptions::default()).unwrap();
    assert!(matches!(decode(FailingWriter, &encoded[..]), Err(Error::Io(_))), "write error");
    assert!(matches!(encode(FailingWriter, &b"abc"[..], &EncodeOptions::default()), Err(Error::Io(_))), "encode write");
    assert!(matches!(encode(Vec::new(), FailingReader, &EncodeOptions::default()), Err(Error::Io(_))), "encode read");
}

/// Bounds the output of copies with huge lengths.
struct LimitedWriter(Vec<u8>, usize);

impl Write for LimitedWriter {
    fn write(&mut self, p: &[u8]) -> io::Result<usize> {
        if self.0.len() + p.len() > self.1 {
            return Err(io::Error::other("output limit"));
        }
        self.0.extend_from_slice(p);
        Ok(p.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A fixed-seed fuzz test: random and mutated streams either fail to decode
/// or decode to output that survives an encode/decode round trip.
#[test]
fn decode_mutated() {
    let mut rng = Rng(0x9e37);
    let mut seeds = Vec::new();
    for input in [&b""[..], b"abcabcX", b"aaaa", b"\x00\xff\x00"] {
        for o in [
            opts(3, 2),
            EncodeOptions { distance: Coding::Varint, length: Coding::Fixed, ..opts(300, 0) },
            EncodeOptions { entropy: Entropy::Huffman, ..opts(300, 0) },
        ] {
            seeds.push(round_trip(input, &o));
        }
    }
    for _ in 0..20000 {
        let mut stream = seeds[rng.below(seeds.len())].clone();
        for _ in 0..1 + rng.below(3) {
            if stream.is_empty() {
                break;
            }
            let i = rng.below(stream.len());
            match rng.below(3) {
                0 => stream[i] = rng.next_u64() as u8,
                1 => stream.truncate(i),
                _ => stream.insert(i, rng.next_u64() as u8),
            }
        }
        let mut out = LimitedWriter(Vec::new(), 1 << 20);
        if decode(&mut out, &stream[..]).is_err() {
            continue;
        }
        let mut encoded = Vec::new();
        encode(&mut encoded, &out.0[..], &opts(3, 0)).unwrap();
        let mut again = Vec::new();
        decode(&mut again, &encoded[..]).unwrap();
        assert!(again == out.0, "re-encoding failed");
    }
}

#[test]
fn encode_example() {
    let mut encoded = Vec::new();
    encode(&mut encoded, Cursor::new(b"abcabcX"), &opts(3, 255)).unwrap();
    assert_eq!(
        encoded,
        [
            0x4c, 0x5a, 0x37, 0x37, 0x01, 0x03, 0xff, 0x01, 0x01, 0x02, 0x00, 0x00, 0x61, 0x00, 0x62, 0x00, 0x63, 0x03,
            0x02, 0x58
        ]
    );
    let mut decoded = Vec::new();
    decode(&mut decoded, &encoded[..]).unwrap();
    assert_eq!(decoded, b"abcabcX");
}
