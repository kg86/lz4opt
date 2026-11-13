//! Byte builders for LZ4 frame fixtures, shared by the framejson tests.

use super::MAGIC_NUMBER_LZ4;
use crate::lz4::format::BLOCK_UNCOMPRESSED_BIT;

/// Encodes `v` as 4 little-endian bytes.
pub(super) fn u32_le(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// Encodes `v` as 8 little-endian bytes.
pub(super) fn u64_le(v: u64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// Packs the FLG bit fields in the order the format defines them: bits 7-6
/// version, bit 5 block independence, bit 4 block checksum, bit 3 content size
/// present, bit 2 content checksum, bit 1 reserved, bit 0 dictionary ID
/// present.
pub(super) fn flg_byte(
    version: u8,
    block_independence: bool,
    block_checksum: bool,
    content_size_present: bool,
    content_checksum: bool,
    reserved_bit: u8,
    dictionary_id_present: bool,
) -> u8 {
    let mut v = (version & 0x03) << 6;
    v |= u8::from(block_independence) << 5;
    v |= u8::from(block_checksum) << 4;
    v |= u8::from(content_size_present) << 3;
    v |= u8::from(content_checksum) << 2;
    v |= (reserved_bit & 0x01) << 1;
    v | u8::from(dictionary_id_present)
}

/// Packs the BD bit fields: bit 7 reserved, bits 6-4 Block Maximum Size code,
/// bits 3-0 reserved.
pub(super) fn bd_byte(reserved_bit_7: u8, block_max_size_code: u8, reserved_bits_3_0: u8) -> u8 {
    ((reserved_bit_7 & 0x01) << 7) | ((block_max_size_code & 0x07) << 4) | (reserved_bits_3_0 & 0x0f)
}

/// Builds the bytes for FLG, BD, the optional Content Size and Dictionary ID
/// (present only when `Some`), and the Header Checksum.
pub(super) fn descriptor_fixture(
    flg: u8,
    bd: u8,
    content_size: Option<u64>,
    dictionary_id: Option<u32>,
    header_checksum: u8,
) -> Vec<u8> {
    let mut buf = vec![flg, bd];
    if let Some(v) = content_size {
        buf.extend(u64_le(v));
    }
    if let Some(v) = dictionary_id {
        buf.extend(u32_le(v));
    }
    buf.push(header_checksum);
    buf
}

/// Returns the little-endian bytes of the normal frame magic number.
pub(super) fn valid_magic_bytes() -> Vec<u8> {
    u32_le(MAGIC_NUMBER_LZ4)
}

/// Builds the 4-byte little-endian Block Size field: the top bit set for
/// uncompressed, the low 31 bits carrying `data_size`.
pub(super) fn block_size_bytes(uncompressed: bool, data_size: u32) -> Vec<u8> {
    u32_le(if uncompressed { data_size | BLOCK_UNCOMPRESSED_BIT } else { data_size })
}

/// Returns the EndMark bytes.
pub(super) fn end_mark_bytes() -> Vec<u8> {
    u32_le(0)
}

/// Builds a valid compressed Data Block's data: a single literal-only sequence
/// (token low nibble 0, no match) whose literals are exactly `payload`.
/// Because it is always exactly one sequence covering the whole expanded
/// output, it satisfies the End of block condition regardless of the payload's
/// length, which keeps fixtures that only care about frame-level behavior
/// (Block Size/data_size, checksums, offsets) from having to reason about
/// match encoding.
pub(super) fn literal_only_block_bytes(payload: &[u8]) -> Vec<u8> {
    let n = payload.len();
    let (code, ext) = if n < 15 {
        (n as u8, Vec::new())
    } else {
        let mut additional = n - 15;
        let mut ext = Vec::new();
        while additional >= 255 {
            ext.push(0xff);
            additional -= 255;
        }
        ext.push(additional as u8);
        (15, ext)
    };
    let mut out = vec![code << 4];
    out.extend(ext);
    out.extend_from_slice(payload);
    out
}

/// Builds the smallest valid normal frame: no Content Size, no Dictionary ID,
/// no checksums, and no Data Blocks.
pub(super) fn minimal_normal_frame_bytes() -> Vec<u8> {
    let flg = flg_byte(1, true, false, false, false, 0, false);
    let bd = bd_byte(0, 4, 0);
    [valid_magic_bytes(), descriptor_fixture(flg, bd, None, None, 0), end_mark_bytes()].concat()
}

/// Builds a skippable frame: magic, a Frame Size field matching `data.len()`,
/// and `data` itself.
pub(super) fn skippable_frame_bytes(magic: u32, data: &[u8]) -> Vec<u8> {
    [u32_le(magic), u32_le(data.len() as u32), data.to_vec()].concat()
}
