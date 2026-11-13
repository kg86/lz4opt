//! Constants and size-code conversions shared by LZ4 encoders and readers.

/// The magic number that begins a normal LZ4 frame, stored little-endian.
pub const MAGIC_NUMBER_LZ4: u32 = 0x184D2204;
/// The magic number that begins a legacy frame.
pub const MAGIC_NUMBER_LEGACY: u32 = 0x184C2102;
/// The first skippable frame magic number.
pub const MAGIC_NUMBER_SKIPPABLE_MIN: u32 = 0x184D2A50;
/// The last skippable frame magic number.
pub const MAGIC_NUMBER_SKIPPABLE_MAX: u32 = 0x184D2A5F;
/// Mask of the FLG version field, bits 7-6.
pub const FLG_VERSION_MASK: u8 = 0xC0;
/// FLG version field holding version 1, the only defined version.
pub const FLG_VERSION_1: u8 = 0x40;
/// FLG bit set when blocks are independent of each other.
pub const FLG_BLOCK_INDEPENDENCE: u8 = 0x20;
/// FLG bit set when each block is followed by its checksum.
pub const FLG_BLOCK_CHECKSUM: u8 = 0x10;
/// FLG bit set when the descriptor holds the content size.
pub const FLG_CONTENT_SIZE: u8 = 0x08;
/// FLG bit set when the frame ends with the content checksum.
pub const FLG_CONTENT_CHECKSUM: u8 = 0x04;
/// Reserved FLG bit, which must be zero.
pub const FLG_RESERVED: u8 = 0x02;
/// FLG bit set when the descriptor holds a dictionary ID.
pub const FLG_DICTIONARY_ID: u8 = 0x01;
/// Reserved BD bits 7 and 3-0, which must be zero.
pub const BD_RESERVED: u8 = 0x8F;

/// Marks a block payload as uncompressed in its size field.
pub const BLOCK_UNCOMPRESSED_BIT: u32 = 0x8000_0000;

/// Maximum block size of 64 KiB, in bytes.
pub const BLOCK_64_KIB: usize = 64 << 10;
/// Maximum block size of 256 KiB, in bytes.
pub const BLOCK_256_KIB: usize = 256 << 10;
/// Maximum block size of 1 MiB, in bytes.
pub const BLOCK_1_MIB: usize = 1 << 20;
/// Maximum block size of 4 MiB, in bytes.
pub const BLOCK_4_MIB: usize = 4 << 20;

const BLOCK_SIZES: [usize; 4] = [BLOCK_64_KIB, BLOCK_256_KIB, BLOCK_1_MIB, BLOCK_4_MIB];

/// Returns the byte size for a BD block maximum size code, or None for an undefined code.
pub fn block_max_size_bytes(code: u8) -> Option<u32> {
    code.checked_sub(4).and_then(|index| BLOCK_SIZES.get(usize::from(index)).map(|&size| size as u32))
}

/// Returns the BD code for a supported maximum block size, or None otherwise.
pub fn block_max_size_code(size: usize) -> Option<u8> {
    BLOCK_SIZES.iter().position(|&bytes| bytes == size).map(|index| index as u8 + 4)
}

/// Minimum number of bytes in a match.
pub const MIN_MATCH_LENGTH: usize = 4;
/// Minimum final literal run length for blocks of at least this size.
pub const MIN_LAST_LITERAL_SIZE: usize = 5;
/// Minimum distance between the last match start and the expanded block end.
pub const MIN_MATCH_DISTANCE_FROM_END: usize = 12;
/// Token length code that requires extension bytes.
pub const LENGTH_EXTENSION_THRESHOLD: usize = 15;
/// Match length that first requires extension bytes.
pub const MATCH_EXTENSION_THRESHOLD: usize = MIN_MATCH_LENGTH + LENGTH_EXTENSION_THRESHOLD;
/// Length contributed by an extension byte that requires another byte.
pub const LENGTH_EXTENSION_MAX: usize = 255;

/// Returns by how much a length field holding `len`, a literal run length or a
/// match length minus `MIN_MATCH_LENGTH`, must grow until it gains its next
/// extension byte, counting the unit that adds the byte.
pub(crate) fn length_extension_slack(len: usize) -> usize {
    if len < LENGTH_EXTENSION_THRESHOLD {
        LENGTH_EXTENSION_THRESHOLD - len
    } else {
        LENGTH_EXTENSION_MAX - (len - LENGTH_EXTENSION_THRESHOLD) % LENGTH_EXTENSION_MAX
    }
}
