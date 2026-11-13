use super::format::{LENGTH_EXTENSION_MAX, LENGTH_EXTENSION_THRESHOLD, MATCH_EXTENSION_THRESHOLD, MIN_MATCH_LENGTH};
use super::greedy::greedy_parse;
use super::matches::{longest_matches, MAX_OFFSET};
use super::optimal::optimal_parse;
use super::Parser;

/// A match of a parse: `length` bytes at `start` copied from `offset` bytes
/// earlier. The literals before it run from the end of the previous match, or
/// from the start of the block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ParsedMatch {
    pub(super) start: usize,
    pub(super) length: usize,
    pub(super) offset: usize,
}

/// Encodes one raw LZ4 block with the parse that `parser` selects, even when
/// the result is larger than the input.
pub(super) fn encode_block(src: &[u8], parser: Parser) -> Vec<u8> {
    let matches = longest_matches(src, MAX_OFFSET);
    let parse = match parser {
        Parser::Greedy => greedy_parse(src, &matches),
        Parser::Optimal => optimal_parse(src, &matches),
    };
    encode_parse(src, &parse)
}

/// Encodes the matches of a parse in increasing position order, each with the
/// literals before it, followed by the final literals.
fn encode_parse(src: &[u8], parse: &[ParsedMatch]) -> Vec<u8> {
    let mut dst = Vec::with_capacity(src.len() + src.len() / LENGTH_EXTENSION_MAX + 16);
    let mut anchor = 0;
    for m in parse {
        append_sequence(&mut dst, &src[anchor..m.start], m.offset, m.length);
        anchor = m.start + m.length;
    }
    append_last_literals(&mut dst, &src[anchor..]);
    dst
}

pub(super) fn append_sequence(dst: &mut Vec<u8>, literals: &[u8], offset: usize, length: usize) {
    append_literals(dst, literals, length - MIN_MATCH_LENGTH);
    dst.extend_from_slice(&(offset as u16).to_le_bytes());
    if length >= MATCH_EXTENSION_THRESHOLD {
        append_extension(dst, length - MATCH_EXTENSION_THRESHOLD);
    }
}

pub(super) fn append_last_literals(dst: &mut Vec<u8>, literals: &[u8]) {
    append_literals(dst, literals, 0);
}

/// Appends a token whose low nibble holds `match_code`, the match length minus
/// `MIN_MATCH_LENGTH`, followed by the literal length extension and the
/// literals.
fn append_literals(dst: &mut Vec<u8>, literals: &[u8], match_code: usize) {
    dst.push((literals.len().min(LENGTH_EXTENSION_THRESHOLD) << 4 | match_code.min(LENGTH_EXTENSION_THRESHOLD)) as u8);
    if literals.len() >= LENGTH_EXTENSION_THRESHOLD {
        append_extension(dst, literals.len() - LENGTH_EXTENSION_THRESHOLD);
    }
    dst.extend_from_slice(literals);
}

fn append_extension(dst: &mut Vec<u8>, mut length: usize) {
    while length >= LENGTH_EXTENSION_MAX {
        dst.push(LENGTH_EXTENSION_MAX as u8);
        length -= LENGTH_EXTENSION_MAX;
    }
    dst.push(length as u8);
}
