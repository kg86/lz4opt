//! Summarizes LZ4 frames: counts, length
//! and offset histograms, and per-block records. It streams the frames instead
//! of building the lz4frame-json model, whose literal strings make large
//! frames slow to parse; the tests check it against that parser.
//!
//! usage: framestats input.lz4

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read};

use lz4opt::lz4::format::{
    block_max_size_bytes, BD_RESERVED, BLOCK_UNCOMPRESSED_BIT, FLG_BLOCK_CHECKSUM, FLG_BLOCK_INDEPENDENCE,
    FLG_CONTENT_CHECKSUM, FLG_CONTENT_SIZE, FLG_DICTIONARY_ID, FLG_RESERVED, FLG_VERSION_1, FLG_VERSION_MASK,
    LENGTH_EXTENSION_MAX, LENGTH_EXTENSION_THRESHOLD, MAGIC_NUMBER_LZ4, MIN_MATCH_LENGTH,
};
use serde::Serialize;

#[cfg(test)]
mod tests;

#[path = "../common/diagnostics.rs"]
mod diagnostics;
use diagnostics::{fail, usage};

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct Counts {
    frames: u64,
    blocks: u64,
    compressed_blocks: u64,
    uncompressed_blocks: u64,
    sequences: u64,
    matches: u64,
    literal_runs: u64,
    literal_bytes: u64,
    match_bytes: u64,
    uncompressed_bytes: u64,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct BlockStats {
    frame: usize,
    block: usize,
    uncompressed: bool,
    payload_bytes: u32,
    decoded_bytes: u64,
    sequences: u64,
    matches: u64,
    literal_bytes: u64,
    longest_match: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Stats {
    counts: Counts,
    histograms: BTreeMap<&'static str, BTreeMap<u64, u64>>,
    block_details: Vec<BlockStats>,
    block_max_bytes: u32,
}

const HISTOGRAMS: [&str; 8] = [
    "literal_length",
    "match_length",
    "match_offset",
    "sequences_per_block",
    "matches_per_block",
    "literal_bytes_per_block",
    "longest_match_per_block",
    "decoded_bytes_per_block",
];

impl Stats {
    fn new() -> Stats {
        Stats {
            counts: Counts::default(),
            histograms: HISTOGRAMS.iter().map(|&name| (name, BTreeMap::new())).collect(),
            block_details: Vec::new(),
            block_max_bytes: 0,
        }
    }

    fn count(&mut self, histogram: &'static str, key: u64) {
        *self.histograms.get_mut(histogram).expect("known histogram").entry(key).or_insert(0) += 1;
    }

    /// Folds a finished block into the totals and the per-block histograms.
    fn add_block(&mut self, bs: BlockStats) {
        self.counts.sequences += bs.sequences;
        self.counts.matches += bs.matches;
        self.counts.literal_bytes += bs.literal_bytes;
        self.count("sequences_per_block", bs.sequences);
        self.count("matches_per_block", bs.matches);
        self.count("literal_bytes_per_block", bs.literal_bytes);
        self.count("longest_match_per_block", bs.longest_match);
        self.count("decoded_bytes_per_block", bs.decoded_bytes);
        self.block_details.push(bs);
    }
}

/// Reads exactly `buf.len()` bytes. Returns false if the input ended before
/// the first byte, and an "unexpected EOF" error if it ended partway.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> Result<bool, String> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) if n == 0 => return Ok(false),
            Ok(0) => return Err("unexpected EOF".to_string()),
            Ok(k) => n += k,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err.to_string()),
        }
    }
    Ok(true)
}

/// Like `read_full`, but input that ends before the first byte is also an
/// "unexpected EOF" error.
fn read_required(r: &mut impl Read, buf: &mut [u8]) -> Result<(), String> {
    if read_full(r, buf)? {
        Ok(())
    } else {
        Err("unexpected EOF".to_string())
    }
}

/// Reads one or more LZ4 frames from `r` and aggregates them block by block,
/// holding at most one block in memory. Literal bytes are skipped, not
/// decoded. The frame layout is checked, but not the header checksum or the
/// content checksum: compare.py checks the content by decoding with the
/// official lz4.
fn summarize(r: impl Read) -> Result<Stats, String> {
    let mut br = BufReader::with_capacity(1 << 16, r);
    let mut s = Stats::new();
    let mut block: Vec<u8> = Vec::new();
    for fi in 0usize.. {
        let mut magic = [0u8; 4];
        match read_full(&mut br, &mut magic) {
            Ok(true) => {}
            Ok(false) if fi > 0 => return Ok(s),
            Ok(false) => return Err(format!("frame {fi}: magic number: unexpected EOF")),
            Err(err) => return Err(format!("frame {fi}: magic number: {err}")),
        }
        let magic = u32::from_le_bytes(magic);
        if magic != MAGIC_NUMBER_LZ4 {
            return Err(format!("frame {fi}: not a normal LZ4 frame (magic {magic:#010x})"));
        }
        let mut desc = [0u8; 3]; // FLG, BD, and header checksum
        read_required(&mut br, &mut desc).map_err(|err| format!("frame {fi}: descriptor: {err}"))?;
        let (flg, bd) = (desc[0], desc[1]);
        if flg & FLG_VERSION_MASK != FLG_VERSION_1 || flg & FLG_RESERVED != 0 || bd & BD_RESERVED != 0 {
            return Err(format!("frame {fi}: invalid descriptor {flg:#04x} {bd:#04x}"));
        }
        // Independent blocks and a content checksum; no block checksum, content
        // size, or dictionary ID.
        let options =
            FLG_BLOCK_INDEPENDENCE | FLG_BLOCK_CHECKSUM | FLG_CONTENT_SIZE | FLG_CONTENT_CHECKSUM | FLG_DICTIONARY_ID;
        if flg & options != FLG_BLOCK_INDEPENDENCE | FLG_CONTENT_CHECKSUM {
            return Err("frames must use independent blocks and a content checksum, and no block checksum, content size, or dictionary ID".to_string());
        }
        let code = bd >> 4;
        let max_bytes = block_max_size_bytes(code)
            .ok_or_else(|| format!("frame {fi}: invalid block maximum size code {code}"))?
            as u32;
        if s.block_max_bytes != 0 && s.block_max_bytes != max_bytes {
            return Err("inconsistent block sizes".to_string());
        }
        s.block_max_bytes = max_bytes;
        s.counts.frames += 1;
        for bi in 0usize.. {
            let mut size = [0u8; 4];
            read_required(&mut br, &mut size).map_err(|err| format!("frame {fi} block {bi}: size: {err}"))?;
            let v = u32::from_le_bytes(size);
            if v == 0 {
                break; // end mark
            }
            let mut bs = BlockStats {
                frame: fi,
                block: bi,
                uncompressed: v & BLOCK_UNCOMPRESSED_BIT != 0,
                payload_bytes: v & !BLOCK_UNCOMPRESSED_BIT,
                ..Default::default()
            };
            if bs.payload_bytes == 0 || bs.payload_bytes > max_bytes {
                return Err(format!("frame {fi} block {bi}: invalid payload size {}", bs.payload_bytes));
            }
            block.resize(bs.payload_bytes as usize, 0);
            read_required(&mut br, &mut block).map_err(|err| format!("frame {fi} block {bi}: payload: {err}"))?;
            s.counts.blocks += 1;
            if bs.uncompressed {
                s.counts.uncompressed_blocks += 1;
                s.counts.uncompressed_bytes += u64::from(bs.payload_bytes);
                bs.decoded_bytes = u64::from(bs.payload_bytes);
            } else {
                s.counts.compressed_blocks += 1;
                s.sequences(&block, max_bytes, &mut bs).map_err(|err| format!("frame {fi} block {bi}: {err}"))?;
            }
            s.add_block(bs);
        }
        let mut checksum = [0u8; 4];
        read_required(&mut br, &mut checksum).map_err(|err| format!("frame {fi}: content checksum: {err}"))?;
    }
    unreachable!("the frame loop returns from inside")
}

impl Stats {
    /// Walks the sequences of one compressed block. Every sequence but the
    /// last has a match; the last one ends exactly at the end of the block.
    fn sequences(&mut self, b: &[u8], max_bytes: u32, bs: &mut BlockStats) -> Result<(), String> {
        // Returns the length for a token nibble and the position after its
        // extension bytes.
        let length = |mut p: usize, nibble: u8| -> Result<(u64, usize), String> {
            let mut n = u64::from(nibble);
            if usize::from(nibble) != LENGTH_EXTENSION_THRESHOLD {
                return Ok((n, p));
            }
            loop {
                if p >= b.len() {
                    return Err(format!("truncated length extension at byte {p}"));
                }
                n += u64::from(b[p]);
                p += 1;
                if usize::from(b[p - 1]) != LENGTH_EXTENSION_MAX {
                    return Ok((n, p));
                }
            }
        };
        let mut p = 0;
        loop {
            if p >= b.len() {
                return Err("missing final sequence".to_string());
            }
            let token = b[p];
            let (lit, q) = length(p + 1, token >> 4)?;
            if lit > (b.len() - q) as u64 {
                return Err(format!("literals overrun the block at byte {q}"));
            }
            p = q + lit as usize;
            bs.sequences += 1;
            bs.literal_bytes += lit;
            bs.decoded_bytes += lit;
            self.count("literal_length", lit);
            if lit > 0 {
                self.counts.literal_runs += 1;
            }
            if p == b.len() {
                break;
            }
            if b.len() - p < 2 {
                return Err(format!("truncated offset at byte {p}"));
            }
            let offset = u64::from(u16::from_le_bytes([b[p], b[p + 1]]));
            if offset == 0 || offset > bs.decoded_bytes {
                return Err(format!("invalid offset {offset} at byte {p}"));
            }
            let (n, q) = length(p + 2, token & LENGTH_EXTENSION_THRESHOLD as u8)?;
            p = q;
            let n = n + MIN_MATCH_LENGTH as u64;
            bs.matches += 1;
            bs.decoded_bytes += n;
            bs.longest_match = bs.longest_match.max(n);
            self.counts.match_bytes += n;
            self.count("match_length", n);
            self.count("match_offset", offset);
        }
        if bs.decoded_bytes > u64::from(max_bytes) {
            return Err(format!("decoded size {} exceeds the block maximum {max_bytes}", bs.decoded_bytes));
        }
        Ok(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        usage("usage: framestats input.lz4");
    }
    let result = File::open(&args[1])
        .map_err(|err| err.to_string())
        .and_then(summarize)
        .and_then(|s| serde_json::to_string(&s).map_err(|err| err.to_string()));
    match result {
        Ok(json) => println!("{json}"),
        Err(err) => {
            fail(err);
        }
    }
}
