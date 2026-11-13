use super::UNBOUNDED;
use snakree::Snakree;

/// Configures a [`Parser`]; also embedded in [`EncodeOptions`](super::EncodeOptions).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Maximum backward distance at the start of each phrase. Zero or a value
    /// above `isize::MAX` selects an unbounded history (`isize::MAX`).
    pub window: usize,

    /// Maximum length of a copy. A phrase whose copy reaches `max_length`
    /// ends there, and the following byte becomes its literal even if it would
    /// extend the copy. Zero or a value above `isize::MAX` selects no limit
    /// (`isize::MAX`).
    pub max_length: usize,
}

impl Options {
    /// Returns the options with zero fields replaced by their defaults and
    /// larger fields lowered to `isize::MAX`, the largest value an encoded
    /// header may hold.
    pub(crate) fn with_defaults(mut self) -> Options {
        if self.window == 0 || self.window > UNBOUNDED {
            self.window = UNBOUNDED;
        }
        if self.max_length == 0 || self.max_length > UNBOUNDED {
            self.max_length = UNBOUNDED;
        }
        self
    }

    /// Returns the options with the window and maximum length lowered to the
    /// largest distance and copy length that an input of `size` bytes can use,
    /// so that fixed-width fields are no wider than needed. Zero fields are
    /// treated as unbounded. Both results are at most `size - 1`, since every
    /// phrase ends with a literal, and at least 1. The parse of such an input
    /// is the same as with the original options.
    pub fn fit_to_size(mut self, size: u64) -> Options {
        let limit = size.saturating_sub(1).clamp(1, UNBOUNDED as u64) as usize;
        if self.window == 0 || self.window > limit {
            self.window = limit;
        }
        if self.max_length == 0 || self.max_length > limit {
            self.max_length = limit;
        }
        self
    }
}

/// A copy followed by a literal byte, representing `length + 1` bytes. Decode
/// by copying `length` bytes one at a time from `distance` bytes behind the
/// current output, then appending `next`. `distance` is zero exactly when
/// `length` is zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Token {
    pub distance: usize,
    pub length: usize,
    pub next: u8,
}

/// Computes greedy LZ77 phrases online using a sliding suffix tree's longest
/// repeated suffix. At phrase start `s`, copy sources may start in
/// `[max(0, s - window), s)`. Copy length is limited by `max_length` only, not
/// by the window or the distance. Among equally long copies, the chosen
/// distance is unspecified. Each phrase includes the longest copy of at most
/// `max_length` bytes and the next byte. A copy that reaches the end of the
/// input is shortened by one byte, so that its last byte becomes the literal
/// of the final phrase.
///
/// With [`Snakree`], processing n bytes takes O(n) total time for the byte
/// alphabet. The tree retains the history plus the pending phrase: allocated
/// storage is O(min(n, window + L)), where L <= `max_length` + 1 is the longest
/// phrase seen. Without a `max_length`, a long repetitive input can require
/// storage proportional to its length even with a small window.
pub struct Parser {
    tree: Snakree,
    window: usize,
    max_length: usize,
    start: usize,
    distance: usize,
    finished: bool,
}

impl Parser {
    /// Creates an empty parser.
    pub fn new(opts: Options) -> Parser {
        let opts = opts.with_defaults();
        Parser {
            tree: Snakree::new(),
            window: opts.window,
            max_length: opts.max_length,
            start: 0,
            distance: 0,
            finished: false,
        }
    }

    /// Returns the maximum backward distance at the start of each phrase.
    pub fn window(&self) -> usize {
        self.window
    }

    /// Returns the maximum length of a copy.
    pub fn max_length(&self) -> usize {
        self.max_length
    }

    /// Returns the tree, for tests that inspect the retained window.
    #[cfg(test)]
    pub(crate) fn tree(&self) -> &Snakree {
        &self.tree
    }

    /// Consumes a prefix of `data`, extending the tree by one byte per
    /// consumed byte, and returns the phrase it completed, if any, and how many
    /// bytes were consumed. Consumes the entire current phrase in one call
    /// whenever `data` is long enough: it stops early only once the phrase
    /// completes, never merely because a chunk boundary was reached. If the
    /// phrase completes within `data`, the token's last consumed byte is its
    /// literal; otherwise all of `data` is consumed and no token is returned.
    /// Calling `push` again with the rest of the stream continues the same
    /// in-progress phrase.
    ///
    /// # Panics
    ///
    /// Panics after [`finish`](Parser::finish).
    pub fn push(&mut self, data: &[u8]) -> (Option<Token>, usize) {
        assert!(!self.finished, "lz77: push after finish");
        for (i, &c) in data.iter().enumerate() {
            self.tree.push(c);
            let length = self.tree.end() - self.start;
            let (pos, repeated) = self.tree.repeated_suffix();
            // A full-length copy takes c as its literal whether or not it repeats.
            if repeated < length || length > self.max_length {
                let token = Token { distance: self.distance, length: length - 1, next: c };
                self.start = self.tree.end();
                self.distance = 0;
                // During the phrase, begin stayed fixed at max(0, old_start - window).
                // Every byte is pushed once and contracted at most once overall.
                let keep_from = self.start.saturating_sub(self.window);
                while self.tree.begin() < keep_from {
                    self.tree.pop_front();
                }
                return (Some(token), i + 1);
            }
            // The phrase is the final `length` bytes of the repeated suffix.
            self.distance = self.start - (pos + repeated - length);
        }
        (None, data.len())
    }

    /// Ends the input and returns the pending phrase, if any, with its copy
    /// shortened by one byte and that byte as its literal. Returns `None` for
    /// empty input, an already completed phrase, or repeated calls. `push` is
    /// no longer allowed, even when `finish` returns `None`.
    pub fn finish(&mut self) -> Option<Token> {
        if self.finished {
            return None;
        }
        self.finished = true;
        let length = self.tree.end() - self.start;
        if length == 0 {
            return None;
        }
        // A prefix of the pending copy has the same source.
        let mut token = Token { length: length - 1, next: self.tree.at(self.tree.end() - 1), distance: 0 };
        if token.length > 0 {
            token.distance = self.distance;
        }
        Some(token)
    }
}
