use super::super::format::{LENGTH_EXTENSION_MAX, LENGTH_EXTENSION_THRESHOLD, MIN_MATCH_LENGTH};
use std::io::Read;

use super::errors::{ParseCause, ParseError};
use super::model::*;
use super::parser::FrameReader;

/// Reads a compressed Data Block's sequence content without ever reading past
/// the block's declared `data_size`. It wraps a `FrameReader` and tracks
/// `remaining`, the number of block-data bytes left to read: every read checks
/// its requested size against `remaining` before touching the underlying
/// reader, so a malformed or truncated sequence never consumes bytes belonging
/// to the Block Checksum, the next Block Size, or EndMark. Only reads made
/// through `BlockReader` advance the frame reader's offset while decoding a
/// block's sequences.
pub(super) struct BlockReader<'a, R> {
    fr: &'a mut FrameReader<R>,
    remaining: u64,
    block_idx: usize,
    seq_idx: usize,
}

impl<'a, R: Read> BlockReader<'a, R> {
    /// Returns a reader bounded to `data_size` bytes of a Data Block's
    /// already-validated data range, reading through `fr` from its current
    /// offset. `block_idx` is the Data Block's index, used to build diagnostic
    /// paths; the sequence index defaults to 0 and is updated by
    /// `set_sequence_index` as the sequence loop advances.
    pub(super) fn new(fr: &'a mut FrameReader<R>, data_size: u64, block_idx: usize) -> Self {
        BlockReader { fr, remaining: data_size, block_idx, seq_idx: 0 }
    }

    /// Records the sequence currently being decoded, so subsequent diagnostic
    /// paths read `data_blocks[<block>].data[<sequence>]`.
    fn set_sequence_index(&mut self, i: usize) {
        self.seq_idx = i;
    }

    /// Returns how many block-data bytes are left to read.
    fn remaining_bytes(&self) -> u64 {
        self.remaining
    }

    /// Builds this reader's diagnostic path for `field`:
    /// `data_blocks[<block>].data[<sequence>].<field>`.
    fn path(&self, field: &str) -> String {
        self.path_at(self.seq_idx, field)
    }

    /// Builds a diagnostic path for an arbitrary sequence index, unlike `path`
    /// (which always uses the reader's current sequence index):
    /// `validate_end_of_block` and `verify_sequence_coverage` inspect
    /// sequences other than the one most recently parsed.
    fn path_at(&self, seq_idx: usize, field: &str) -> String {
        format!("data_blocks[{}].data[{}].{}", self.block_idx, seq_idx, field)
    }

    /// Reports a block-boundary error, without touching the underlying reader,
    /// when `n` exceeds `remaining`. The error's offset is the current cursor,
    /// since no byte is read for a rejected request.
    fn check_boundary(&self, n: u64, field: &str) -> Result<(), ParseError> {
        if n > self.remaining {
            return Err(ParseError::msg(
                self.fr.offset,
                self.path(field),
                format!("block boundary exceeded: need {n} byte(s) but only {} remain in block data", self.remaining),
            ));
        }
        Ok(())
    }

    /// Reads exactly `n` bytes for `field`, bounded by `remaining`, and
    /// decrements `remaining` by the bytes consumed. It is meant for small,
    /// fixed-size fields (the 1-byte Token, the 2-byte Match Offset);
    /// large or variable-length reads (Literals) use `read_literal` instead so
    /// they are not read into a single buffer sized by an attacker-controlled
    /// `n`.
    fn read_bytes(&mut self, n: u64, field: &str) -> Result<(Vec<u8>, u64), ParseError> {
        self.check_boundary(n, field)?;
        let before = self.fr.offset;
        let result = self.fr.read_exact(n as usize, &self.path(field));
        // Even on error, the read may have consumed some bytes (a short read):
        // remaining must track the underlying reader's actual position, not
        // just successful reads.
        self.remaining -= self.fr.offset - before;
        result
    }

    fn read_byte(&mut self, field: &str) -> Result<(u8, u64), ParseError> {
        let (raw, offset) = self.read_bytes(1, field)?;
        Ok((raw[0], offset))
    }

    /// Reads exactly `n` bytes for `field`, bounded by `remaining`, and returns
    /// both their hex encoding and their `display_string` rendering. It copies
    /// through a fixed-size buffer rather than allocating one sized by `n`, and
    /// it never materializes a vector of the full literal: the display string
    /// is built byte by byte from each chunk as it is read.
    fn read_literal(&mut self, n: u64, field: &str) -> Result<(String, String, u64), ParseError> {
        let path = self.path(field);
        self.check_boundary(n, field)?;
        if n == 0 {
            return Ok((String::new(), String::new(), self.fr.offset));
        }

        // No capacity from n: it comes from the block's declared
        // (attacker-controlled) length, and a truncated stream must not pay
        // for an allocation sized by it.
        let (mut hex, mut display) = (String::new(), String::new());
        let before = self.fr.offset;
        let result = self.fr.read_chunked(n, &path, |chunk| {
            push_hex(&mut hex, chunk);
            for &b in chunk {
                push_display_byte(&mut display, b);
            }
        });
        // remaining must track the underlying reader's actual position, not just
        // a successful full read: even a short read that ends in error already
        // advanced the offset.
        self.remaining -= self.fr.offset - before;
        let start = result?;
        Ok((hex, display, start))
    }

    /// Decodes a literal or match length's variable-length extension: `nibble`
    /// is the token's literal-length or match-length code (0-15; any value
    /// above 15 is unreachable from an actual token nibble and is treated the
    /// same as 15).
    ///
    /// 1. `nibble < 15`: no extension byte exists; returns `None` and the
    ///    caller uses `nibble` as the effective length contribution directly.
    /// 2. `nibble == 15`: reads one or more bytes. Each 0xff continues (adding
    ///    255 to the additional length); the first byte less than 0xff ends
    ///    the extension and is included in the sum.
    /// 3. Reaching the end of the block's data while still expecting a
    ///    continuation or terminating byte is a "length extension not
    ///    terminated" error, distinct from a generic boundary error since no
    ///    further byte was ever requested from the underlying reader.
    /// 4. The running sum is accumulated with an overflow check before each
    ///    addition.
    ///
    /// `field` names the field being decoded for diagnostics.
    fn read_length_extension(&mut self, nibble: u8, field: &str) -> Result<Option<LengthExtension>, ParseError> {
        if usize::from(nibble) < LENGTH_EXTENSION_THRESHOLD {
            return Ok(None);
        }

        let path = self.path(field);
        let start = self.fr.offset;
        let mut raw = Vec::new();
        let mut additional = 0u64;
        loop {
            if self.remaining == 0 {
                let message = if raw.is_empty() {
                    "length extension truncated: block data ends before any extension byte"
                } else {
                    "length extension not terminated before end of block data"
                };
                return Err(ParseError::msg(self.fr.offset, path, message));
            }
            let (b, b_offset) = self.read_byte(field)?;
            raw.push(b);

            let (next, done) = add_length_extension_byte(additional, b)
                .map_err(|msg| ParseError::new(b_offset, path.clone(), ParseCause::Message(msg.to_string())))?;
            additional = next;
            if done {
                break;
            }
        }

        Ok(Some(LengthExtension {
            offset: start,
            size: raw.len() as u64,
            hex: hex_string(&raw),
            additional_length: additional,
        }))
    }
}

/// The decoded result of a literal/match length's variable-length extension
/// byte sequence: its absolute start offset, byte size, hex, and decoded
/// additional length. `read_length_extension` returns this rather than a model
/// segment type directly, since the caller knows whether to attach it as
/// `literal_length_extension` or `match_length_extension`.
struct LengthExtension {
    offset: u64,
    size: u64,
    hex: String,
    additional_length: u64,
}

/// Adds one length-extension byte's contribution to `acc`, checking for u64
/// overflow before adding rather than after. The second value reports whether
/// `b` terminates the extension (any value other than 0xff); 0xff always
/// continues, contributing 255 either way, so the accumulation is simply
/// `acc + b` in both cases.
fn add_length_extension_byte(acc: u64, b: u8) -> Result<(u64, bool), &'static str> {
    let delta = u64::from(b);
    if delta > u64::MAX - acc {
        return Err("length extension additional length overflows u64");
    }
    Ok((acc + delta, usize::from(b) != LENGTH_EXTENSION_MAX))
}

/// The diagnostic field name used for a sequence's Match Offset in
/// `BlockReader` paths. It is not a segment type constant because
/// `match_offset` carries no "type" in its JSON encoding (it reuses
/// `NumericField<u16>` directly).
const FIELD_MATCH_OFFSET: &str = "match_offset";

/// Decodes one LZ4 Block Format sequence from `br`: Token, optional literal
/// length extension, literals, and -- when block data remains once the
/// literals have been read -- a match offset and optional match length
/// extension. Child fields are built in physical order (token,
/// literal_length_extension, literals, match_offset, match_length_extension)
/// since each is read directly off `br`, so every field's offset equals the
/// previous field's offset+size by construction.
///
/// Whether *this* sequence has a match is decided the same way the sequence
/// loop later decides whether to keep looping: if no block data remains after
/// the literals, this is a literal-only sequence and `match_length`,
/// `match_offset`, and `match_length_extension` stay `None`.
///
/// Callers must call `br.set_sequence_index` with this sequence's index before
/// calling `parse_sequence`, since every diagnostic path it builds uses
/// whatever index `br` currently holds.
fn parse_sequence<R: Read>(br: &mut BlockReader<'_, R>) -> Result<Sequence, ParseError> {
    let (token_byte, token_offset) = br.read_byte(SEGMENT_TYPE_TOKEN)?;
    let token = TokenSegment::new(token_offset, token_byte);
    let (literal_code, match_code) = (token.literal_length_code, token.match_length_code);

    let lit_ext = br.read_length_extension(literal_code, SEGMENT_TYPE_LITERAL_LENGTH_EXTENSION)?;
    let literal_length = effective_literal_length(literal_code, lit_ext.as_ref())
        .map_err(|cause| wrap_length_overflow(br, lit_ext.as_ref(), SEGMENT_TYPE_LITERAL_LENGTH_EXTENSION, cause))?;

    let (literal_hex, literal_display, literal_offset) = br.read_literal(literal_length, SEGMENT_TYPE_LITERALS)?;

    let mut seq = Sequence {
        literal_length,
        match_length: None,
        token,
        literal_length_extension: to_length_extension_segment(lit_ext, SEGMENT_TYPE_LITERAL_LENGTH_EXTENSION),
        literals: LiteralsSegment::new(literal_offset, literal_hex, literal_display),
        match_offset: None,
        match_length_extension: None,
    };

    if br.remaining_bytes() == 0 {
        return Ok(seq);
    }

    let (offset_raw, offset_start) = br.read_bytes(2, FIELD_MATCH_OFFSET)?;
    let offset_value = decode_u16_le(&offset_raw);
    if offset_value == 0 {
        return Err(ParseError::msg(
            offset_start,
            br.path(FIELD_MATCH_OFFSET),
            "invalid match offset: value must not be zero",
        ));
    }
    seq.match_offset = Some(NumericField::new(offset_start, offset_value, &offset_raw));

    let match_ext = br.read_length_extension(match_code, SEGMENT_TYPE_MATCH_LENGTH_EXTENSION)?;
    let match_length = effective_match_length(match_code, match_ext.as_ref())
        .map_err(|cause| wrap_length_overflow(br, match_ext.as_ref(), SEGMENT_TYPE_MATCH_LENGTH_EXTENSION, cause))?;
    seq.match_length = Some(match_length);
    seq.match_length_extension = to_length_extension_segment(match_ext, SEGMENT_TYPE_MATCH_LENGTH_EXTENSION);

    Ok(seq)
}

/// Combines a token's literal-length nibble (`code`) with its optional decoded
/// length extension into the sequence's effective literal length, checking for
/// u64 overflow before adding.
fn effective_literal_length(code: u8, ext: Option<&LengthExtension>) -> Result<u64, String> {
    add_length_extension_result(u64::from(code), ext, "literal length")
}

/// Combines a token's match-length nibble (`code`) with its optional decoded
/// length extension into the sequence's effective match length (4 + code +
/// additional length), checking for u64 overflow before adding. `4 + code`
/// never overflows on its own (code is a 4-bit nibble), so only the addition
/// of the extension's additional length needs checking.
fn effective_match_length(code: u8, ext: Option<&LengthExtension>) -> Result<u64, String> {
    add_length_extension_result(MIN_MATCH_LENGTH as u64 + u64::from(code), ext, "match length")
}

/// Adds the extension's additional length (if any) to `base`, checking for u64
/// overflow before adding rather than after.
fn add_length_extension_result(base: u64, ext: Option<&LengthExtension>, what: &str) -> Result<u64, String> {
    let Some(ext) = ext else { return Ok(base) };
    if ext.additional_length > u64::MAX - base {
        return Err(format!("{what} overflows u64"));
    }
    Ok(base + ext.additional_length)
}

/// Turns an overflow error from `effective_literal_length` or
/// `effective_match_length` into a `ParseError` attributed to the extension's
/// start offset: the extension is never absent here, since the nibble alone
/// -- at most 15 for literals, 19 for matches -- can never overflow u64 on its
/// own, so the extension's decoded value is always what overflowed.
fn wrap_length_overflow<R: Read>(
    br: &BlockReader<'_, R>,
    ext: Option<&LengthExtension>,
    field: &str,
    cause: String,
) -> ParseError {
    let offset = ext.map_or(br.fr.offset, |e| e.offset);
    ParseError::new(offset, br.path(field), ParseCause::Message(cause))
}

/// Converts a decoded `LengthExtension` into the model's
/// `LengthExtensionSegment`, tagging it with `kind` since only the caller
/// knows whether it is a literal or match length extension.
fn to_length_extension_segment(ext: Option<LengthExtension>, kind: &'static str) -> Option<LengthExtensionSegment> {
    ext.map(|e| LengthExtensionSegment::new(e.offset, e.size, e.hex, e.additional_length, kind))
}

/// The End of block condition thresholds, quoting the LZ4 Block Format
/// description's decoder-compatibility requirements.
const MIN_LAST_LITERAL_SIZE: u64 = super::super::format::MIN_LAST_LITERAL_SIZE as u64;
const MIN_MATCH_DISTANCE_FROM_END: u64 = super::super::format::MIN_MATCH_DISTANCE_FROM_END as u64;

/// Adds `n` to `expanded_size` (the cursor into the block's decompressed
/// output), rejecting both u64 overflow and exceeding the Frame Descriptor's
/// Block Maximum Size. `offset` and `field` identify where to attribute either
/// error.
fn checked_add_expanded_size<R: Read>(
    br: &BlockReader<'_, R>,
    expanded_size: u64,
    n: u64,
    bd: &Bd,
    offset: u64,
    field: &str,
) -> Result<u64, ParseError> {
    if n > u64::MAX - expanded_size {
        return Err(ParseError::msg(offset, br.path(field), "expanded block size overflows u64"));
    }
    let sum = expanded_size + n;
    if sum > u64::from(bd.block_max_size_bytes) {
        return Err(ParseError::msg(
            offset,
            br.path(field),
            format!("expanded block size {sum} exceeds Block Maximum Size {}", bd.block_max_size_bytes),
        ));
    }
    Ok(sum)
}

/// Parses every sequence of one compressed block: it repeats the
/// single-sequence parse until the block's declared data is exhausted, tracks
/// the block's expanded (decompressed) size against the Frame Descriptor's
/// Block Maximum Size, and rejects a match that is not followed by a final
/// literal-only sequence. Once the loop ends it checks the End of block
/// condition and, as an internal postcondition, that the parsed sequences'
/// physical fields cover `br`'s declared data range without gap or overlap.
/// `br` must be freshly constructed (`remaining_bytes` reflects the whole
/// block) and positioned at the start of the block's data.
pub(super) fn parse_sequence_loop<R: Read>(br: &mut BlockReader<'_, R>, bd: &Bd) -> Result<Vec<Sequence>, ParseError> {
    let data_start = br.fr.offset;
    let data_size = br.remaining_bytes();

    let mut sequences = Vec::with_capacity(1);
    let mut expanded_size = 0u64;

    for seq_idx in 0.. {
        br.set_sequence_index(seq_idx);

        // parse_sequence itself applies the "remaining == 0 right after the
        // literal" rule to decide whether to read match fields at all, so
        // `match_length == None` is the authoritative signal that this is the
        // block's final, literal-only sequence -- not a second, independent
        // check of the remaining bytes here, which by this point may already
        // reflect bytes parse_sequence consumed for this sequence's own match
        // fields.
        let seq = parse_sequence(br)?;

        let literal_end = seq.literals.offset + seq.literals.size;
        expanded_size =
            checked_add_expanded_size(br, expanded_size, seq.literal_length, bd, literal_end, SEGMENT_TYPE_LITERALS)?;

        let Some(match_length) = seq.match_length else {
            sequences.push(seq);
            break;
        };

        // Attributed to match_offset, not match_length_extension: the latter
        // is only present when the match-length code is 15, but match_offset
        // is guaranteed present for every match (it is what makes the
        // sequence match-bearing at all).
        let match_offset = seq.match_offset.as_ref().expect("a match has an offset").offset;
        expanded_size =
            checked_add_expanded_size(br, expanded_size, match_length, bd, match_offset, FIELD_MATCH_OFFSET)?;

        sequences.push(seq);

        if br.remaining_bytes() == 0 {
            return Err(ParseError::msg(
                br.fr.offset,
                br.path(FIELD_MATCH_OFFSET),
                "block ends immediately after a match, with no final literal-only sequence",
            ));
        }
    }

    // verify_sequence_coverage runs first: it is an internal-bug detector over
    // the same lengths/offsets validate_end_of_block reasons about, so a parser
    // bug should surface as that internal error rather than a misleading
    // "End of block condition violated" input rejection.
    verify_sequence_coverage(br, &sequences, data_start, data_size)?;
    validate_end_of_block(br, &sequences, expanded_size)?;

    Ok(sequences)
}

/// Enforces the End of block condition: a decoder-compatibility requirement,
/// not merely a structural one, so it is checked after sequences have already
/// been fully parsed. It is a hard error: a structurally valid sequence
/// breakdown can still violate this LZ4-mandated condition. `sequences` must be
/// non-empty; `parse_sequence_loop`, its only caller, always appends at least
/// one sequence before reaching this call.
fn validate_end_of_block<R: Read>(
    br: &BlockReader<'_, R>,
    sequences: &[Sequence],
    expanded_size: u64,
) -> Result<(), ParseError> {
    let last_idx = sequences.len() - 1;
    let last = &sequences[last_idx];

    if expanded_size >= MIN_LAST_LITERAL_SIZE {
        if last.literal_length < MIN_LAST_LITERAL_SIZE {
            return Err(ParseError::msg(
                last.literals.offset,
                br.path_at(last_idx, SEGMENT_TYPE_LITERALS),
                format!(
                    "End of block condition violated: expanded block size {expanded_size} >= {MIN_LAST_LITERAL_SIZE} requires the last sequence's literal length >= {MIN_LAST_LITERAL_SIZE}, got {}",
                    last.literal_length
                ),
            ));
        }
    } else if sequences.len() != 1 {
        return Err(ParseError::msg(
            last.token.offset,
            br.path_at(last_idx, SEGMENT_TYPE_TOKEN),
            format!(
                "End of block condition violated: expanded block size {expanded_size} < {MIN_LAST_LITERAL_SIZE} requires exactly 1 sequence whose output is entirely literal, got {} sequences",
                sequences.len()
            ),
        ));
    }

    let mut pos = 0u64;
    for (i, seq) in sequences.iter().enumerate() {
        pos += seq.literal_length;
        let Some(match_length) = seq.match_length else { continue };
        let match_start = pos;
        pos += match_length;

        if expanded_size - match_start < MIN_MATCH_DISTANCE_FROM_END {
            let offset = seq.match_offset.as_ref().expect("a match has an offset").offset;
            return Err(ParseError::msg(
                offset,
                br.path_at(i, FIELD_MATCH_OFFSET),
                format!(
                    "End of block condition violated: every match must start at least {MIN_MATCH_DISTANCE_FROM_END} byte(s) before the block's expanded end, but this one starts {} byte(s) before",
                    expanded_size - match_start
                ),
            ));
        }
    }

    Ok(())
}

/// One physical field's offset and size, as needed to check contiguous,
/// non-overlapping coverage; it is not a JSON-facing type.
struct CoverageField {
    offset: u64,
    size: u64,
    path: String,
}

/// Lists a sequence's physical fields in the encoded order `parse_sequence`
/// built them in: token, optional literal_length_extension, literals, optional
/// match_offset, optional match_length_extension.
fn sequence_coverage_fields<R: Read>(br: &BlockReader<'_, R>, seq_idx: usize, seq: &Sequence) -> Vec<CoverageField> {
    let field = |offset, size, name: &str| CoverageField { offset, size, path: br.path_at(seq_idx, name) };
    let mut fields = Vec::with_capacity(5);
    fields.push(field(seq.token.offset, seq.token.size, SEGMENT_TYPE_TOKEN));
    if let Some(e) = &seq.literal_length_extension {
        fields.push(field(e.offset, e.size, SEGMENT_TYPE_LITERAL_LENGTH_EXTENSION));
    }
    fields.push(field(seq.literals.offset, seq.literals.size, SEGMENT_TYPE_LITERALS));
    if let Some(m) = &seq.match_offset {
        fields.push(field(m.offset, 2, FIELD_MATCH_OFFSET));
    }
    if let Some(e) = &seq.match_length_extension {
        fields.push(field(e.offset, e.size, SEGMENT_TYPE_MATCH_LENGTH_EXTENSION));
    }
    fields
}

/// Checks, as an internal postcondition rather than an input-validation step,
/// that `sequences`' physical fields cover `[data_start, data_start +
/// data_size)` contiguously with no gap, overlap, or leftover bytes. Every
/// offset `parse_sequence` assigns comes directly from `BlockReader`'s
/// sequential reads, so a mismatch here can only mean a parser bug, not
/// malformed input; it is still reported as a regular `ParseError`, consistent
/// with the rest of this module, rather than a panic. `sequences` must be
/// non-empty.
fn verify_sequence_coverage<R: Read>(
    br: &BlockReader<'_, R>,
    sequences: &[Sequence],
    data_start: u64,
    data_size: u64,
) -> Result<(), ParseError> {
    let mut cursor = data_start;
    for (i, seq) in sequences.iter().enumerate() {
        for f in sequence_coverage_fields(br, i, seq) {
            if f.offset != cursor {
                return Err(ParseError::msg(
                    f.offset,
                    f.path,
                    format!(
                        "internal error: segment coverage broken, expected next physical field at offset {cursor}, found {}",
                        f.offset
                    ),
                ));
            }
            cursor += f.size;
        }
    }
    if cursor != data_start + data_size {
        return Err(ParseError::msg(
            cursor,
            br.path_at(sequences.len() - 1, SEGMENT_TYPE_LITERALS),
            format!(
                "internal error: segment coverage totals {} byte(s), want block data_size {data_size}",
                cursor - data_start
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;
