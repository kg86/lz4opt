use std::fmt;
use std::io::{self, Read, Write};

use crate::io::read_full;

use super::block::encode_block;
use super::format::{
    block_max_size_code, BLOCK_4_MIB, BLOCK_UNCOMPRESSED_BIT, FLG_BLOCK_INDEPENDENCE, FLG_CONTENT_CHECKSUM,
    FLG_VERSION_1, MAGIC_NUMBER_LZ4,
};
use super::xxhash::{checksum, Xxh32};

/// Selects how each block is split into literals and matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Parser {
    /// Takes the longest legal match at every parse position.
    Greedy,
    /// Minimizes each block's compressed size exactly, given fixed block
    /// boundaries and the 65,535-byte offset limit.
    #[default]
    Optimal,
}

/// Configures frame encoding. The default selects `BLOCK_4_MIB` and `Optimal`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Maximum input bytes per independent block. Zero selects `BLOCK_4_MIB`;
    /// otherwise this must be one of the four block size constants.
    pub block_size: usize,
    /// Block parser.
    pub parser: Parser,
}

impl Options {
    /// Returns an error if `block_size` is neither zero nor one of the four
    /// block size constants.
    pub fn validate(&self) -> Result<(), Error> {
        self.block_size_code().map(|_| ())
    }

    /// Returns the effective block size and its BD code.
    fn block_size_code(&self) -> Result<(usize, u8), Error> {
        let size = if self.block_size == 0 { BLOCK_4_MIB } else { self.block_size };
        block_max_size_code(size).map(|code| (size, code)).ok_or(Error::UnsupportedBlockSize(size))
    }
}

/// An error from [`encode`].
#[derive(Debug)]
pub enum Error {
    /// The block size is not one of the supported constants.
    UnsupportedBlockSize(usize),
    /// Reading the input failed.
    Read(io::Error),
    /// Writing the frame failed.
    Write(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnsupportedBlockSize(size) => write!(f, "lz4: unsupported block size {size}"),
            Error::Read(err) => write!(f, "lz4: read input: {err}"),
            Error::Write(err) => write!(f, "lz4: write frame: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::UnsupportedBlockSize(_) => None,
            Error::Read(err) | Error::Write(err) => Some(err),
        }
    }
}

/// Encodes `src` into a standard LZ4 frame and writes it to `dst`. Blocks are
/// parsed as selected by `options.parser` and are stored uncompressed unless
/// compression reduces their size. A content checksum is always included;
/// block checksums, content size, and dictionary ID are omitted. Empty input
/// produces a frame without blocks.
///
/// Processes one block at a time, independently of `src`'s read boundaries. It
/// does not close `src` or `dst`. Invalid options fail before any I/O; I/O
/// errors may leave an incomplete frame in `dst`.
pub fn encode(mut dst: impl Write, mut src: impl Read, options: &Options) -> Result<(), Error> {
    let (size, block_id) = options.block_size_code()?;
    let flg = FLG_VERSION_1 | FLG_BLOCK_INDEPENDENCE | FLG_CONTENT_CHECKSUM;
    let mut header = [0, 0, 0, 0, flg, block_id << 4, 0];
    header[..4].copy_from_slice(&MAGIC_NUMBER_LZ4.to_le_bytes());
    header[6] = (checksum(&header[4..6]) >> 8) as u8;
    write_bytes(&mut dst, &header)?;

    let mut buffer = vec![0u8; size];
    let mut content = Xxh32::new();
    loop {
        let (n, err) = read_full(&mut src, &mut buffer);
        if let Some(err) = err {
            return Err(Error::Read(err));
        }
        if n > 0 {
            let block = &buffer[..n];
            content.write(block);
            let payload = encode_block(block, options.parser);
            let (payload, field) = if payload.len() >= n {
                (block, n as u32 | BLOCK_UNCOMPRESSED_BIT)
            } else {
                (&payload[..], payload.len() as u32)
            };
            write_bytes(&mut dst, &field.to_le_bytes())?;
            write_bytes(&mut dst, payload)?;
        }
        if n < size {
            break;
        }
    }
    // EndMark followed by the checksum of all original bytes.
    let mut footer = [0u8; 8];
    footer[4..].copy_from_slice(&content.sum32().to_le_bytes());
    write_bytes(&mut dst, &footer)
}

fn write_bytes(dst: &mut impl Write, p: &[u8]) -> Result<(), Error> {
    dst.write_all(p).map_err(Error::Write)
}
