use std::io::{self, Read, Write};
use std::process::{Command, Stdio};

use super::block::{append_last_literals, append_sequence, encode_block};
use super::matches::{longest_matches, Match, MAX_OFFSET};
use super::xxhash::{checksum, Xxh32};
use super::*;
use crate::testutil::Rng;

// XXH32

#[test]
fn xxh32() {
    let cases: [(&str, u32); 6] = [
        ("", 0x02cc5d05),
        ("a", 0x550d7456),
        ("abc", 0x32d153ff),
        ("message digest", 0x7c948494),
        ("abcdefghijklmnopqrstuvwxyz", 0x63a14d5f),
        ("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", 0x9c285e64),
    ];
    for (input, want) in cases {
        let input = input.as_bytes();
        for step in 1..=input.len() + 1 {
            let mut h = Xxh32::new();
            for chunk in input.chunks(step) {
                h.write(chunk);
                h.write(&[]);
                let _ = h.sum32(); // Taking a snapshot must not finalize the state.
            }
            let got = h.sum32();
            assert!(got == want && h.sum32() == got, "input {input:?} chunk {step}: {got:08x}, want {want:08x}");
        }
    }
}

#[test]
fn xxh32_length_wrap() {
    // The stripe flag must survive wrapping the 32-bit length counter.
    let mut h = Xxh32::new();
    h.write(&[0; 16]);
    h.set_length(u32::MAX - 3);
    h.write(&[1, 2, 3, 4]);
    assert!(h.length() == 0 && h.striped(), "incorrect length wrap state");
    let mut copy = h.clone();
    copy.clear_striped();
    assert_ne!(h.sum32(), copy.sum32(), "wrapped length lost the stripe accumulators");
}

// Longest matches

/// Tries every source directly, independently of the suffix tree.
fn reference_matches(src: &[u8], window: usize) -> Vec<Match> {
    let mut out = vec![Match::default(); src.len()];
    for i in 0..src.len() {
        for s in i.saturating_sub(window)..i {
            let mut length = 0;
            while i + length < src.len() && src[s + length] == src[i + length] {
                length += 1;
            }
            if length > out[i].length {
                out[i] = Match { length, offset: i - s };
            }
        }
    }
    out
}

fn check_matches(src: &[u8], window: usize) {
    let (got, want) = (longest_matches(src, window), reference_matches(src, window));
    for (i, m) in got.iter().enumerate() {
        assert_eq!(m.length, want[i].length, "input {src:02x?} window {window} position {i}: match {m:?}");
        if m.length == 0 {
            assert_eq!(m.offset, 0, "position {i}: zero length with offset {}", m.offset);
            continue;
        }
        assert!(m.offset >= 1 && m.offset <= i.min(window), "position {i}: invalid match {m:?}");
        assert!(
            src[i - m.offset..i - m.offset + m.length] == src[i..i + m.length],
            "position {i}: source does not match {m:?}"
        );
    }
}

#[test]
fn longest_matches_against_reference() {
    for n in 0..=9 {
        for mask in 0..1u32 << n {
            let src: Vec<u8> = (0..n).map(|i| (mask >> i & 1) as u8).collect();
            for window in [1, 2, 5, MAX_OFFSET] {
                check_matches(&src, window);
            }
        }
    }
    let mut rng = Rng(17);
    for trial in 0..200 {
        let alphabet = [2, 5, 256][trial % 3];
        let n = rng.below(300);
        let src: Vec<u8> = (0..n).map(|_| rng.below(alphabet) as u8).collect();
        check_matches(&src, 1 + rng.below(40));
        check_matches(&src, MAX_OFFSET);
    }
    for src in [&b"a".repeat(1024)[..], &b"abc".repeat(128), b"abcabcXabcabcYabcabcX"] {
        check_matches(src, 7);
    }
}

#[test]
fn offset_limit() {
    for distance in [MAX_OFFSET, MAX_OFFSET + 1] {
        // Only positions 0 and distance begin with 0xff.
        let mut src = vec![b'a'; distance + 20];
        src[..8].copy_from_slice(&[255, 1, 2, 3, 4, 5, 6, 7]);
        src.copy_within(..8, distance);
        let got = longest_matches(&src, MAX_OFFSET)[distance];
        if distance == MAX_OFFSET {
            assert!(got.offset == distance && got.length == 20, "boundary match = {got:?}");
        } else {
            assert_eq!(got, Match::default(), "out-of-window source was used");
        }
    }
}

// Blocks

#[derive(Debug)]
struct Sequence {
    start: usize,
    literals: usize,
    offset: usize,
    length: usize,
}

/// Decodes test output independently and records the parse for greedy checks.
/// A per-block output bound also catches invalid lengths before allocation.
fn decode_block(src: &[u8], limit: usize) -> (Vec<u8>, Vec<Sequence>) {
    let mut out = Vec::new();
    let mut sequences = Vec::new();
    let mut pos = 0;
    let length = |pos: &mut usize, base: usize| {
        let mut n = base;
        if base == 15 {
            loop {
                assert!(*pos < src.len(), "truncated length extension");
                let c = src[*pos];
                *pos += 1;
                n += c as usize;
                assert!(n <= limit, "length exceeds output bound");
                if c != 255 {
                    break;
                }
            }
        }
        n
    };
    while pos < src.len() {
        let token = src[pos];
        pos += 1;
        let r = length(&mut pos, (token >> 4) as usize);
        assert!(r <= src.len() - pos && r <= limit - out.len(), "invalid literal length");
        let mut s = Sequence { start: out.len(), literals: r, offset: 0, length: 0 };
        out.extend_from_slice(&src[pos..pos + r]);
        pos += r;
        if pos == src.len() {
            assert!(token & 15 == 0 && r >= 5.min(out.len()), "invalid final literal sequence");
            sequences.push(s);
            return (out, sequences);
        }
        assert!(src.len() - pos >= 2, "truncated offset");
        s.offset = u16::from_le_bytes([src[pos], src[pos + 1]]) as usize;
        pos += 2;
        s.length = 4 + length(&mut pos, (token & 15) as usize);
        assert!(s.offset != 0 && s.offset <= out.len() && s.length <= limit - out.len(), "invalid match {s:?}");
        for _ in 0..s.length {
            out.push(out[out.len() - s.offset]);
        }
        sequences.push(s);
    }
    panic!("missing final literal sequence");
}

fn check_greedy_block(src: &[u8]) -> Vec<Sequence> {
    let (out, got) = decode_block(&encode_block(src, Parser::Greedy), src.len());
    assert!(out == src, "block round trip differs");
    // Construct the expected parse directly from an exhaustive match oracle.
    let best = reference_matches(src, MAX_OFFSET);
    let mut want = Vec::new();
    let (mut anchor, mut i) = (0, 0);
    while i < src.len() {
        if src.len() - i < 12 {
            i += 1;
            continue;
        }
        let m = best[i].length.min(src.len() - 5 - i);
        if m < 4 {
            i += 1;
            continue;
        }
        want.push((anchor, i - anchor, m));
        i += m;
        anchor = i;
    }
    want.push((anchor, src.len() - anchor, 0));
    let got_triples: Vec<_> = got.iter().map(|s| (s.start, s.literals, s.length)).collect();
    assert_eq!(got_triples, want, "parse of {src:?}");
    got
}

#[test]
fn greedy_block() {
    for n in 0..40 {
        check_greedy_block(&b"a".repeat(n));
    }
    let mut rng = Rng(9);
    for trial in 0..200 {
        let n = rng.below(200);
        let src: Vec<u8> = (0..n).map(|_| rng.below(2 + trial % 17) as u8).collect();
        check_greedy_block(&src);
    }
    // A short match at 10 must not skip the useful match at 11.
    let got = check_greedy_block(b"abcXbcdefYabcdefZ1234567");
    assert!(got[0].literals == 11 && got[0].length == 5, "short match skipped a parse position: {got:?}");
    // A match may be immediately followed by another match, without a literal.
    let got = check_greedy_block(b"abcdXefghYabcdefghZ123456789012");
    assert!(got.len() >= 3 && got[1].literals == 0 && got[1].length == 4, "adjacent matches not selected: {got:?}");
    // Only the occurrence exactly 12 bytes before the end may start a match.
    for tail in [7, 8] {
        let mut src = b"abcdXabcd".to_vec();
        src.extend(vec![b'z'; tail]);
        let got = check_greedy_block(&src);
        assert_eq!(got[0].length == 4, tail == 8, "tail {tail}: unexpected boundary parse {got:?}");
    }
}

#[test]
fn sequence_bytes() {
    let cases: [(usize, &[u8]); 6] =
        [(4, &[]), (18, &[]), (19, &[0]), (273, &[254]), (274, &[255, 0]), (529, &[255, 255, 0])];
    for (length, ext) in cases {
        for offset in [1, MAX_OFFSET] {
            let mut got = Vec::new();
            append_sequence(&mut got, b"a", offset, length);
            let mut want = vec![0x10 | (length - 4).min(15) as u8, b'a', offset as u8, (offset >> 8) as u8];
            want.extend_from_slice(ext);
            assert!(got == want, "length {length} offset {offset}: {got:02x?}, want {want:02x?}");
        }
    }
    let cases: [(usize, &[u8]); 5] =
        [(14, &[0xe0]), (15, &[0xf0, 0]), (269, &[0xf0, 254]), (270, &[0xf0, 255, 0]), (525, &[0xf0, 255, 255, 0])];
    for (length, header) in cases {
        let literals = vec![b'x'; length];
        let mut want = header.to_vec();
        want.extend_from_slice(&literals);
        let mut got = Vec::new();
        append_last_literals(&mut got, &literals);
        assert!(got == want, "literal length {length}: incorrect encoding");
        want.extend_from_slice(&[1, 0]);
        let mut got = Vec::new();
        append_sequence(&mut got, &literals, 1, 4);
        assert!(got == want, "sequence literal length {length}: incorrect encoding");
    }
}

/// Checks `extension` against the bytes the encoder writes for literal runs and
/// matches, and the slack against `extension`.
#[test]
fn length_extension_helpers() {
    use super::format::length_extension_slack;
    let literal_extension = |len: usize| {
        let mut dst = Vec::new();
        append_last_literals(&mut dst, &vec![b'x'; len]);
        dst.len() - 1 - len
    };
    let match_extension = |code: usize| {
        let mut dst = Vec::new();
        append_sequence(&mut dst, b"", 1, code + 4);
        dst.len() - 3
    };
    for len in 0..1200 {
        assert_eq!(extension(len), literal_extension(len), "literal length {len}");
        assert_eq!(extension(len), match_extension(len), "match code {len}");
        let slack = length_extension_slack(len);
        assert!(slack >= 1, "length {len}: zero slack");
        for grow in 1..slack {
            assert_eq!(extension(len + grow), extension(len), "length {len} + {grow}");
        }
        assert_eq!(extension(len + slack), extension(len) + 1, "length {len} + {slack}");
    }
}

// Frames

struct FrameBlock {
    raw: bool,
    payload: Vec<u8>,
}

/// Inspects the supported frame subset and decodes each block with fresh
/// history.
fn inspect_frame(frame: &[u8], size: usize) -> (Vec<u8>, Vec<FrameBlock>) {
    assert!(frame.len() >= 15 && frame[..4] == [0x04, 0x22, 0x4d, 0x18] && frame[4] == 0x64, "incorrect frame header");
    let want_id = match size {
        BLOCK_64_KIB => 4,
        BLOCK_256_KIB => 5,
        BLOCK_1_MIB => 6,
        _ => 7,
    };
    assert!(
        frame[5] == want_id << 4 && frame[6] == (checksum(&frame[4..6]) >> 8) as u8,
        "incorrect block size or header checksum"
    );
    let mut p = &frame[7..];
    let mut out = Vec::new();
    let mut blocks = Vec::new();
    loop {
        assert!(p.len() >= 4, "missing EndMark");
        let field = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
        p = &p[4..];
        if field == 0 {
            break;
        }
        let n = (field & 0x7fff_ffff) as usize;
        assert!(n != 0 && n <= size && n <= p.len(), "invalid data block size {n}");
        let raw = field >> 31 != 0;
        let payload = p[..n].to_vec();
        p = &p[n..];
        let decoded = if raw {
            payload.clone()
        } else {
            let (decoded, sequences) = decode_block(&payload, size);
            for s in sequences {
                assert!(
                    s.length == 0 || s.start + s.literals <= decoded.len() - 12,
                    "match starts too close to the block end"
                );
            }
            assert!(payload.len() < decoded.len(), "compressed block did not save space");
            decoded
        };
        out.extend_from_slice(&decoded);
        blocks.push(FrameBlock { raw, payload });
    }
    assert!(
        p.len() == 4 && u32::from_le_bytes([p[0], p[1], p[2], p[3]]) == checksum(&out),
        "incorrect content checksum or trailing bytes"
    );
    (out, blocks)
}

fn encode_frame(src: &[u8], block_size: usize) -> Vec<u8> {
    encode_frame_with(src, &Options { block_size, ..Default::default() })
}

fn encode_frame_with(src: &[u8], options: &Options) -> Vec<u8> {
    let mut dst = Vec::new();
    encode(&mut dst, src, options).unwrap();
    dst
}

const BLOCK_SIZES: [usize; 4] = [BLOCK_64_KIB, BLOCK_256_KIB, BLOCK_1_MIB, BLOCK_4_MIB];

#[test]
fn frame_boundaries() {
    for size in BLOCK_SIZES {
        for n in [0, 1, size - 1, size, size + 1] {
            let src = vec![b'a'; n];
            let (out, blocks) = inspect_frame(&encode_frame(&src, size), size);
            assert!(
                out == src && blocks.len() == n.div_ceil(size),
                "size {size} n {n}: incorrect contents or block count"
            );
            for (i, block) in blocks.iter().enumerate() {
                let decoded = if block.raw { block.payload.len() } else { decode_block(&block.payload, size).0.len() };
                assert_eq!(decoded, size.min(n - i * size), "incorrect fixed block boundary");
            }
        }
    }
    assert_eq!(encode_frame(&[], 0), encode_frame(&[], BLOCK_4_MIB), "default block size is not 4 MiB");
}

#[test]
fn block_selection() {
    // These cases produce payload deltas -1, 0, +1 while respecting tail rules.
    for (src, delta) in [("abcdeXabcde1234567", -1), ("abcdXabcd12345678", 0), ("abcdefghijklm", 1)] {
        let src = src.as_bytes();
        assert_eq!(encode_block(src, Parser::Greedy).len() as isize - src.len() as isize, delta, "fixture {src:?}");
        let (out, blocks) = inspect_frame(&encode_frame(src, 0), BLOCK_4_MIB);
        assert!(
            out == src && blocks.len() == 1 && blocks[0].raw == (delta >= 0),
            "incorrect block selection for {src:?}"
        );
    }
}

#[test]
fn independent_blocks() {
    let mut rng = Rng(5);
    let mut src: Vec<u8> = (0..BLOCK_64_KIB).map(|_| rng.next_u64() as u8).collect();
    // The second block copies the preceding 65,535 bytes. It would compress
    // with leaked history; copying all 65,536 bytes would exceed MAX_OFFSET.
    let tail = src[1..].to_vec();
    src.extend(tail);
    let (out, blocks) = inspect_frame(&encode_frame(&src, BLOCK_64_KIB), BLOCK_64_KIB);
    assert!(
        out == src && blocks.len() == 2 && blocks[0].raw && blocks[1].raw,
        "independent random blocks should both be stored raw"
    );
    let (_, standalone) = inspect_frame(&encode_frame(&src[BLOCK_64_KIB..], BLOCK_64_KIB), BLOCK_64_KIB);
    assert!(blocks[1].payload == standalone[0].payload, "block encoding depends on preceding history");
}

/// Reads at most one byte at a time.
struct OneByteReader<'a>(&'a [u8]);

impl Read for OneByteReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(1).min(self.0.len());
        buf[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

/// Reads at most half of what is asked for.
struct HalfReader<'a>(&'a [u8]);

impl Read for HalfReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().div_ceil(2).min(self.0.len());
        buf[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

#[test]
fn read_boundaries() {
    let src = b"abcd".repeat(BLOCK_64_KIB / 4 + 9);
    let want = encode_frame(&src, BLOCK_64_KIB);
    let options = Options { block_size: BLOCK_64_KIB, ..Default::default() };
    for reader in [Box::new(OneByteReader(&src)) as Box<dyn Read>, Box::new(HalfReader(&src))] {
        let mut dst = Vec::new();
        encode(&mut dst, reader, &options).unwrap();
        assert!(dst == want, "read boundaries changed the frame");
    }
}

/// Returns a partial read, then an error.
struct FailingReader(bool);

impl Read for FailingReader {
    fn read(&mut self, p: &mut [u8]) -> io::Result<usize> {
        if self.0 {
            return Err(io::Error::other("test I/O failure"));
        }
        self.0 = true;
        let n = p.len().min(7);
        p[..n].copy_from_slice(&b"partial"[..n]);
        Ok(n)
    }
}

/// Accepts `remaining` bytes, then either fails or stops accepting bytes.
struct FailingWriter {
    remaining: usize,
    fail: bool,
}

impl Write for FailingWriter {
    fn write(&mut self, p: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return if self.fail { Err(io::Error::other("test I/O failure")) } else { Ok(0) };
        }
        let n = p.len().min(self.remaining);
        self.remaining -= n;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn io_errors() {
    let options = Options { block_size: BLOCK_64_KIB, ..Default::default() };
    let mut dst = Vec::new();
    assert!(matches!(encode(&mut dst, FailingReader(false), &options), Err(Error::Read(_))), "read error");
    assert_eq!(dst.len(), 7, "read error should leave only the header, without an EndMark");
    // Fail within the header, block-size field, payload, and footer.
    for remaining in [0, 3, 7, 9, 11, 12, 14, 18, 21] {
        for fail in [false, true] {
            let writer = FailingWriter { remaining, fail };
            match encode(writer, &b"abc"[..], &options) {
                Err(Error::Write(err)) if fail => assert_eq!(err.to_string(), "test I/O failure"),
                Err(Error::Write(err)) if !fail => assert_eq!(err.kind(), io::ErrorKind::WriteZero),
                other => panic!("write at {remaining} (fail {fail}) = {other:?}"),
            }
        }
    }
    // The writer panics if touched, which shows that option validation
    // precedes any I/O.
    struct Untouchable;
    impl Write for Untouchable {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            panic!("I/O before option validation")
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    for size in [1, BLOCK_64_KIB - 1, BLOCK_4_MIB + 1, usize::MAX] {
        let err = encode(Untouchable, FailingReader(false), &Options { block_size: size, ..Default::default() });
        assert!(matches!(err, Err(Error::UnsupportedBlockSize(_))), "accepted invalid block size {size}");
    }
}

#[test]
fn encode_example() {
    let mut frame = Vec::new();
    encode(&mut frame, &b"hello"[..], &Options::default()).unwrap();
    assert_eq!(frame[..4], [0x04, 0x22, 0x4d, 0x18]);
}

/// Decodes `frame` with the official lz4 CLI, or returns `None` if it is not
/// installed.
fn official(args: &[&str], frame: &[u8]) -> Option<io::Result<std::process::Output>> {
    let mut child = match Command::new("lz4")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
        Err(err) => return Some(Err(err)),
    };
    let mut stdin = child.stdin.take().unwrap();
    let frame = frame.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&frame);
    });
    let output = child.wait_with_output();
    writer.join().unwrap();
    Some(output)
}

#[test]
fn official_decoder() {
    if official(&["-V"], &[]).is_none() {
        eprintln!("install the official lz4 CLI to run interoperability tests");
        return;
    }
    let mut rng = Rng(13);
    let random: Vec<u8> = (0..BLOCK_64_KIB).map(|_| rng.next_u64() as u8).collect();
    // Mix raw and compressed blocks, and include a partial final block.
    let mut mixed = random.clone();
    mixed.extend(vec![b'a'; BLOCK_64_KIB]);
    mixed.extend_from_slice(&random[..1003]);
    // Exercise the largest legal offset in a compressed data block.
    let mut far = vec![b'a'; MAX_OFFSET + 20];
    far[..8].copy_from_slice(&[255, 1, 2, 3, 4, 5, 6, 7]);
    far.copy_within(..8, MAX_OFFSET);
    let cases: Vec<Vec<u8>> = vec![
        vec![],
        b"abc".to_vec(),
        b"abcdXabcd12345".to_vec(),
        vec![b'a'; 1000],
        b"abcde".repeat(14000),
        random,
        mixed,
        vec![b'a'; BLOCK_4_MIB + 1],
        far,
    ];
    for parser in [Parser::Greedy, Parser::Optimal] {
        for size in BLOCK_SIZES {
            for (i, src) in cases.iter().enumerate() {
                let mut frame = encode_frame_with(src, &Options { block_size: size, parser });
                let out = official(&["-d", "-c", "-q"], &frame).unwrap().unwrap();
                assert!(out.status.success(), "{parser:?}/{size}/{i}: official decoder: {:?}", out.stderr);
                assert!(out.stdout == *src, "{parser:?}/{size}/{i}: official decoder output differs");
                // Ensure the checksum is actually checked by the external decoder.
                *frame.last_mut().unwrap() ^= 1;
                let out = official(&["-t", "-q"], &frame).unwrap().unwrap();
                assert!(!out.status.success(), "{parser:?}/{size}/{i}: official decoder accepted a corrupted checksum");
            }
        }
    }
}

// Optimal parsing

/// Returns the number of length-extension bytes for a 4-bit length field
/// holding `x`.
fn extension(x: usize) -> usize {
    if x < 15 {
        0
    } else {
        1 + (x - 15) / 255
    }
}

/// Minimizes the block size over every parse and every source, without
/// assuming that the longest match's offset serves all shorter lengths.
fn exhaustive_optimal_size(src: &[u8]) -> usize {
    let n = src.len();
    // best[p] encodes src[p..] with p starting a sequence.
    let mut best = vec![0; n + 1];
    for p in (0..=n).rev() {
        best[p] = 1 + (n - p) + extension(n - p);
        let mut i = p;
        while i + 12 <= n {
            let literals = (i - p) + extension(i - p);
            for m in 4..=n - 5 - i {
                let found = (i.saturating_sub(MAX_OFFSET)..i).any(|s| src[s..s + m] == src[i..i + m]);
                if found {
                    best[p] = best[p].min(literals + 3 + extension(m - 4) + best[i + m]);
                }
            }
            i += 1;
        }
    }
    best[0]
}

/// Evaluates the dp and g recurrences of `optimal_parse` directly: all literal run starts and all match
/// lengths, in O(n^2 + sum of match lengths) time.
#[allow(clippy::needless_range_loop)]
fn direct_optimal_size(src: &[u8], matches: &[Match]) -> usize {
    let n = src.len();
    if n < 13 {
        return 1 + n + extension(n);
    }
    const INF: usize = 1 << 60;
    let mut dp = vec![INF; n + 1];
    let mut g = vec![INF; n + 1];
    dp[0] = 0;
    for i in 0..=n {
        for p in 0..=i {
            if dp[p] < INF {
                g[i] = g[i].min(dp[p] + (i - p) + extension(i - p));
            }
        }
        if i + 12 > n {
            continue;
        }
        for m in 4..=matches[i].length.min(n - 5 - i) {
            dp[i + m] = dp[i + m].min(g[i] + 3 + extension(m - 4));
        }
    }
    1 + g[n]
}

fn check_optimal_block(src: &[u8]) -> usize {
    let encoded = encode_block(src, Parser::Optimal);
    let (out, sequences) = decode_block(&encoded, src.len());
    assert!(out == src, "block round trip differs");
    for s in &sequences[..sequences.len() - 1] {
        assert!(
            s.offset <= MAX_OFFSET
                && s.start + s.literals + 12 <= src.len()
                && s.start + s.literals + s.length + 5 <= src.len(),
            "illegal match {s:?} in block of {} bytes",
            src.len()
        );
    }
    let want = direct_optimal_size(src, &reference_matches(src, MAX_OFFSET));
    assert_eq!(encoded.len(), want, "input {src:?}: size");
    let greedy = encode_block(src, Parser::Greedy).len();
    assert!(encoded.len() <= greedy, "optimal size {} exceeds greedy size {greedy}", encoded.len());
    encoded.len()
}

#[test]
fn optimal_exhaustive() {
    let mut rng = Rng(3);
    for trial in 0..300 {
        let n = rng.below(36);
        let src: Vec<u8> = (0..n).map(|_| b'a' + rng.below(1 + trial % 4) as u8).collect();
        assert_eq!(check_optimal_block(&src), exhaustive_optimal_size(&src), "input {src:?}");
    }
}

#[test]
fn optimal_block() {
    let mut n = 0;
    while n < 700 {
        check_optimal_block(&b"a".repeat(n));
        n += 1 + n / 40;
    }
    // One literal before a long match beats two greedy matches.
    // At "abcd...", the longest match has 5 bytes and the next one 22.
    let src = b"abcdeQbcdefghijklmnopqrstuvwZabcdefghijklmnopqrstuvw0123456789012";
    let (got, greedy) = (check_optimal_block(src), encode_block(src, Parser::Greedy).len());
    assert!(got < greedy, "optimal size {got}, greedy {greedy}: expected an improvement");
    let mut rng = Rng(7);
    for trial in 0..400 {
        let n = rng.below(1500);
        let alphabet = 1 + trial % 9;
        let mut src: Vec<u8> = (0..n).map(|_| rng.below(alphabet) as u8).collect();
        // Splice long runs and copies so that extension boundaries are crossed.
        if trial % 3 == 0 && src.len() > 600 {
            let (at, length) = (rng.below(src.len() - 600), 15 + rng.below(560));
            src[at..at + length].fill(trial as u8);
        }
        if trial % 3 == 1 && src.len() > 700 {
            let noise: Vec<u8> = (0..20 + rng.below(560)).map(|_| rng.next_u64() as u8).collect();
            let at = rng.below(src.len() - noise.len());
            src[at..at + noise.len()].copy_from_slice(&noise);
        }
        check_optimal_block(&src);
    }
}
