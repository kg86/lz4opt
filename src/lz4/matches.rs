use snakree::Snakree;

/// The largest offset an LZ4 match can have.
pub const MAX_OFFSET: usize = 65535;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Match {
    pub(super) length: usize,
    pub(super) offset: usize,
}

/// Computes every position's longest match, including overlapping copies.
/// The tree's left boundary is `max(0, i - window)`, while its right boundary
/// includes lookahead. The `window` parameter also permits small-window
/// oracles.
pub(super) fn longest_matches(src: &[u8], window: usize) -> Vec<Match> {
    let mut matches = Vec::with_capacity(src.len());
    for_each_longest_match(src, window, |length, offset| matches.push(Match { length, offset }));
    matches
}

/// Calls `visit(length, offset)` with the longest match at every position `i`
/// of `src`, in order. The match source starts in `[max(0, i - window), i)`
/// and may overlap position `i`; a position without a match yields `(0, 0)`.
/// Among equally long matches, the chosen offset is the one the sliding
/// suffix tree reports.
///
/// This is the matching behind the LZ4 encoders, which use a window of
/// 65,535. It is public only so that the `matchbench` tool times the same
/// code, and is hidden from the documentation: it is not part of the
/// library's supported API.
#[doc(hidden)]
#[allow(clippy::needless_range_loop)]
pub fn for_each_longest_match(src: &[u8], window: usize, mut visit: impl FnMut(usize, usize)) {
    let mut tree = Snakree::new();
    let mut previous = Match::default();
    for i in 0..src.len() {
        while i > window && tree.begin() < i - window {
            tree.pop_front();
        }
        // Moving both source and destination forward preserves the offset
        // and at least previous.length - 1 matching bytes, even after eviction.
        let mut current = Match { length: previous.length.saturating_sub(1), offset: 0 };
        if current.length > 0 {
            current.offset = previous.offset;
        }
        for j in i + current.length..src.len() {
            // End is j or j + 1: a failed probe at the preceding position
            // remains in the tree and must not be appended a second time.
            if tree.end() == j {
                tree.push(src[j]);
            }
            let (pos, repeated) = tree.repeated_suffix();
            let length = j + 1 - i;
            if repeated < length {
                break;
            }
            current = Match { length, offset: i - (pos + repeated - length) };
        }
        visit(current.length, current.offset);
        previous = current;
    }
}
