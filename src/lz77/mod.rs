//! Online greedy LZ77 phrases built from the longest repeated suffix of a
//! sliding suffix tree ([`Snakree`](snakree::Snakree)), and a simple
//! byte-oriented container format for them; see `docs/lz77/README.md` for the
//! format.
//!
//! [`Parser`] computes the phrases. [`encode`] and [`decode`] write and read
//! the format.

mod format;
mod parser;
#[cfg(test)]
mod tests;

pub use format::{decode, encode, Coding, EncodeOptions, Entropy, Error};
pub use parser::{Options, Parser, Token};

/// The default for a zero window or maximum length: `isize::MAX`, which
/// fixes the value written in encoded headers.
pub(crate) const UNBOUNDED: usize = isize::MAX as usize;
