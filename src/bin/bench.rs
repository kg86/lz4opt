//! Times the encoders built on the sliding suffix tree, to compare revisions
//! of this crate. For every workload and input it runs the workload `-repeats`
//! times and prints one JSON line per run with the elapsed time and a hash of
//! the output. A change that only improves speed leaves the hashes unchanged;
//! a hash that differs shows that the output changed.
//!
//! Workloads (all in memory; reading the input is not timed):
//!
//! - `lz77`: `lz77::encode` without entropy coding, window 65536.
//! - `lz77-huffman`: the same with Huffman coding.
//! - `match`: only the matching behind the lz4 encoder, as in `matchbench`
//!   (window 65535). Its output is the 8-byte FNV-1a checksum of the matches (so the printed hash is a hash of that checksum, not `matchbench`'s value).
//!   Not run by default.
//! - `lz77-decode`, `lz77-huffman-decode`: `lz77::decode` of the output of the
//!   corresponding encoding workload, which is made untimed. Not run by default.
//! - `lz4-greedy`, `lz4-optimal`: `lz4::encode` with 4 MiB blocks (`-block-size`).
//! - `framejson`: `lz4::framejson::parse` and `Root::to_json` of a greedy frame
//!   with 64 KiB blocks. Its JSON is about 50 times larger than the input, so
//!   it is not run by default; limit the input with `-max-bytes`.
//!
//! The hash is FNV-1a 64 over the output bytes. With `-synthetic n`, the
//! inputs are `unary` (all `a`), `periodic` (`abracadabra` repeated), and
//! `random` (SplitMix64 bytes, seed 1), each `n` bytes long, in place of or in
//! addition to files.
//!
//! usage: bench [-workload name]... [-repeats n] [-block-size n] [-max-bytes n] [-synthetic n] [file...]

use std::time::Instant;

use lz4opt::lz4::{self, framejson};
use lz4opt::lz77::{self, EncodeOptions, Entropy, Options};
use serde::Serialize;

#[path = "common/cli.rs"]
mod cli;

#[path = "common/measure.rs"]
mod measure;
use measure::{input_name, ns_per_byte};

#[path = "common/fnv.rs"]
mod fnv;
use fnv::Fnv1a;

#[path = "common/diagnostics.rs"]
mod diagnostics;
use diagnostics::{fail, usage};

const USAGE: &str =
    "usage: bench [-workload name]... [-repeats n] [-block-size n] [-max-bytes n] [-synthetic n] [file...]";

const WORKLOADS: [&str; 8] =
    ["lz77", "lz77-huffman", "lz4-greedy", "lz4-optimal", "framejson", "lz77-decode", "lz77-huffman-decode", "match"];

/// The workloads that run when none is named.
const DEFAULT_WORKLOADS: [&str; 4] = ["lz77", "lz77-huffman", "lz4-greedy", "lz4-optimal"];

struct Config {
    workloads: Vec<String>,
    repeats: usize,
    block_size: usize,
    max_bytes: Option<usize>,
    synthetic: Option<usize>,
    files: Vec<String>,
}

#[derive(Serialize)]
struct Record<'a> {
    bench: &'a str,
    input: &'a str,
    input_bytes: usize,
    repeat: usize,
    seconds: f64,
    ns_per_byte: f64,
    output_bytes: usize,
    output_hash: String,
}

fn main() {
    let config = parse_args(&std::env::args().skip(1).collect::<Vec<_>>());
    let mut inputs: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(n) = config.synthetic {
        inputs.extend(synthetic_inputs(n));
    }
    for path in &config.files {
        let data = std::fs::read(path).unwrap_or_else(|err| fail(format!("{path}: {err}")));
        inputs.push((input_name(path), data));
    }
    if let Some(max) = config.max_bytes {
        inputs.iter_mut().for_each(|(_, data)| data.truncate(max));
    }

    for workload in &config.workloads {
        for (name, data) in &inputs {
            let prepared = prepare(workload, data, &config);
            for repeat in 1..=config.repeats {
                let start = Instant::now();
                let output = run(workload, data, prepared.as_deref(), &config);
                let elapsed = start.elapsed();
                let mut hash = Fnv1a::new();
                hash.write(&output);
                let record = Record {
                    bench: workload,
                    input: name,
                    input_bytes: data.len(),
                    repeat,
                    seconds: elapsed.as_secs_f64(),
                    ns_per_byte: ns_per_byte(elapsed, data.len()),
                    output_bytes: output.len(),
                    output_hash: format!("{:016x}", hash.sum()),
                };
                println!("{}", serde_json::to_string(&record).unwrap_or_else(|err| fail(err)));
            }
        }
    }
}

/// Returns the untimed setup a workload needs: the LZ4 frame that `framejson`
/// parses, or the encoded stream that a decode workload decodes.
fn prepare(workload: &str, data: &[u8], config: &Config) -> Option<Vec<u8>> {
    match workload {
        "framejson" => {
            let mut frame = Vec::new();
            let options = lz4::Options { block_size: lz4::BLOCK_64_KIB, parser: lz4::Parser::Greedy };
            lz4::encode(&mut frame, data, &options).unwrap_or_else(|err| fail(err));
            Some(frame)
        }
        "lz77-decode" => Some(run("lz77", data, None, config)),
        "lz77-huffman-decode" => Some(run("lz77-huffman", data, None, config)),
        _ => None,
    }
}

/// Runs one workload on `data` and returns its output.
fn run(workload: &str, data: &[u8], prepared: Option<&[u8]>, config: &Config) -> Vec<u8> {
    let mut out = Vec::new();
    match workload {
        "lz77" | "lz77-huffman" => {
            // Like lz77 encode, limit options to what the input can use.
            let options = Options { window: 65536, max_length: 0 }.fit_to_size(data.len() as u64);
            let entropy = if workload == "lz77" { Entropy::None } else { Entropy::Huffman };
            let opts = EncodeOptions { options, entropy, ..Default::default() };
            lz77::encode(&mut out, data, &opts).unwrap_or_else(|err| fail(err));
        }
        "lz4-greedy" | "lz4-optimal" => {
            let parser = if workload == "lz4-greedy" { lz4::Parser::Greedy } else { lz4::Parser::Optimal };
            lz4::encode(&mut out, data, &lz4::Options { block_size: config.block_size, parser })
                .unwrap_or_else(|err| fail(err));
        }
        "match" => {
            let mut sum = Fnv1a::new();
            lz4::for_each_longest_match(data, lz4::MAX_OFFSET, |length, offset| sum.write_match(length, offset));
            out = sum.sum().to_le_bytes().to_vec();
        }
        "lz77-decode" | "lz77-huffman-decode" => {
            let encoded = prepared.expect("decode workloads are prepared");
            lz77::decode(&mut out, encoded).unwrap_or_else(|err| fail(err));
        }
        "framejson" => {
            let frame = prepared.expect("framejson is prepared");
            let root = framejson::parse(frame).unwrap_or_else(|err| fail(err));
            out = root.to_json().into_bytes();
        }
        _ => unreachable!("workloads are validated while parsing the arguments"),
    }
    out
}

/// Returns the three synthetic inputs of `n` bytes.
fn synthetic_inputs(n: usize) -> Vec<(String, Vec<u8>)> {
    let periodic: Vec<u8> = b"abracadabra".iter().copied().cycle().take(n).collect();
    let mut state = 1u64;
    let random: Vec<u8> = (0..n)
        .map(|_| {
            // SplitMix64.
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            (z ^ (z >> 31)) as u8
        })
        .collect();
    vec![("unary".to_string(), vec![b'a'; n]), ("periodic".to_string(), periodic), ("random".to_string(), random)]
}

/// Parses the flags with [`cli::parse_flags`], followed by the input files.
fn parse_args(args: &[String]) -> Config {
    let mut config = Config {
        workloads: Vec::new(),
        repeats: 3,
        block_size: lz4::BLOCK_4_MIB,
        max_bytes: None,
        synthetic: None,
        files: Vec::new(),
    };
    config.files = cli::parse_flags(args, |name, value| {
        match name {
            "workload" => {
                if !WORKLOADS.contains(&value) {
                    usage(format!("unknown workload {value:?}; choose from {}", WORKLOADS.join(", ")));
                }
                config.workloads.push(value.to_string());
            }
            "repeats" => config.repeats = value.parse().ok()?,
            "block-size" => config.block_size = value.parse().ok()?,
            "max-bytes" => config.max_bytes = Some(value.parse().ok()?),
            "synthetic" => config.synthetic = Some(value.parse().ok()?),
            _ => return None,
        }
        Some(())
    })
    .unwrap_or_else(|| usage(USAGE));
    lz4::Options { block_size: config.block_size, ..Default::default() }.validate().unwrap_or_else(|err| usage(err));
    if config.workloads.is_empty() {
        config.workloads = DEFAULT_WORKLOADS.iter().map(|w| w.to_string()).collect();
    }
    if config.repeats == 0 || (config.files.is_empty() && config.synthetic.is_none()) {
        usage(USAGE);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            workloads: Vec::new(),
            repeats: 1,
            block_size: lz4::BLOCK_64_KIB,
            max_bytes: None,
            synthetic: None,
            files: Vec::new(),
        }
    }

    #[test]
    fn synthetic_inputs_are_deterministic() {
        let (a, b) = (synthetic_inputs(1000), synthetic_inputs(1000));
        assert_eq!(a, b);
        assert_eq!(a.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["unary", "periodic", "random"]);
        assert!(a.iter().all(|(_, data)| data.len() == 1000));
        assert_eq!(&a[1].1[..12], b"abracadabraa");
    }

    #[test]
    fn workloads_are_deterministic_and_decodable() {
        let config = config();
        for (name, data) in synthetic_inputs(5000) {
            for workload in WORKLOADS {
                let prepared = prepare(workload, &data, &config);
                let first = run(workload, &data, prepared.as_deref(), &config);
                let second = run(workload, &data, prepared.as_deref(), &config);
                assert!(!first.is_empty() && first == second, "{workload}/{name}: output differs between runs");
            }
            // The decode workloads restore the input.
            for workload in ["lz77-decode", "lz77-huffman-decode"] {
                let prepared = prepare(workload, &data, &config);
                assert!(run(workload, &data, prepared.as_deref(), &config) == data, "{workload}/{name}");
            }
            // The lz77 outputs decode to the input.
            for workload in ["lz77", "lz77-huffman"] {
                let encoded = run(workload, &data, None, &config);
                let mut decoded = Vec::new();
                lz77::decode(&mut decoded, &encoded[..]).unwrap();
                assert!(decoded == data, "{workload}/{name}: round trip differs");
            }
        }
    }
}
