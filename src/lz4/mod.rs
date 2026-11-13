//! Encodes standard LZ4 frames using exact longest matches from a sliding
//! suffix tree. Blocks are independent, with a content checksum and no
//! external dictionary. The greedy parser takes the longest match at every
//! parse position; the optimal parser minimizes each block's compressed size
//! exactly for the fixed block boundaries. This experimental encoder does not
//! promise better speed than other LZ4 encoders.
//!
//! Matching takes O(n) time per block for the byte alphabet, and optimal
//! parsing O(n). Match arrays and input/output buffers use O(n)
//! memory. The tree retains both history and lookahead, using
//! O(min(n, 65535 + L + 1)) space, where L is the longest match. In
//! particular, its memory is not limited to a 64 KiB history buffer.
//!
//! The format follows
//! <https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md> and
//! <https://github.com/lz4/lz4/blob/dev/doc/lz4_Frame_format.md>.

mod block;
pub mod format;
mod frame;
pub mod framejson;
mod greedy;
mod matches;
mod optimal;
#[cfg(test)]
mod tests;
mod xxhash;

pub use format::{BLOCK_1_MIB, BLOCK_256_KIB, BLOCK_4_MIB, BLOCK_64_KIB};
pub use frame::{encode, Error, Options, Parser};
pub use matches::{for_each_longest_match, MAX_OFFSET};
