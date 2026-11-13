use std::io::Read;

use crate::io::read_full;

use super::block_parser::{parse_sequence_loop, BlockReader};
use super::errors::{ParseCause, ParseError};
use super::model::*;

use super::super::format::{
    block_max_size_bytes, BLOCK_UNCOMPRESSED_BIT, FLG_BLOCK_CHECKSUM, FLG_BLOCK_INDEPENDENCE, FLG_CONTENT_CHECKSUM,
    FLG_CONTENT_SIZE, FLG_DICTIONARY_ID,
};
pub use super::super::format::{
    MAGIC_NUMBER_LEGACY, MAGIC_NUMBER_LZ4, MAGIC_NUMBER_SKIPPABLE_MAX, MAGIC_NUMBER_SKIPPABLE_MIN,
};

fn is_skippable_magic(v: u32) -> bool {
    (MAGIC_NUMBER_SKIPPABLE_MIN..=MAGIC_NUMBER_SKIPPABLE_MAX).contains(&v)
}

/// Reads `r` to EOF as a concatenated LZ4 frame stream: zero or more normal
/// frames and skippable frames, in input order. Byte offsets in the returned
/// model are absolute positions in `r`, not per-frame. A clean EOF (0 bytes
/// read while looking for the next magic number) ends the stream successfully
/// only once at least one frame has been parsed; an empty input and a magic
/// number truncated partway through (1-3 bytes) are both errors.
pub fn parse(r: impl Read) -> Result<Root, ParseError> {
    let mut fr = FrameReader::new(r);
    let mut frames = Vec::new();

    loop {
        let Some(magic) = fr.read_stream_magic()? else {
            if frames.is_empty() {
                return Err(ParseError::msg(fr.offset, "magic_number", "empty input: expected at least one LZ4 Frame"));
            }
            return Ok(Root { frames });
        };

        if magic.value == MAGIC_NUMBER_LZ4 {
            let frame_descriptor = parse_frame_descriptor(&mut fr)?;
            let tail = parse_data_blocks(&mut fr, &frame_descriptor.flg, &frame_descriptor.bd)?;
            frames.push(Frame::Normal(Box::new(NormalFrame {
                magic_number: magic,
                frame_descriptor,
                data_blocks: tail.data_blocks,
                end_mark: tail.end_mark,
                content_checksum: tail.content_checksum,
            })));
        } else if is_skippable_magic(magic.value) {
            frames.push(Frame::Skippable(parse_skippable_frame(&mut fr, magic)?));
        } else if magic.value == MAGIC_NUMBER_LEGACY {
            return Err(ParseError::msg(
                magic.offset,
                "magic_number",
                format!("Legacy Frame (magic {:#010x}) is not supported", magic.value),
            ));
        } else {
            return Err(ParseError::msg(
                magic.offset,
                "magic_number",
                format!("unknown magic number {:#010x}", magic.value),
            ));
        }
    }
}

/// Reads fixed-length fields from a reader while tracking the byte offset of
/// the next unread byte, so parse errors can report where in the input they
/// occurred.
pub(super) struct FrameReader<R> {
    r: R,
    pub(super) offset: u64,
}

/// Bounds the buffer used by `read_chunked` (and, via it, `read_hex`,
/// `read_data`, and `BlockReader::read_literal`) to copy Data Block content,
/// Block Format literals, and Skippable Frame User Data. Reading in
/// fixed-size chunks means a maliciously large declared size cannot force an
/// allocation sized to that declared value before any bytes are read.
const DATA_CHUNK_SIZE: usize = 32 * 1024;

impl<R: Read> FrameReader<R> {
    pub(super) fn new(r: R) -> FrameReader<R> {
        FrameReader { r, offset: 0 }
    }

    /// Reads exactly `n` bytes starting at the reader's current offset,
    /// advancing it by the number of bytes actually read so it always reflects
    /// the underlying reader's position, even on a short or failed read. `path`
    /// names the field being read, for the returned error.
    pub(super) fn read_exact(&mut self, n: usize, path: &str) -> Result<(Vec<u8>, u64), ParseError> {
        let start = self.offset;
        let mut buf = vec![0u8; n];
        let (read, err) = read_full(&mut self.r, &mut buf);
        self.offset += read as u64;
        if let Some(err) = err {
            return Err(ParseError::new(start, path, ParseCause::Io(err)));
        }
        if read < n {
            return Err(ParseError::new(
                start,
                path,
                ParseCause::ShortRead { expected: n as u64, actual: read as u64 },
            ));
        }
        Ok((buf, start))
    }

    /// Reads the next frame's 4-byte little-endian magic number at the
    /// stream's current offset. Unlike `read_exact`, it treats a read of
    /// exactly zero bytes before EOF as a non-error condition (`None`): the
    /// stream-level loop is the only caller that can legitimately stop there,
    /// and only once it has already parsed at least one frame. Any read of 1-3
    /// bytes before EOF is always a truncated magic number.
    fn read_stream_magic(&mut self) -> Result<Option<NumericField<u32>>, ParseError> {
        let start = self.offset;
        let mut buf = [0u8; 4];
        let (read, err) = read_full(&mut self.r, &mut buf);
        self.offset += read as u64;
        if let Some(err) = err {
            return Err(ParseError::new(start, "magic_number", ParseCause::Io(err)));
        }
        if read == 0 {
            return Ok(None);
        }
        if read < 4 {
            return Err(ParseError::new(
                start,
                "magic_number",
                ParseCause::ShortRead { expected: 4, actual: read as u64 },
            ));
        }
        Ok(Some(NumericField::new(start, decode_u32_le(&buf), &buf)))
    }

    /// Reads exactly `total` bytes starting at the reader's current offset,
    /// copying through a fixed-size buffer rather than allocating one sized by
    /// `total`, and calls `on_chunk` with each chunk actually read, in order,
    /// before any read error on that chunk is reported: `on_chunk` sees every
    /// byte `offset` advanced past, including a final short chunk that ends in
    /// an error, and may be called with a zero-length chunk on that final
    /// iteration if no bytes were read at all. `path` names the field being
    /// read, for the returned error.
    pub(super) fn read_chunked(
        &mut self,
        total: u64,
        path: &str,
        mut on_chunk: impl FnMut(&[u8]),
    ) -> Result<u64, ParseError> {
        let start = self.offset;
        let mut buf = vec![0u8; DATA_CHUNK_SIZE];
        let mut read = 0u64;
        while read < total {
            let n = (buf.len() as u64).min(total - read) as usize;
            let (got, err) = read_full(&mut self.r, &mut buf[..n]);
            self.offset += got as u64;
            read += got as u64;
            on_chunk(&buf[..got]);
            if let Some(err) = err {
                return Err(ParseError::new(start, path, ParseCause::Io(err)));
            }
            if got < n {
                return Err(ParseError::new(start, path, ParseCause::ShortRead { expected: total, actual: read }));
            }
        }
        Ok(start)
    }

    /// Reads exactly `total` bytes starting at the reader's current offset and
    /// hex-encodes them into a single string, via `read_chunked`.
    fn read_hex(&mut self, total: u64, path: &str) -> Result<(String, u64), ParseError> {
        if total == 0 {
            return Ok((String::new(), self.offset));
        }
        let mut hex = String::new();
        let start = self.read_chunked(total, path, |chunk| push_hex(&mut hex, chunk))?;
        Ok((hex, start))
    }

    /// Reads exactly `total` bytes starting at the reader's current offset and
    /// returns them as a single vector, via `read_chunked`. Unlike `read_hex`,
    /// callers must only pass a total already bounded to a safe upper limit
    /// (e.g. validated against Block Maximum Size) before the call: reading in
    /// chunks avoids allocating a buffer sized by an attacker-controlled
    /// total, but the result vector reintroduces that allocation, so it is
    /// unsuitable for unbounded fields such as Skippable Frame User Data.
    fn read_data(&mut self, total: u64, path: &str) -> Result<(Vec<u8>, u64), ParseError> {
        if total == 0 {
            return Ok((Vec::new(), self.offset));
        }
        let mut raw = Vec::with_capacity(total as usize);
        let start = self.read_chunked(total, path, |chunk| raw.extend_from_slice(chunk))?;
        Ok((raw, start))
    }
}

/// Reads `n` bytes as `path` and decodes them with `decode` into a
/// `NumericField`, preserving the input byte order in the field's hex.
fn read_numeric_field<T, R: Read>(
    fr: &mut FrameReader<R>,
    n: usize,
    path: &str,
    decode: fn(&[u8]) -> T,
) -> Result<NumericField<T>, ParseError> {
    let (raw, offset) = fr.read_exact(n, path)?;
    Ok(NumericField::new(offset, decode(&raw), &raw))
}

/// Reads and decomposes the Frame Descriptor's FLG byte, validating its
/// version and reserved bit.
fn parse_flg<R: Read>(fr: &mut FrameReader<R>) -> Result<Flg, ParseError> {
    let field = read_numeric_field(fr, 1, "frame_descriptor.flg", decode_u8_le)?;
    let v = field.value;
    let version = (v >> 6) & 0x03;
    let reserved_bit = (v >> 1) & 0x01;

    if version != 1 {
        return Err(ParseError::msg(
            field.offset,
            "frame_descriptor.flg",
            format!("unsupported FLG version {version}, want 1"),
        ));
    }
    if reserved_bit != 0 {
        return Err(ParseError::msg(
            field.offset,
            "frame_descriptor.flg",
            format!("FLG reserved bit is {reserved_bit}, want 0"),
        ));
    }

    Ok(Flg {
        field,
        version,
        block_independence: v & FLG_BLOCK_INDEPENDENCE != 0,
        block_checksum: v & FLG_BLOCK_CHECKSUM != 0,
        content_size_present: v & FLG_CONTENT_SIZE != 0,
        content_checksum: v & FLG_CONTENT_CHECKSUM != 0,
        reserved_bit,
        dictionary_id_present: v & FLG_DICTIONARY_ID != 0,
    })
}

/// Reads and decomposes the Frame Descriptor's BD byte, validating its
/// reserved bits and Block Maximum Size code.
fn parse_bd<R: Read>(fr: &mut FrameReader<R>) -> Result<Bd, ParseError> {
    let field = read_numeric_field(fr, 1, "frame_descriptor.bd", decode_u8_le)?;
    let v = field.value;
    let reserved_bit_7 = (v >> 7) & 0x01;
    let block_max_size_code = (v >> 4) & 0x07;
    let reserved_bits_3_to_0 = v & 0x0F;

    if reserved_bit_7 != 0 {
        return Err(ParseError::msg(
            field.offset,
            "frame_descriptor.bd",
            format!("BD reserved bit 7 is {reserved_bit_7}, want 0"),
        ));
    }
    if reserved_bits_3_to_0 != 0 {
        return Err(ParseError::msg(
            field.offset,
            "frame_descriptor.bd",
            format!("BD reserved bits 3-0 are {reserved_bits_3_to_0:#x}, want 0"),
        ));
    }
    let Some(max_size_bytes) = block_max_size_bytes(block_max_size_code) else {
        return Err(ParseError::msg(
            field.offset,
            "frame_descriptor.bd",
            format!("undefined Block Maximum Size code {block_max_size_code}, want 4-7"),
        ));
    };

    Ok(Bd {
        field,
        reserved_bit_7,
        block_max_size_code,
        block_max_size_bytes: max_size_bytes,
        reserved_bits_3_0: reserved_bits_3_to_0,
    })
}

/// Reads a normal frame's Frame Descriptor: FLG, BD, the Content Size and
/// Dictionary ID fields FLG marks present, and the Header Checksum. It reads
/// the Header Checksum's stored value only; it does not recompute or validate
/// it.
fn parse_frame_descriptor<R: Read>(fr: &mut FrameReader<R>) -> Result<FrameDescriptor, ParseError> {
    let flg = parse_flg(fr)?;
    let bd = parse_bd(fr)?;

    let content_size = if flg.content_size_present {
        Some(read_numeric_field(fr, 8, "frame_descriptor.content_size", decode_u64_le)?)
    } else {
        None
    };
    let dictionary_id = if flg.dictionary_id_present {
        Some(read_numeric_field(fr, 4, "frame_descriptor.dictionary_id", decode_u32_le)?)
    } else {
        None
    };
    let header_checksum = read_numeric_field(fr, 1, "frame_descriptor.header_checksum", decode_u8_le)?;

    Ok(FrameDescriptor { flg, bd, content_size, dictionary_id, header_checksum })
}

/// Splits a non-zero Block Size field into its uncompressed flag and data
/// size, validating the data size against the Frame Descriptor's Block Maximum
/// Size. A zero value (EndMark) must be handled by the caller before calling
/// this function.
fn decode_block_size(field: NumericField<u32>, bd: &Bd, path: &str) -> Result<BlockSize, ParseError> {
    let uncompressed = field.value & BLOCK_UNCOMPRESSED_BIT != 0;
    let data_size = field.value & !BLOCK_UNCOMPRESSED_BIT;

    if data_size > bd.block_max_size_bytes {
        return Err(ParseError::msg(
            field.offset,
            path,
            format!("block data size {data_size} exceeds Block Maximum Size {}", bd.block_max_size_bytes),
        ));
    }

    Ok(BlockSize { field, data_size, uncompressed })
}

/// The parts of a normal frame that follow its magic number and Frame
/// Descriptor: the Data Blocks, the terminating EndMark, and, when present,
/// the Content Checksum.
struct FrameTail {
    data_blocks: Vec<DataBlock>,
    end_mark: NumericField<u32>,
    content_checksum: Option<NumericField<u32>>,
}

/// Reads a normal frame's Data Block sequence: each Block Size, its data, and,
/// when FLG marks it present, the Block Checksum that follows each block. It
/// stops at the first zero Block Size (EndMark) and, when FLG marks it
/// present, reads the Content Checksum that follows. It never recomputes or
/// validates checksum values.
///
/// A block's data is read according to `block_size.uncompressed`: an
/// uncompressed block (including size 0) becomes a single
/// `UncompressedDataSegment`; a compressed block is delegated to the Block
/// Format parser, which never reads past the block's declared `data_size`, so
/// a compressed-block parse error leaves the following Block Checksum (or next
/// Block Size/EndMark) unconsumed.
fn parse_data_blocks<R: Read>(fr: &mut FrameReader<R>, flg: &Flg, bd: &Bd) -> Result<FrameTail, ParseError> {
    let mut blocks = Vec::new();

    loop {
        let i = blocks.len();
        // The 4-byte field is read before it is known to be a Block Size or the
        // EndMark, so a truncated read is reported as the next block's size.
        let size_field = read_numeric_field(fr, 4, &format!("data_blocks[{i}].block_size"), decode_u32_le)?;

        if size_field.value == 0 {
            let content_checksum = if flg.content_checksum {
                Some(read_numeric_field(fr, 4, "content_checksum", decode_u32_le)?)
            } else {
                None
            };
            return Ok(FrameTail { data_blocks: blocks, end_mark: size_field, content_checksum });
        }

        let block_size = decode_block_size(size_field, bd, &format!("data_blocks[{i}].block_size"))?;

        let data = if block_size.uncompressed {
            let (raw, offset) = fr.read_data(u64::from(block_size.data_size), &format!("data_blocks[{i}].data"))?;
            vec![BlockData::Uncompressed(UncompressedDataSegment::new(offset, &raw))]
        } else {
            let mut br = BlockReader::new(fr, u64::from(block_size.data_size), i);
            parse_sequence_loop(&mut br, bd)?.into_iter().map(|s| BlockData::Sequence(Box::new(s))).collect()
        };

        let block_checksum = if flg.block_checksum {
            Some(read_numeric_field(fr, 4, &format!("data_blocks[{i}].block_checksum"), decode_u32_le)?)
        } else {
            None
        };

        blocks.push(DataBlock { block_size, data, block_checksum });
    }
}

/// Reads a skippable frame's Frame Size and User Data, given its already-read
/// magic number. It reads the data through a fixed-size buffer (via
/// `read_hex`) rather than allocating a buffer sized by the declared Frame
/// Size, so a large declared size paired with a short input cannot force a
/// large upfront allocation.
fn parse_skippable_frame<R: Read>(
    fr: &mut FrameReader<R>,
    magic: NumericField<u32>,
) -> Result<SkippableFrame, ParseError> {
    let frame_size = read_numeric_field(fr, 4, "frame_size", decode_u32_le)?;
    let (hex, offset) = fr.read_hex(u64::from(frame_size.value), "data")?;
    let size = u64::from(frame_size.value);
    Ok(SkippableFrame { magic_number: magic, frame_size, data: SkippableData { offset, size, hex } })
}

#[cfg(test)]
mod block_tests;
#[cfg(test)]
mod tests;
