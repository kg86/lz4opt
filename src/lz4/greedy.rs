use super::block::ParsedMatch;
use super::format::{MIN_LAST_LITERAL_SIZE, MIN_MATCH_DISTANCE_FROM_END, MIN_MATCH_LENGTH};
use super::matches::Match;

/// Returns the parse that takes the longest legal match at every parse
/// position, given the longest match at each position.
pub(super) fn greedy_parse(src: &[u8], matches: &[Match]) -> Vec<ParsedMatch> {
    let mut parse = Vec::new();
    let mut i = 0;
    // The last match must start at least 12 bytes before the end.
    while i + MIN_MATCH_DISTANCE_FROM_END <= src.len() {
        let length = matches[i].length.min(src.len() - MIN_LAST_LITERAL_SIZE - i);
        if length < MIN_MATCH_LENGTH {
            i += 1;
            continue;
        }
        parse.push(ParsedMatch { start: i, length, offset: matches[i].offset });
        i += length;
    }
    parse
}
