# LZ4 encoders: greedy and optimal parsing

The `lz4` module (`src/lz4/`) encodes standard LZ4 frames that the official `lz4` CLI decodes.
It has two parsers:

- `Parser::Optimal` (LZ4opt), the default, minimizes the compressed size of each block over every parse the LZ4 format allows.
- `Parser::Greedy` takes the exact longest match at every position, shortened only by the end-of-block rules.

Both take their matches from the sliding suffix tree of the `snakree` crate, through its public API only.
The module is a proof of concept for understanding LZ4, not a fast encoder.
The [paper](../paper/lz4opt-en.pdf) explains the algorithm and the experiments; this document describes the code and follows the paper's notation.

## Usage

```rust
use lz4opt::lz4::{self, Options, Parser};

// Options::default() selects 4 MiB blocks and Parser::Optimal.
let options = Options { block_size: lz4::BLOCK_64_KIB, parser: Parser::Greedy };
lz4::encode(dst, src, &options)?; // dst: impl Write, src: impl Read
```

`Options::block_size` accepts `BLOCK_64_KIB`, `BLOCK_256_KIB`, `BLOCK_1_MIB`, or `BLOCK_4_MIB`; zero selects 4 MiB.
Any other size returns `Error::UnsupportedBlockSize` before any I/O.
The encoder cuts the input into fixed blocks independently of `read` boundaries and does not close either stream.
Read and write errors are returned with context, and `dst` may then hold an incomplete frame.

The `lz4opt` command wraps this, with the same defaults:

```sh
cargo run --release --bin lz4opt < input > output.lz4
cargo run --release --bin lz4opt -- -parser greedy -block-size 65536 < input > output.lz4
lz4 -t output.lz4
lz4 -dc output.lz4 > restored
cmp input restored
```

## Format

The output is an LZ4 frame with independent blocks and a content checksum (XXH32).
Block checksums, content size, and dictionary ID are not written.
A block that does not get strictly smaller is stored uncompressed.
There is no decoder in this module; use the official `lz4` tool, or `lz4frame-json` to inspect frames.
The block format is in the [LZ4 specification](https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md) and the [paper](../paper/lz4opt-en.pdf).

## Longest matches

`longest_matches` (`matches.rs`) computes, for every position `i` of a block, the longest match whose source starts at most 65,535 bytes earlier, with its offset.
Write `lpf(i)` for its length.
The sliding suffix tree reuses the previous position's work, so the whole block takes O(n) time.

## Greedy parsing

`encode_matches` (`block.rs`) takes the longest match at each position that can start a legal match and then advances by its length.
This does not minimize the size: a long match now can cost more than one literal and a longer match one byte later.
The tests in `src/lz4/tests.rs` include such an input.

## Optimal parsing

`optimal_parse` (`optimal.rs`) finds the parse of smallest size.
The [paper](../paper/lz4opt-en.pdf) gives the derivation and the proofs, including the O(n) bound; this section states the essence.

- A parse is a path in a graph whose states are a position and the length of the literal run not yet closed by a match.
  The edge weights are the byte costs of a literal and of a match, and the shortest path gives the smallest size.
- Keeping one state per position, the cheaper candidate and on a tie the one farther from its next extension byte, loses nothing and keeps `opt[i]` and the run length `b[i]`.
- Updating each position from all the sources that reach it at once, with monotone queues and a segment tree, makes the whole block `O(n)` time and space.

The block boundaries are fixed at the frame's block size, and choosing them is not implemented.

| Paper | `optimal.rs` |
| --- | --- |
| `lpf(i)` | `matches[i].length` |
| `opt[i]`, `b[i]` | `literal_cost[i]`, `run_len` and `literal_start[i]` |
| match candidate into `j` | `end_cost`, computed in `settle` and not stored |
| near and far sources | `near_sources`, `far_sources` |

## Cost and validation

The tree keeps history plus lookahead, and the match and DP arrays take about 40 bytes per byte of the block, so a 4 MiB block allocates well over 100 MiB.
The module does not promise the speed or memory of the official hash-based encoder.

The tests in `src/lz4/tests.rs` check the matches, the greedy parse, and the optimal sizes against exhaustive searches, and round-trip frames through the official CLI.

```sh
cargo test --release lz4
```

The round-trip test skips when `lz4` is not installed; CI installs it.

## Measurements

The [experiments](../../experiments/lz4/README.md) compare the official levels with this encoder on the Silesia and repetitive corpora, and [benchmarking](../benchmarking.md) describes how to benchmark two revisions of this crate.
The paper discusses the results.
On Silesia the optimal parser was only 282 bytes smaller than LZ4HC `-12` but took about 3.6 times as long and 25 times the memory, and on some repetitive files `-12` was noticeably larger.

## References

- [An Optimal LZ4 Encoder: How Much Smaller Can LZ4 Get?](../paper/lz4opt-en.pdf), the paper this implementation is based on
- [LZ4 block format](https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md)
- [LZ4 frame format](https://github.com/lz4/lz4/blob/dev/doc/lz4_Frame_format.md)
- [XXH32 specification](https://github.com/Cyan4973/xxHash/blob/dev/doc/xxhash_spec.md)
