use super::block::ParsedMatch;
use super::format::{
    length_extension_slack, LENGTH_EXTENSION_MAX, MATCH_EXTENSION_THRESHOLD, MIN_LAST_LITERAL_SIZE,
    MIN_MATCH_DISTANCE_FROM_END, MIN_MATCH_LENGTH,
};
use super::matches::Match;

/// Returns the parse of src with the smallest block size, given the longest
/// match at each position.
///
/// Every match has at least `MIN_MATCH_LENGTH` bytes, starts at least
/// `MIN_MATCH_DISTANCE_FROM_END` bytes before the end of src, and ends at least
/// `MIN_LAST_LITERAL_SIZE` bytes before it.
/// A match from `source` may take any length from `MIN_MATCH_LENGTH` to the
/// length of `matches[source]`, as far as that end limit allows, always with the
/// offset of `matches[source]`.
/// This is O(n) time and O(n) space, where n is the length of src; see `OptimalDp`
/// for the DP.
pub(super) fn optimal_parse(src: &[u8], matches: &[Match]) -> Vec<ParsedMatch> {
    let n = src.len();
    if n < MIN_MATCH_DISTANCE_FROM_END + 1 {
        return Vec::new();
    }
    let mut dp = OptimalDp::new(n, matches);
    for end in 1..=n {
        dp.advance_window(end);
        let (match_cost, source) = dp.best_source(end);
        dp.extend_literal();
        dp.settle(end, match_cost, source);
    }
    dp.backtrack()
}

/// Shortest-path DP over positions, advanced one `end` position at a time.
///
/// A parse alternates literal runs and matches, so the DP tracks two costs, both
/// in bytes, for a prefix of src:
///
/// - `literal_cost[pos]` is the smallest size of src[..pos] ending in an open
///   literal run, counting the run's bytes and its extension bytes.
/// - `end_cost[end]` is the smallest size of src[..end] ending with a match that
///   ends at `end`. It is computed in `settle` and not stored.
///
/// They depend on each other as follows:
///
/// - `literal_cost[pos]` is the minimum over `start` of
///   `end_cost[start] + run_len + ext(run_len)` with `run_len = pos - start`.
/// - `end_cost[end]` is `3 + match_cost`, where `match_cost` is the minimum of
///   `literal_cost[source] + ext(end - MIN_MATCH_LENGTH - source)` over the
///   sources that reach `end`. The 3 is the token and the offset of the match.
///
/// Here `ext(len)` is the number of length extension bytes: 0 for
/// `len < LENGTH_EXTENSION_THRESHOLD`, and `1 + (len - LENGTH_EXTENSION_THRESHOLD) / LENGTH_EXTENSION_MAX` otherwise.
/// A source reaches `end` if a match from it can end at `end`.
///
/// Per `end`, `advance_window` and `best_source` find the best match into `end`,
/// `extend_literal` extends the literal candidate, and `settle` picks the
/// cheaper of the two as `literal_cost[end]`.
struct OptimalDp<'a> {
    matches: &'a [Match],
    n: usize,
    /// Last legal match start.
    last_match_start: usize,
    /// Largest match end from each source, limited to `MIN_LAST_LITERAL_SIZE`
    /// bytes before the end of src.
    match_reach: Vec<i32>,
    /// Smallest size of src[..pos] ending in an open literal run.
    literal_cost: Vec<i32>,
    /// Start of the literal run that `literal_cost[pos]` ends.
    literal_start: Vec<i32>,
    /// Source of the match ending at `end` that gives `end_cost[end]`, or -1.
    match_source: Vec<i32>,
    /// Sliding-window minimum of `literal_cost` over the sources whose match to
    /// `end` needs no extension byte.
    near_sources: MonotoneQueue,
    /// Sliding-window minima of `literal_cost` over the sources whose match to
    /// `end` needs extension bytes.
    far_sources: FarSources,
    /// Cost and length of the open literal run that is the `literal_cost`
    /// candidate at the current position.
    run_cost: i32,
    run_len: usize,
    /// First source that may still reach the current position.
    first_source: usize,
}

impl<'a> OptimalDp<'a> {
    fn new(n: usize, matches: &'a [Match]) -> Self {
        let last_match_start = n - MIN_MATCH_DISTANCE_FROM_END;
        let match_reach = (0..=last_match_start)
            .map(|source| (source + matches[source].length.min(n - MIN_LAST_LITERAL_SIZE - source)) as i32)
            .collect();
        Self {
            matches,
            n,
            last_match_start,
            match_reach,
            literal_cost: vec![0; n + 1],
            literal_start: vec![0; n + 1],
            match_source: vec![-1; n + 1],
            near_sources: MonotoneQueue::new(n + 1),
            far_sources: FarSources::new(last_match_start + 1),
            run_cost: 0,
            run_len: 0,
            first_source: 0,
        }
    }

    /// Admits the sources that become usable at `end` and expires those that
    /// cannot reach it.
    ///
    /// Since `match_reach` is nondecreasing, the sources reaching `end` form the
    /// interval from `first_source` to `end - MIN_MATCH_LENGTH`, and
    /// `first_source` never moves back.
    /// The sources whose match to `end` needs no extension byte, those within
    /// `LENGTH_EXTENSION_THRESHOLD` bytes of the shortest match, lie in a sliding
    /// window in `near_sources`, and the others are in `far_sources`.
    fn advance_window(&mut self, end: usize) {
        let (literal_cost, last_match_start) = (&self.literal_cost, self.last_match_start);
        // The newest source whose match of the minimum length ends at `end`.
        if end >= MIN_MATCH_LENGTH && end - MIN_MATCH_LENGTH <= last_match_start {
            self.near_sources.push((end - MIN_MATCH_LENGTH) as i32, |source| literal_cost[source as usize]);
        }
        // The newest source whose match needs an extension byte to end at `end`.
        if end >= MATCH_EXTENSION_THRESHOLD {
            let far_source = end - MATCH_EXTENSION_THRESHOLD;
            if far_source >= self.first_source && far_source <= last_match_start {
                self.far_sources.push(far_source, literal_cost);
            }
        }
        while self.first_source < end
            && (self.first_source > last_match_start || (self.match_reach[self.first_source] as usize) < end)
        {
            if self.first_source <= last_match_start {
                self.far_sources.evict(self.first_source, literal_cost);
            }
            self.first_source += 1;
        }
        self.near_sources.pop_before(self.first_source.max(end.saturating_sub(MATCH_EXTENSION_THRESHOLD - 1)) as i32);
    }

    /// Returns the minimum of `literal_cost[source] + ext(end - MIN_MATCH_LENGTH - source)`
    /// over the sources reaching `end` and the source attaining it, or
    /// `(i32::MAX, -1)` if no source reaches `end`.
    /// The 3 fixed bytes of the match are added by `settle`.
    fn best_source(&self, end: usize) -> (i32, i32) {
        let (mut match_cost, mut source) = (i32::MAX, -1i32);
        if let Some(near_source) = self.near_sources.front() {
            (match_cost, source) = (self.literal_cost[near_source as usize], near_source);
        }
        if end >= MATCH_EXTENSION_THRESHOLD {
            let (quot, rem) = (
                ((end - MATCH_EXTENSION_THRESHOLD) / LENGTH_EXTENSION_MAX) as i32,
                (end - MATCH_EXTENSION_THRESHOLD) % LENGTH_EXTENSION_MAX,
            );
            let (far_cost, far_source) = self.far_sources.min(quot, rem);
            if far_source >= 0 && far_cost < match_cost {
                (match_cost, source) = (far_cost, far_source);
            }
        }
        (match_cost, source)
    }

    /// Extends the literal candidate by one literal.
    ///
    /// `literal_cost` keeps one candidate run (`run_cost` and `run_len`).
    /// Of two runs, the one with smaller cost, or with equal cost the one
    /// farther from its next extension byte, is never overtaken.
    /// Extending both runs by one literal adds 1 to both costs, plus 1 to a run
    /// that gains an extension byte; a run that gains one is then
    /// `LENGTH_EXTENSION_MAX` literals from its next, the most any run can be.
    /// Each extension keeps the preferred run preferred:
    ///
    /// - If neither run or both gain a byte, the comparison is unchanged.
    /// - If only the other run gains one, the preferred run becomes cheaper
    ///   still.
    /// - If only the preferred run gains one, it was strictly cheaper, since at
    ///   equal cost it is no nearer its next byte, so its cost is still at most
    ///   the other's and it is now the farther one from its next byte.
    fn extend_literal(&mut self) {
        // The literal that uses up the slack adds an extension byte.
        self.run_cost += 1 + i32::from(length_extension_slack(self.run_len) == 1);
        self.run_len += 1;
    }

    /// Finalizes `literal_cost[end]`, given the best match into `end` from
    /// `source` with `match_cost` from `best_source`.
    /// The match gives `end_cost = match_cost + 3`, which replaces the literal
    /// candidate with a new empty run starting at `end` if it is preferred.
    fn settle(&mut self, end: usize, match_cost: i32, source: i32) {
        if source >= 0 {
            let end_cost = match_cost + 3;
            self.match_source[end] = source;
            if end_cost < self.run_cost
                || end_cost == self.run_cost && length_extension_slack(0) > length_extension_slack(self.run_len)
            {
                (self.run_cost, self.run_len) = (end_cost, 0);
            }
        }
        self.literal_cost[end] = self.run_cost;
        self.literal_start[end] = (end - self.run_len) as i32;
    }

    /// Walks back from the final literal run, then restores source order.
    fn backtrack(&self) -> Vec<ParsedMatch> {
        let mut parse = Vec::new();
        let mut pos = self.literal_start[self.n] as usize;
        while pos > 0 {
            let source = self.match_source[pos] as usize;
            parse.push(ParsedMatch { start: source, length: pos - source, offset: self.matches[source].offset });
            pos = self.literal_start[source] as usize;
        }
        parse.reverse();
        parse
    }
}

/// Sliding-window minima for sources whose match needs extension bytes.
///
/// Write `end - MATCH_EXTENSION_THRESHOLD = LENGTH_EXTENSION_MAX * quot + rem` and
/// `source = LENGTH_EXTENSION_MAX * source_quot + residue`.
/// Then a match from `source` to `end` needs
/// `1 + quot - source_quot - [residue > rem]` extension bytes, so its cost is
/// `literal_cost[source] - source_quot + 1 + quot - [residue > rem]`.
/// Each residue keeps a sliding-window minimum of
/// `literal_cost[source] - source_quot`, and the tree indexes the queue fronts,
/// so the minimum over `residue <= rem` and `residue > rem` is a tree query.
struct FarSources {
    queues: Vec<MonotoneQueue>,
    tree: ResidueTree,
}

impl FarSources {
    fn new(source_count: usize) -> Self {
        Self {
            queues: (0..LENGTH_EXTENSION_MAX)
                .map(|_| MonotoneQueue::new(source_count / LENGTH_EXTENSION_MAX + 1))
                .collect(),
            tree: ResidueTree::new(),
        }
    }

    /// Returns `literal_cost[source] - source_quot`, the key a residue minimizes.
    fn key(literal_cost: &[i32], source: i32) -> i32 {
        literal_cost[source as usize] - source / LENGTH_EXTENSION_MAX as i32
    }

    fn refresh(&mut self, residue: usize, literal_cost: &[i32]) {
        match self.queues[residue].front() {
            Some(source) => self.tree.set(residue, Self::key(literal_cost, source), source),
            None => self.tree.set(residue, i32::MAX, -1),
        }
    }

    /// Adds a source in increasing position order, using its finalized `literal_cost`.
    fn push(&mut self, source: usize, literal_cost: &[i32]) {
        let residue = source % LENGTH_EXTENSION_MAX;
        self.queues[residue].push(source as i32, |source| Self::key(literal_cost, source));
        self.refresh(residue, literal_cost);
    }

    /// Removes the source, called for every expired position in increasing order.
    fn evict(&mut self, source: usize, literal_cost: &[i32]) {
        // No queue holds a position below the source, so only its residue can change.
        let residue = source % LENGTH_EXTENSION_MAX;
        if self.queues[residue].pop_before(source as i32 + 1) {
            self.refresh(residue, literal_cost);
        }
    }

    /// Returns the minimum match cost and its source for
    /// `end - MATCH_EXTENSION_THRESHOLD = LENGTH_EXTENSION_MAX * quot + rem`, or
    /// `(i32::MAX, -1)` if no source remains.
    fn min(&self, quot: i32, rem: usize) -> (i32, i32) {
        let (mut match_cost, mut source) = (i32::MAX, -1);
        // Residues up to `rem` pay one more extension byte than those above it.
        let (key, pos) = self.tree.min(0, rem + 1);
        if pos >= 0 {
            (match_cost, source) = (key + 1 + quot, pos);
        }
        let (key, pos) = self.tree.min(rem + 1, LENGTH_EXTENSION_MAX);
        if pos >= 0 && key + quot < match_cost {
            (match_cost, source) = (key + quot, pos);
        }
        (match_cost, source)
    }
}

/// Queue of increasing positions with strictly increasing keys from front to
/// back, for sliding-window minima. Its capacity bounds the number of pushes.
struct MonotoneQueue {
    items: Vec<i32>,
    head: usize,
    tail: usize,
}

impl MonotoneQueue {
    fn new(capacity: usize) -> MonotoneQueue {
        MonotoneQueue { items: vec![0; capacity], head: 0, tail: 0 }
    }

    /// Appends `i` after dropping the positions whose key is at least `i`'s.
    fn push(&mut self, i: i32, key: impl Fn(i32) -> i32) {
        let k = key(i);
        while self.tail > self.head && key(self.items[self.tail - 1]) >= k {
            self.tail -= 1;
        }
        self.items[self.tail] = i;
        self.tail += 1;
    }

    /// Drops the positions below `i` and returns whether there were any.
    fn pop_before(&mut self, i: i32) -> bool {
        let head = self.head;
        while self.head < self.tail && self.items[self.head] < i {
            self.head += 1;
        }
        self.head > head
    }

    fn front(&self) -> Option<i32> {
        (self.head < self.tail).then(|| self.items[self.head])
    }
}

/// Segment tree over the 255 residues, holding each residue's minimum key and
/// the position attaining it.
struct ResidueTree {
    value: [i32; 512],
    pos: [i32; 512],
}

impl ResidueTree {
    fn new() -> ResidueTree {
        ResidueTree { value: [i32::MAX; 512], pos: [-1; 512] }
    }

    fn set(&mut self, c: usize, value: i32, pos: i32) {
        let mut k = c + 256;
        self.value[k] = value;
        self.pos[k] = pos;
        while k > 1 {
            k >>= 1;
            let (mut l, r) = (2 * k, 2 * k + 1);
            if self.value[r] < self.value[l] {
                l = r;
            }
            self.value[k] = self.value[l];
            self.pos[k] = self.pos[l];
        }
    }

    /// Returns the minimum key over residues `[lo, hi)` and the position attaining it, or
    /// -1 if every residue is empty.
    fn min(&self, lo: usize, hi: usize) -> (i32, i32) {
        let (mut value, mut pos) = (i32::MAX, -1);
        let mut take = |k: usize| {
            if self.pos[k] >= 0 && self.value[k] < value {
                (value, pos) = (self.value[k], self.pos[k]);
            }
        };
        let (mut lo, mut hi) = (lo + 256, hi + 256);
        while lo < hi {
            if lo & 1 == 1 {
                take(lo);
                lo += 1;
            }
            if hi & 1 == 1 {
                hi -= 1;
                take(hi);
            }
            lo >>= 1;
            hi >>= 1;
        }
        (value, pos)
    }
}
