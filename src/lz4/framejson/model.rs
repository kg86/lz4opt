use serde::Serialize;

use serde_json::ser::{Formatter, PrettyFormatter};

/// Frame type discriminators, stored verbatim in each frame's "type" field.
pub const FRAME_TYPE_LZ4: &str = "lz4_frame";
pub const FRAME_TYPE_SKIPPABLE: &str = "skippable_frame";

/// Block Format segment type discriminators, stored verbatim in each physical
/// field's "type" field. `Sequence` itself is not a physical field and
/// carries no "type"; `match_offset` reuses `NumericField<u16>` directly and
/// likewise carries no "type".
pub(crate) const SEGMENT_TYPE_TOKEN: &str = "token";
pub(crate) const SEGMENT_TYPE_LITERAL_LENGTH_EXTENSION: &str = "literal_length_extension";
pub(crate) const SEGMENT_TYPE_MATCH_LENGTH_EXTENSION: &str = "match_length_extension";
pub(crate) const SEGMENT_TYPE_LITERALS: &str = "literals";
pub(crate) const SEGMENT_TYPE_UNCOMPRESSED_DATA: &str = "uncompressed_data";

/// The JSON document produced by inspect and consumed by restore.
#[derive(Debug, Serialize)]
pub struct Root {
    pub frames: Vec<Frame>,
}

impl Root {
    /// Renders the document as indented JSON (two spaces) with a trailing newline,
    /// escaping `<`, `>`, `&`, U+2028, and U+2029 in strings.
    pub fn to_json(&self) -> String {
        let mut out = Vec::new();
        let mut ser = serde_json::Serializer::with_formatter(
            &mut out,
            HtmlEscapingFormatter(PrettyFormatter::with_indent(b"  ")),
        );
        self.serialize(&mut ser).expect("serializing the model cannot fail");
        out.push(b'\n');
        String::from_utf8(out).expect("serde_json writes UTF-8")
    }
}

/// A pretty formatter that also escapes HTML-sensitive characters.
struct HtmlEscapingFormatter<'a>(PrettyFormatter<'a>);

macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(fn $name<W: ?Sized + std::io::Write>(&mut self, w: &mut W $(, $arg: $ty)*) -> std::io::Result<()> {
            self.0.$name(w $(, $arg)*)
        })*
    };
}

impl Formatter for HtmlEscapingFormatter<'_> {
    delegate! {
        begin_array(); end_array();
        begin_array_value(first: bool); end_array_value();
        begin_object(); end_object();
        begin_object_key(first: bool); end_object_key();
        begin_object_value(); end_object_value();
    }

    fn write_string_fragment<W: ?Sized + std::io::Write>(&mut self, w: &mut W, fragment: &str) -> std::io::Result<()> {
        let mut start = 0;
        for (i, c) in fragment.char_indices() {
            let escape = match c {
                '<' => "\\u003c",
                '>' => "\\u003e",
                '&' => "\\u0026",
                '\u{2028}' => "\\u2028",
                '\u{2029}' => "\\u2029",
                _ => continue,
            };
            w.write_all(&fragment.as_bytes()[start..i])?;
            w.write_all(escape.as_bytes())?;
            start = i + c.len_utf8();
        }
        w.write_all(&fragment.as_bytes()[start..])
    }
}

/// A normal frame or a skippable frame, the two shapes that can appear in
/// [`Root::frames`].
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum Frame {
    #[serde(rename = "lz4_frame")]
    Normal(Box<NormalFrame>),
    #[serde(rename = "skippable_frame")]
    Skippable(SkippableFrame),
}

/// A physical field: the byte offset it occupies in the input, its value
/// decoded per the format's little-endian encoding, and the exact input bytes
/// it was decoded from as hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NumericField<T> {
    pub offset: u64,
    pub value: T,
    pub hex: String,
}

impl<T> NumericField<T> {
    /// Builds a `NumericField` from an already-decoded value and the raw bytes
    /// it was decoded from. `raw` is hex-encoded verbatim, so callers must pass
    /// the bytes in the order they appeared in the input.
    pub fn new(offset: u64, value: T, raw: &[u8]) -> NumericField<T> {
        NumericField { offset, value, hex: hex_string(raw) }
    }
}

/// Returns the lowercase, unprefixed hex encoding used for every physical
/// field's "hex" attribute.
pub(crate) fn hex_string(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len() * 2);
    push_hex(&mut out, raw);
    out
}

pub(super) fn push_hex(out: &mut String, raw: &[u8]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for &b in raw {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
}

/// Reads a 1-byte little-endian unsigned integer. `raw` must have been read in
/// full by the caller first: like the other decoders, it panics on a short
/// slice rather than reporting an EOF.
pub(crate) fn decode_u8_le(raw: &[u8]) -> u8 {
    raw[0]
}

/// Reads a 2-byte little-endian unsigned integer (used for a sequence's Match
/// Offset).
pub(crate) fn decode_u16_le(raw: &[u8]) -> u16 {
    u16::from_le_bytes(raw[..2].try_into().unwrap())
}

/// Reads a 4-byte little-endian unsigned integer.
pub(crate) fn decode_u32_le(raw: &[u8]) -> u32 {
    u32::from_le_bytes(raw[..4].try_into().unwrap())
}

/// Reads an 8-byte little-endian unsigned integer.
pub(crate) fn decode_u64_le(raw: &[u8]) -> u64 {
    u64::from_le_bytes(raw[..8].try_into().unwrap())
}

/// The Frame Descriptor's FLG byte, decomposed into its bit fields. The
/// field's own offset, value, and hex serialize first, inline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Flg {
    #[serde(flatten)]
    pub field: NumericField<u8>,
    pub version: u8,
    pub block_independence: bool,
    pub block_checksum: bool,
    pub content_size_present: bool,
    pub content_checksum: bool,
    pub reserved_bit: u8,
    pub dictionary_id_present: bool,
}

/// The Frame Descriptor's BD byte, decomposed into its bit fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Bd {
    #[serde(flatten)]
    pub field: NumericField<u8>,
    pub reserved_bit_7: u8,
    pub block_max_size_code: u8,
    pub block_max_size_bytes: u32,
    pub reserved_bits_3_0: u8,
}

/// A normal frame's Frame Descriptor: FLG, BD, the optional Content Size and
/// Dictionary ID, and the Header Checksum.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FrameDescriptor {
    pub flg: Flg,
    pub bd: Bd,
    pub content_size: Option<NumericField<u64>>,
    pub dictionary_id: Option<NumericField<u32>>,
    pub header_checksum: NumericField<u8>,
}

/// A Data Block's Block Size field, decomposed into the uncompressed flag and
/// the data size it encodes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BlockSize {
    #[serde(flatten)]
    pub field: NumericField<u32>,
    pub data_size: u32,
    pub uncompressed: bool,
}

/// The two shapes `DataBlock::data` holds: a compressed block's data is a
/// non-empty array of sequences, an uncompressed block's a single
/// uncompressed data segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum BlockData {
    Uncompressed(UncompressedDataSegment),
    Sequence(Box<Sequence>),
}

/// A sequence's 1-byte Token, decomposed into its two nibbles. It carries no
/// computed length and no sequence number: those are the parent `Sequence`'s
/// `literal_length` and `match_length`, and the data array's index,
/// respectively.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TokenSegment {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub offset: u64,
    pub size: u64,
    pub hex: String,
    pub literal_length_code: u8,
    pub match_length_code: u8,
}

impl TokenSegment {
    /// Builds a `TokenSegment` from an already-decoded token byte, deriving
    /// `size`, `hex`, and the two nibble codes from `raw` so they cannot drift
    /// from the byte actually read.
    pub fn new(offset: u64, raw: u8) -> TokenSegment {
        TokenSegment {
            kind: SEGMENT_TYPE_TOKEN,
            offset,
            size: 1,
            hex: hex_string(&[raw]),
            literal_length_code: raw >> 4,
            match_length_code: raw & 0x0f,
        }
    }
}

/// A variable-length length-extension byte sequence for either literal length
/// or match length: one or more bytes, each 255 except possibly the last,
/// ending at the first byte less than 255. `additional_length` is the decoded
/// sum of those bytes (255 per full byte plus the final byte's value), not a
/// raw numeric interpretation of `hex`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LengthExtensionSegment {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub offset: u64,
    pub size: u64,
    pub hex: String,
    pub additional_length: u64,
}

impl LengthExtensionSegment {
    /// Builds a `LengthExtensionSegment` from its decoded fields, tagging it
    /// with `kind` (`SEGMENT_TYPE_LITERAL_LENGTH_EXTENSION` or
    /// `SEGMENT_TYPE_MATCH_LENGTH_EXTENSION`) since only the caller knows
    /// which of the two this decoded extension belongs to.
    pub fn new(
        offset: u64,
        size: u64,
        hex: String,
        additional_length: u64,
        kind: &'static str,
    ) -> LengthExtensionSegment {
        LengthExtensionSegment { kind, offset, size, hex, additional_length }
    }
}

/// A sequence's literal bytes. `hex` is the sole source of truth for the
/// bytes; `display` is a lossless, human-readable rendering of the same bytes
/// (see `display_string`) and is never null, becoming the empty string when
/// `size` is 0.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LiteralsSegment {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub offset: u64,
    pub size: u64,
    pub hex: String,
    pub display: String,
}

impl LiteralsSegment {
    /// Builds a `LiteralsSegment` from its hex and display renderings,
    /// deriving `size` from the length of `hex` so it cannot drift from the
    /// bytes actually read.
    pub fn new(offset: u64, hex: String, display: String) -> LiteralsSegment {
        LiteralsSegment { kind: SEGMENT_TYPE_LITERALS, offset, size: hex.len() as u64 / 2, hex, display }
    }
}

/// The single physical field representing an uncompressed block's data range:
/// an uncompressed block has no sequences to decode, so its data is always
/// this one segment. `display` is the same lossless rendering
/// `LiteralsSegment::display` uses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UncompressedDataSegment {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub offset: u64,
    pub size: u64,
    pub hex: String,
    pub display: String,
}

impl UncompressedDataSegment {
    /// Builds an `UncompressedDataSegment` from the raw bytes it covers,
    /// deriving `size`, `hex`, and `display` from `raw` so they cannot drift
    /// from one another or from the bytes actually read.
    pub fn new(offset: u64, raw: &[u8]) -> UncompressedDataSegment {
        UncompressedDataSegment {
            kind: SEGMENT_TYPE_UNCOMPRESSED_DATA,
            offset,
            size: raw.len() as u64,
            hex: hex_string(raw),
            display: display_string(raw),
        }
    }
}

/// One LZ4 Block Format sequence: the semantic grouping of a Token, an
/// optional literal length extension, literals, and (for all but the final
/// sequence) a match offset and optional match length extension. It is not
/// itself a physical field (no offset/size/hex) so it omits "type" from its
/// JSON encoding; `DataBlock::data`'s array index is the sequence number,
/// since compressed blocks hold only sequences. `match_length`,
/// `literal_length_extension`, `match_offset`, and `match_length_extension`
/// serialize as explicit JSON null, not an omitted key, when absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Sequence {
    pub literal_length: u64,
    pub match_length: Option<u64>,
    pub token: TokenSegment,
    pub literal_length_extension: Option<LengthExtensionSegment>,
    pub literals: LiteralsSegment,
    pub match_offset: Option<NumericField<u16>>,
    pub match_length_extension: Option<LengthExtensionSegment>,
}

/// Renders raw bytes as a lossless, human-readable string, used by
/// `LiteralsSegment::display` and `UncompressedDataSegment::display`.
/// Backslash and double-quote escape to `\\` and `\"`; printable ASCII
/// (0x20-0x7e) other than those two passes through unchanged; LF, CR, and TAB
/// escape to `\n`, `\r`, `\t`; every other byte becomes an uppercase-hex
/// `\xHH`. No byte is replaced with a lossy placeholder such as '?'.
pub(crate) fn display_string(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    for &b in raw {
        push_display_byte(&mut out, b);
    }
    out
}

/// Appends one byte's `display_string` rendering to `out`. It is factored out
/// so callers that hex/display-encode a large literal in fixed-size chunks can
/// build the same lossless display incrementally, without first materializing
/// the full byte slice `display_string` expects.
pub(super) fn push_display_byte(out: &mut String, b: u8) {
    const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";
    match b {
        b'\\' => out.push_str("\\\\"),
        b'"' => out.push_str("\\\""),
        b'\n' => out.push_str("\\n"),
        b'\r' => out.push_str("\\r"),
        b'\t' => out.push_str("\\t"),
        0x20..=0x7e => out.push(b as char),
        _ => {
            out.push_str("\\x");
            out.push(HEX_UPPER[(b >> 4) as usize] as char);
            out.push(HEX_UPPER[(b & 0x0f) as usize] as char);
        }
    }
}

/// One Data Block of a normal frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DataBlock {
    pub block_size: BlockSize,
    pub data: Vec<BlockData>,
    pub block_checksum: Option<NumericField<u32>>,
}

/// A normal LZ4 frame (magic number 0x184D2204).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NormalFrame {
    pub magic_number: NumericField<u32>,
    pub frame_descriptor: FrameDescriptor,
    pub data_blocks: Vec<DataBlock>,
    pub end_mark: NumericField<u32>,
    pub content_checksum: Option<NumericField<u32>>,
}

/// A skippable frame's user data: unread bytes reported as a single opaque
/// range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkippableData {
    pub offset: u64,
    pub size: u64,
    pub hex: String,
}

/// A skippable frame (magic number 0x184D2A50-0x184D2A5F).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkippableFrame {
    pub magic_number: NumericField<u32>,
    pub frame_size: NumericField<u32>,
    pub data: SkippableData,
}

#[cfg(test)]
mod blockdata_tests;
#[cfg(test)]
mod tests;
