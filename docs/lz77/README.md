# Online greedy LZ77

The `lz77` module (`src/lz77/`) builds greedy LZ77 phrases online from the longest repeated suffix reported by a sliding suffix tree (`Snakree`), and serializes them in a simple byte-oriented container format.
It uses the tree only through its public API.

## Parsing

`Parser::new(Options { window: w, max_length: m })` sets the maximum backward distance at each phrase start and the maximum copy length.
A zero field selects `isize::MAX` (2^63-1), which in practice means no limit; a larger value is lowered to `isize::MAX`, the largest an encoded header may hold.
At phrase start `s`, a copy source may start anywhere in `[max(0, s-window), s)`, and the copy may overlap the phrase itself: its length is limited by `max_length` only, not by the distance or the window.
Each phrase is the longest such copy followed by the next byte.
When a copy reaches `max_length`, the phrase ends there and the next byte becomes its literal even if it would extend the copy.
Among equally long copies, the chosen distance is unspecified.
Every phrase ends with a literal: if the copy of the last phrase reaches the end of the input, it is shortened by one byte and that byte becomes the literal.

`push` takes a byte slice, consumes as much of it as completes the current phrase (or all of it, if the phrase is still open), and emits a copy followed by the first unmatched literal when the phrase completes; `finish` emits the pending final phrase, if any.
Call `finish` once the entire input has arrived, not at intermediate buffer boundaries.

```rust
use lz4opt::lz77::{Options, Parser};

let mut p = Parser::new(Options { window: 4096, max_length: 0 });
let mut tokens = Vec::new();
let mut data = &b"abcabcX"[..];
while !data.is_empty() {
    let (token, consumed) = p.push(data);
    data = &data[consumed..];
    tokens.extend(token);
}
tokens.extend(p.finish());
// Three literals a, b, c, then Token { distance: 3, length: 3, next: b'X' }.
```

Decode each token by copying `length` bytes **one at a time** from `distance` bytes behind the current output, then appending `next`.

The parser owns a `Snakree`, retains the history while extending a phrase, and contracts only when that phrase is complete.
With `Snakree`, total processing time is O(n), and allocated storage is O(min(n, window + longest phrase seen)), where the longest phrase is at most `max_length + 1`.
Without a `max_length`, a repetitive stream can defer output until `finish` and use memory proportional to its length even with a small window.
A fixed-width window that evicts a byte per appended byte would not work: it would discard candidates before a phrase is complete.

## Encoding and decoding streams

```rust
use lz4opt::lz77::{self, Coding, EncodeOptions, Entropy, Options};

let opts = EncodeOptions {
    options: Options { window: 4096, max_length: 255 },
    distance: Coding::Fixed,   // Coding::Default selects Fixed
    length: Coding::Varint,    // Coding::Default selects Varint
    entropy: Entropy::Huffman, // default Entropy::None
};
lz77::encode(dst, src, &opts)?; // dst: impl Write, src: impl Read
lz77::decode(dst, src)?;
```

`EncodeOptions` contains the parser's `Options`, with the same `isize::MAX` defaults, and selects the coding of distances and lengths and whether the records are entropy coded (see below).
Both codings default to `Coding::Default`, which selects fixed distances and varint lengths.
Neither function closes its streams.
`decode` validates the whole stream and returns `Error::Format` for malformed or truncated input; output written before the error may be incomplete.
Because the format has no end marker, well-formed records appended to a stream are decoded as part of it rather than rejected.
The decoder keeps O(min(n, window)) bytes of history (up to twice the window, plus a 64 KiB output buffer), so with the default window it holds the whole output.

From the repository root (or run the built `target/release/lz77`):

```sh
cargo run --release --bin lz77 -- encode input > output.lz77
cargo run --release --bin lz77 -- encode -window 4096 -max-length 255 input > output.lz77
cargo run --release --bin lz77 -- encode -window 4096 -distance varint -length fixed input > output.lz77
cargo run --release --bin lz77 -- encode -window 65536 -entropy huffman input > output.lz77
cargo run --release --bin lz77 -- decode output.lz77 > restored
cmp input restored
```

Both subcommands read the named file, or standard input when the argument is omitted or `-`, and write standard output.
`-window` and `-max-length` default to `isize::MAX` and must be positive; larger values are lowered to `isize::MAX`.
`-distance` (default `fixed`) and `-length` (default `varint`) take `fixed` or `varint`.
`-entropy` takes `none` (the default) or `huffman`.

When the input is a regular file (a named file or a `< file` redirection), `encode` lowers the window and max length to `size - 1` (at least 1), the largest distance and copy length that input can use.
The parse is unchanged, but fixed fields become no wider than the file needs: a 2.5 MB file gets 4-byte fixed distances instead of 8-byte ones.
A pipe has no known size, so the limits stay as given; with the default window, fixed distances then take 8 bytes.
The command exits with status 2 on a usage error and 1 on an encoding, decoding, or I/O error.

## Output format

An encoded stream is a header followed by one record per token in parse order, written directly as bytes or, with Huffman entropy coding, grouped into blocks (see [Huffman blocks](#huffman-blocks)).
There is no end marker: the stream ends at the end of the file, which must fall on a record or block boundary.

```
file    = header record*          if entropy is 00
        | header block*           if entropy is 01
header  = "LZ77"          (4 bytes: 4c 5a 37 37)
          version         (1 byte: 01, fixed while the format is a prototype)
          window          (uvarint, >= 1)
          max_length      (uvarint, >= 1)
          distance_coding (1 byte: 01 fixed, 02 varint)
          length_coding   (1 byte: 01 fixed, 02 varint)
          entropy         (1 byte: 00 none, 01 huffman)
record  = length   (length coding)    0 <= length <= max_length
          distance (distance coding)  present iff length > 0, stored as distance-1
          next     (1 byte)           the literal
```

Every record represents `length + 1` bytes.
Decoding a record copies `length` bytes one at a time from `distance` bytes behind the end of the output, so `length` may exceed `distance` (a run of `aaaa` is literal `a` then a copy of length 2 at distance 1 with literal `a`), and then appends `next`.
A literal-only record (`length` 0) has no distance field.
`distance` satisfies `1 <= distance <= min(window, bytes decoded so far)`; it is stored minus one so that, for example, window 256 fits in one fixed byte.

Because the parser gives every phrase a literal, all records have the same shape, and the decoder needs no flag to recognize the last one.
A file cut at a record (or block) boundary therefore decodes without error to a prefix of the input; a file cut inside the header, a record, or a block is rejected.
For the same reason, well-formed records (or blocks) appended to a file decode as extra output, not as an error.
The format stores no checksum or original length.

### Field codings

Header numbers (`window`, `max_length`) are always uvarints.
The record fields `length` and `distance` each use the coding chosen in the header:

- **fixed**: little-endian, in the fewest of 1, 2, 4, or 8 bytes that hold the field's largest value: `max_length` for lengths and `window - 1` for distances.
  For example, window 65536 gives 2-byte distances, and max length 255 gives 1-byte lengths; the default `isize::MAX` gives 8 bytes.
- **varint** (uvarint): a variable number of bytes, described below.

The defaults are fixed distances and varint lengths: a source may lie anywhere in the window, while most copies are short.
Which is smaller depends on the input; for text with many nearby repeats, varint distances can win.

### Uvarint

A uvarint (unsigned variable-length integer, as in Protocol Buffers) stores a nonnegative integer in as few bytes as it needs.
The number is split into 7-bit groups, least significant group first.
Each group goes in one byte; the byte's high bit is 1 if more bytes follow and 0 on the last byte.
A decoder reads bytes until it sees one below `0x80`.

| Value | Binary | Bytes |
|---|---|---|
| 0 | `0` | `00` |
| 3 | `11` | `03` |
| 127 | `1111111` | `7f` |
| 128 | `1 0000000` | `80 01` |
| 200 | `1 1001000` | `c8 01` (`0x48 \| 0x80`, then `0x01`) |
| 65536 | `100 0000000 0000000` | `80 80 04` |
| `isize::MAX` (2^63-1) | 63 one bits | `ff ff ff ff ff ff ff ff 7f` |

Values below 128 take one byte, below 16384 two bytes, and so on; at most 10 bytes.

### Huffman blocks

Blocks exist only with entropy `01`; with entropy `00`, records follow the header one by one.
A block is a run of consecutive records, at most 65536 of them, and takes the place of those records in the `file` rule above; the encoder fills every block but the last:

```
header | block 1 (records 1-65536) | block 2 (records 65537-131072) | ... | last block
```

Huffman coding needs blocks for three reasons:

- **Codes come from frequencies.**
  A Huffman code is built from how often each byte occurs, so the encoder must collect records before it can write any of them; it cannot emit each record as soon as the parser completes it.
  Each block carries its own code tables (inside its deflate streams).
- **Streams need a group of records.**
  Gathering all lengths, all distances, and all literals into separate streams requires holding several records at once.
- **Bounded memory.**
  A single block for the whole input would make both the encoder and the decoder hold every record, in memory proportional to the input.
  With a block limit, each side holds one block at a time and the stream can be processed as it flows.
  Fresh code tables per block also follow changes in the data.

In exchange, output is delayed until a block fills, and a truncated file decodes to a prefix of the input only when it is cut at a block boundary.

A block splits the fields of its records into byte streams and compresses each stream separately.
`tokens` is the number of records in the block (one per parser token); for example, `abcabcX` gives one block of 4 records covering 7 input bytes:

```
block  = tokens (uvarint, 1 <= tokens <= 65536)
         stream*    length streams, then distance streams, then literals
stream = size (uvarint)
         data (size bytes: a complete deflate stream, RFC 1951)
```

A field in varint coding takes one stream holding its uvarints in record order.
A field in fixed coding of `w` bytes takes `w` streams: stream `k` holds byte `k` (from the least significant) of each value, so low bytes, which are nearly uniform, and high bytes, which are skewed, get separate Huffman codes.
The distance streams hold only the records with `length > 0`, and the literal stream holds one byte per record.
For example, two-byte fixed distances and varint lengths give four streams: lengths, distance low bytes, distance high bytes, literals.

The encoder compresses each stream as raw deflate in Huffman-only mode (the `miniz_oxide` crate), which uses Huffman codes but no LZ77 matching of its own; any deflate data decodes.
The Huffman codes themselves depend on the deflate implementation, so the encoded bytes (not only their meaning) can differ between implementations.
Each stream must decompress to exactly the bytes its block's records use and end at `size`; extra bytes are rejected.
The record count bounds the size of each decompressed stream (one byte per record for a fixed-coding byte or the literals, at most 10 for uvarints), so a small stream cannot expand without limit.

On `silesia` files with a 65,535-byte window and default field codings, Huffman blocks shrink the output from 0.546 to 0.419 of the input for `dickens`, from 0.167 to 0.138 for `xml`, and from 0.542 to 0.409 for `mozilla`.
The default field codings (fixed distances, varint lengths) remain the best choice under Huffman coding on these inputs.

### Example

`abcabcX` with window 3 and max length 3, default codings, encodes to 19 bytes:

```
4c 5a 37 37 01    "LZ77", version 1
03 03             window 3, max length 3
01 02             fixed distances (1 byte, since window-1 = 2 < 256), varint lengths
00                no entropy coding
00 61             length 0, literal 'a'
00 62             'b'
00 63             'c'
03 02 58          length 3, distance 3 (stored 2), literal 'X'
```

With max length 2, the header's max length becomes `02`, and the copy stops after `ab` so that `c` becomes its literal: `... 02 02 63` (length 2, distance 3, `c`) followed by `00 58` (literal `X`).

Without entropy coding, the format is a transparent record of the parse, not a competitive compressed format: literals cost at least two bytes.

## Relation to the original LZ77 paper

The parse follows the scheme of Ziv and Lempel, *A Universal Algorithm for Sequential Data Compression*, IEEE Transactions on Information Theory 23(3), 1977: each step takes the longest prefix of the remaining input that has an earlier occurrence starting in the sliding history, possibly overlapping the prefix itself, and emits a (distance, length, next symbol) triple, so every phrase ends with an explicit literal.
It differs from the paper in these respects:

- **Lookahead bound is optional.**
  The paper's buffer has a lookahead part of length `Ls`, so a match is at most `Ls - 1` bytes, and a phrase that reaches that length ends with the next symbol whether or not it matches.
  `max_length = Ls - 1` reproduces this, with `window = n - Ls` for the history part of the paper's buffer of length `n`; the default leaves the match length unbounded, limiting only where the source starts, by `window`.
- **Empty initial history.**
  The paper starts with the history filled with a fixed symbol (zeros), so the first phrase may already copy.
  Here the history starts empty, and the first phrases are literals with `length` 0.
- **End of input.**
  The paper assumes that a next symbol always exists.
  Here a final match that reaches the end of the input is shortened by one byte so that the last byte becomes its literal; every record is still a triple.
- **Codeword encoding.**
  The paper writes every phrase as a codeword of the same width: a pointer of `⌈log(n - Ls)⌉` symbols, a length of `⌈log Ls⌉` symbols, and one literal symbol.
  Fixed codings come closest, but round each field up to whole bytes (1, 2, 4, or 8), omit the pointer of a literal-only record, and follow a header; varint codings and Huffman blocks differ further.
  The format is not bit-compatible with the paper's code.
- **Pointer representation.**
  The paper addresses a source by its position in the buffer; this format stores the backward distance, which is equivalent for a fixed history window.

## LZ76 compression

Without `-window` and `-max-length` (zero in `lz77::Options`), the window is unbounded and copies have no length limit, so the greedy parse is the LZ76 parse.
Each phrase is the longest copy from anywhere earlier in the input, which may overlap the phrase itself, followed by one literal byte.
`lz77 encode` is therefore an LZ76 compressor, and `lz77 decode` restores the input:

```sh
cargo run --release --bin lz77 -- encode -distance varint -entropy huffman input > output.lz76
cargo run --release --bin lz77 -- decode output.lz76 > restored
cmp input restored
```

The number of phrases is the LZ76 complexity of the input.
`lz77stats` prints it, and the encoded sizes, for each file next to the same figures for LZ4's 65,535-byte window:

```sh
cargo run --release --bin lz77stats -- input
```

The unbounded window keeps the whole input in the tree and the decoder keeps the whole output, so memory is O(n).

## Testing

Tests compare the parser with a direct search over every source that does not use the tree, over short exhaustive binary inputs, random inputs, phrases much longer than the window, and all byte values.
Format tests check the exact encoding for each coding, fixed field widths, the stream layout of Huffman blocks, round trips with chunked readers, and rejection of malformed streams.
Randomized and mutated inputs use fixed seeds, so the tests are deterministic.

```sh
cargo test --release lz77
```
