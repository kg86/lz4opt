//! LZ4 frame encoder with greedy and optimal parsing, and a greedy LZ77
//! encoder, both built on the sliding suffix tree of the
//! [`snakree`](https://github.com/kg86/snakree) crate.
//!
//! [`lz4`] encodes LZ4 frames that the official `lz4` tool decodes, with a
//! greedy or an optimal parser; [`lz4::framejson`] converts frames to JSON and
//! back. [`lz77`] provides a greedy LZ77 parser and a container format.

mod io;

pub mod lz4;
pub mod lz77;

#[cfg(test)]
mod testutil;
