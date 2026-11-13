use lz4opt::lz4::framejson::{self, BlockData, Frame};
use lz4opt::lz4::{self, Options, Parser};

use super::*;

fn encode(input: &[u8], options: &Options) -> Vec<u8> {
    let mut out = Vec::new();
    lz4::encode(&mut out, input, options).unwrap();
    out
}

#[test]
fn summarize_counts() {
    let cases: [(&str, Vec<u8>, Counts); 3] = [
        ("empty", vec![], Counts { frames: 1, ..Default::default() }),
        (
            "raw",
            b"abcdef".to_vec(),
            Counts { frames: 1, blocks: 1, uncompressed_blocks: 1, uncompressed_bytes: 6, ..Default::default() },
        ),
        (
            "match",
            b"abcd".repeat(6),
            Counts {
                frames: 1,
                blocks: 1,
                compressed_blocks: 1,
                sequences: 2,
                matches: 1,
                literal_runs: 2,
                literal_bytes: 9,
                match_bytes: 15,
                ..Default::default()
            },
        ),
    ];
    for (name, input, want) in cases {
        let s = summarize(&encode(&input, &Options::default())[..]).unwrap();
        assert_eq!(s.counts, want, "{name}: counts");
        if name == "match" {
            let h = |name: &str, key: u64| s.histograms[name].get(&key).copied().unwrap_or(0);
            assert!(
                h("match_length", 15) == 1
                    && h("match_offset", 4) == 1
                    && h("literal_length", 4) == 1
                    && h("literal_length", 5) == 1,
                "unexpected histograms: {:?}",
                s.histograms
            );
            assert!(
                s.block_details[0].decoded_bytes == 24 && s.block_details[0].longest_match == 15,
                "unexpected block: {:?}",
                s.block_details[0]
            );
        }
    }
}

#[test]
fn summarize_rejects_invalid_frame() {
    assert!(summarize(&b"invalid"[..]).is_err(), "invalid frame accepted");
}

/// Reference implementation that builds the lz4frame-json model; the streaming
/// `summarize` must agree with it.
fn summarize_model(r: &[u8]) -> Stats {
    let root = framejson::parse(r).unwrap();
    let mut s = Stats::new();
    for (fi, item) in root.frames.iter().enumerate() {
        let Frame::Normal(f) = item else { panic!("not a normal frame") };
        s.block_max_bytes = f.frame_descriptor.bd.block_max_size_bytes;
        s.counts.frames += 1;
        for (bi, b) in f.data_blocks.iter().enumerate() {
            let mut bs = BlockStats {
                frame: fi,
                block: bi,
                uncompressed: b.block_size.uncompressed,
                payload_bytes: b.block_size.data_size,
                ..Default::default()
            };
            s.counts.blocks += 1;
            if bs.uncompressed {
                s.counts.uncompressed_blocks += 1;
                s.counts.uncompressed_bytes += u64::from(bs.payload_bytes);
                bs.decoded_bytes = u64::from(bs.payload_bytes);
            } else {
                s.counts.compressed_blocks += 1;
                for data in &b.data {
                    let BlockData::Sequence(seq) = data else { panic!("not a sequence") };
                    bs.sequences += 1;
                    bs.literal_bytes += seq.literal_length;
                    bs.decoded_bytes += seq.literal_length;
                    s.count("literal_length", seq.literal_length);
                    if seq.literal_length > 0 {
                        s.counts.literal_runs += 1;
                    }
                    if let Some(n) = seq.match_length {
                        bs.matches += 1;
                        bs.decoded_bytes += n;
                        bs.longest_match = bs.longest_match.max(n);
                        s.counts.match_bytes += n;
                        s.count("match_length", n);
                        s.count("match_offset", u64::from(seq.match_offset.as_ref().unwrap().value));
                    }
                }
            }
            s.add_block(bs);
        }
    }
    s
}

#[test]
fn summarize_matches_model() {
    let mut rng = test_rng(1);
    let random: Vec<u8> = (0..300_000).map(|_| rng.below(256) as u8).collect();
    let mut text = Vec::new();
    while text.len() < 600_000 {
        // Words from a small vocabulary give many short and long matches,
        // including literal and match lengths of 15 and more.
        let n = 1 + rng.below(40);
        text.resize(text.len() + n, b'a' + rng.below(6) as u8);
        let start = rng.below(random.len() - 300);
        text.extend_from_slice(&random[start..start + rng.below(300)]);
    }
    let inputs: [(&str, Vec<u8>); 3] = [("random", random), ("text", text), ("runs", b"ab".repeat(200_000))];
    for (name, input) in &inputs {
        for parser in [Parser::Greedy, Parser::Optimal] {
            for size in [lz4::BLOCK_64_KIB, lz4::BLOCK_4_MIB] {
                let mut frames = encode(input, &Options { parser, block_size: size });
                frames.extend(encode(input, &Options { parser, block_size: size })); // two concatenated frames
                let got = summarize(&frames[..]).unwrap_or_else(|err| panic!("{name}/{parser:?}/{size}: {err}"));
                let want = summarize_model(&frames);
                assert!(got == want, "{name}/{parser:?}/{size}: streaming summary differs from the model");
            }
        }
    }
}

#[test]
fn summarize_rejects_truncated_frame() {
    let b = encode(&b"abcd".repeat(100), &Options::default());
    for n in 1..b.len() {
        assert!(summarize(&b[..n]).is_err(), "frame truncated to {n} of {} bytes accepted", b.len());
    }
}

/// A small deterministic generator (SplitMix64) for reproducible random inputs.
/// It duplicates `testutil`, which a binary's tests cannot reach because it is
/// private to the library crate.
struct TestRng(u64);

impl TestRng {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) % n as u64) as usize
    }
}

fn test_rng(seed: u64) -> TestRng {
    TestRng(seed)
}
