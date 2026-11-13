//! Prints, as JSON, the number of greedy
//! LZ77 phrases and the lz77 format's encoded sizes of each input file, for a
//! window of 65,535 bytes (LZ4's largest offset) and for an unlimited window
//! (LZ76).
//!
//! usage: lz77stats file...

use std::time::Instant;

use lz4opt::lz4::MAX_OFFSET;
use lz4opt::lz77::{self, EncodeOptions, Entropy, Options, Parser};
use serde::Serialize;

#[path = "common/measure.rs"]
mod measure;
use measure::input_name;

#[path = "common/diagnostics.rs"]
mod diagnostics;
use diagnostics::{fail, usage};

#[derive(Serialize)]
struct Encoding {
    name: &'static str,
    bytes: u64,
    seconds: f64,
}

#[derive(Serialize)]
struct Measurement {
    file: String,
    input_bytes: u64,
    parse: &'static str,
    window: usize,
    phrases: u64,
    copy_phrases: u64,
    longest_copy: usize,
    parse_seconds: f64,
    encodings: Vec<Encoding>,
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        usage("usage: lz77stats file...");
    }
    for path in &files {
        let data = std::fs::read(path).unwrap_or_else(|err| fail(err));
        for (name, window) in [("lz77", MAX_OFFSET), ("lz76", 0)] {
            // Like lz77 encode, limit options to what the input can use so
            // that fixed fields are no wider than the file needs.
            let opts = Options { window, max_length: 0 }.fit_to_size(data.len() as u64);
            let mut r = measure(&data, opts).unwrap_or_else(|err| fail(format!("{path} {name}: {err}")));
            r.file = input_name(path);
            r.parse = name;
            println!("{}", serde_json::to_string(&r).unwrap_or_else(|err| fail(err)));
        }
    }
}

fn measure(data: &[u8], opts: Options) -> Result<Measurement, String> {
    let mut r = Measurement {
        file: String::new(),
        input_bytes: data.len() as u64,
        parse: "",
        window: opts.window,
        phrases: 0,
        copy_phrases: 0,
        longest_copy: 0,
        parse_seconds: 0.0,
        encodings: Vec::new(),
    };
    let start = Instant::now();
    let mut p = Parser::new(opts);
    let mut count = |t: lz77::Token| {
        r.phrases += 1;
        if t.length > 0 {
            r.copy_phrases += 1;
            r.longest_copy = r.longest_copy.max(t.length);
        }
    };
    let mut rest = data;
    while !rest.is_empty() {
        let (token, consumed) = p.push(rest);
        rest = &rest[consumed..];
        token.into_iter().for_each(&mut count);
    }
    p.finish().into_iter().for_each(&mut count);
    r.parse_seconds = start.elapsed().as_secs_f64();

    for (name, entropy) in [("none", Entropy::None), ("huffman", Entropy::Huffman)] {
        let mut out = Vec::new();
        let start = Instant::now();
        lz77::encode(&mut out, data, &EncodeOptions { options: opts, entropy, ..Default::default() })
            .map_err(|err| err.to_string())?;
        let seconds = start.elapsed().as_secs_f64();
        let mut decoded = Vec::with_capacity(data.len());
        lz77::decode(&mut decoded, &out[..]).map_err(|err| format!("decode {name}: {err}"))?;
        if decoded != data {
            return Err(format!("decode {name}: output differs from input"));
        }
        r.encodings.push(Encoding { name, bytes: out.len() as u64, seconds });
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_counts_phrases_and_verifies_decoding() {
        // Three literals, then a copy of length 3 with the literal X.
        let r = measure(b"abcabcX", Options { window: 3, max_length: 6 }).unwrap();
        assert_eq!((r.phrases, r.copy_phrases, r.longest_copy), (4, 1, 3));
        let names: Vec<_> = r.encodings.iter().map(|e| e.name).collect();
        assert_eq!(names, ["none", "huffman"]);
        // 5 magic bytes, two one-byte uvarints, three codings, and four records.
        assert_eq!(r.encodings[0].bytes, 5 + 2 + 3 + 2 + 2 + 2 + 3);
    }
}
