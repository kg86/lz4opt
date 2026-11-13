//! Times the sliding suffix tree work behind the lz4 encoder on a fixed
//! workload, so revisions of the tree can be compared. For every position of
//! each input file it computes the longest match whose source starts at most
//! `window` bytes earlier, and prints one JSON line per file and repeat.
//!
//! Only the matching is timed, not reading the file. Instead of storing the
//! matches, it folds them into a checksum: FNV-1a 64 over, for each position
//! in order, the match length and then the offset, each as 8 little-endian
//! bytes. A faithful implementation prints the same checksum.
//!
//! usage: matchbench [-window n] [-repeats n] file...

use std::time::Instant;

use lz4opt::lz4::{for_each_longest_match, MAX_OFFSET};
use serde::Serialize;

#[path = "common/measure.rs"]
mod measure;
use measure::{input_name, ns_per_byte};

#[path = "common/fnv.rs"]
mod fnv;
use fnv::Fnv1a;

#[path = "common/cli.rs"]
mod cli;

#[path = "common/diagnostics.rs"]
mod diagnostics;
use diagnostics::{fail, usage};

const USAGE: &str = "usage: matchbench [-window n] [-repeats n] file...";

#[derive(Serialize)]
struct Record<'a> {
    file: &'a str,
    input_bytes: usize,
    window: usize,
    repeat: usize,
    seconds: f64,
    ns_per_byte: f64,
    checksum: String,
}

fn main() {
    let (window, repeats, files) = parse_args(&std::env::args().skip(1).collect::<Vec<_>>());
    for path in &files {
        let data = std::fs::read(path).unwrap_or_else(|err| fail(format!("matchbench: {path}: {err}")));
        let name = input_name(path);
        for repeat in 1..=repeats {
            let mut h = Fnv1a::new();
            let start = Instant::now();
            for_each_longest_match(&data, window, |length, offset| h.write_match(length, offset));
            let elapsed = start.elapsed();
            let record = Record {
                file: &name,
                input_bytes: data.len(),
                window,
                repeat,
                seconds: elapsed.as_secs_f64(),
                ns_per_byte: ns_per_byte(elapsed, data.len()),
                checksum: format!("{:016x}", h.sum()),
            };
            println!("{}", serde_json::to_string(&record).unwrap_or_else(|err| fail(format!("matchbench: {err}"))));
        }
    }
}

/// Parses the flags with [`cli::parse_flags`], followed by the files.
fn parse_args(args: &[String]) -> (usize, usize, Vec<String>) {
    let (mut window, mut repeats) = (MAX_OFFSET, 1);
    let files = cli::parse_flags(args, |name, value| {
        match name {
            "window" => window = value.parse().ok()?,
            "repeats" => repeats = value.parse().ok()?,
            _ => return None,
        }
        Some(())
    })
    .unwrap_or_else(|| usage(USAGE));
    if files.is_empty() || window == 0 || repeats == 0 {
        usage(USAGE);
    }
    (window, repeats, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the checksum: ties between equally long matches are broken by the
    /// tree, so a faithful implementation prints this value too.
    #[test]
    fn checksum_pinned() {
        let mut h = Fnv1a::new();
        for_each_longest_match(b"abracadabra abracadabra", MAX_OFFSET, |l, o| h.write_match(l, o));
        assert_eq!(h.sum(), 0xd0f47c156bad6d4c);
    }
}
