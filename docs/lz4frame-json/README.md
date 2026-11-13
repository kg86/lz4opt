# lz4frame-json

A Rust command-line tool that converts between a binary LZ4 frame stream and JSON that represents its physical structure as is.
`inspect` decomposes the binary into JSON, and `restore` turns the JSON back into the byte-identical binary.

It does not decompress or recompress data, but it does decompose each compressed block into the sequences of the LZ4 block format (token, literals, match offset, match length, and so on).
Its purpose is to visualize the structure of LZ4 frames and blocks, not to compress or decompress.

## Build

It needs the Rust toolchain (`cargo`).
JSON input and output use `serde` and `serde_json`.
Build from the repository root.

```sh
cargo build --release --bin lz4frame-json
```

The executable is `target/release/lz4frame-json`.
The examples below use `lz4frame-json` to mean this executable.

## Usage

```text
lz4frame-json inspect [input.lz4|-]
lz4frame-json restore [input.json|-] [output.lz4|-]
```

- `inspect`: parses an LZ4 binary and writes pretty-printed JSON to standard output.
  The input is standard input when the argument is omitted or `-`, and the named file otherwise.
- `restore`: concatenates the physical fields of JSON produced by `inspect` in offset order and writes the LZ4 binary back.
  Standard input and output are used for the input and output when the argument is omitted or `-`.
  If the output is standard output and a character device (a terminal, or a device file such as `/dev/null`), it refuses with an error so that binary data is not written to the screen by mistake.
  Redirects to regular files and pipes are allowed.
  A named output file is written to a temporary file in the same directory and renamed over the target, with mode 0644.
  When restoring fails, an existing output file is left untouched and no temporary file remains.
- Diagnostic messages always go to standard error, so standard output carries only the JSON of `inspect` or the binary of `restore`.

### Examples with files

```sh
lz4frame-json inspect sample.lz4 > sample.json
lz4frame-json restore sample.json restored.lz4
cmp sample.lz4 restored.lz4   # identical byte for byte
```

### Examples with stdin and stdout

```sh
cat sample.lz4 | lz4frame-json inspect > sample.json
cat sample.json | lz4frame-json restore > restored.lz4
```

`restore` can also use a file for only one of the input and the output.
The omitted side is treated like `-` (standard input or standard output).

```sh
lz4frame-json restore sample.json > restored.lz4
cat sample.json | lz4frame-json restore - restored.lz4
```

### Exit codes

- `0`: success
- `1`: an I/O error, an invalid LZ4 frame, or JSON that cannot be restored
- `2`: a usage error (for example, a missing subcommand or an extra positional argument)

## JSON output conventions

`inspect` prints JSON like the following.
It is the actual output of `lz4frame-json inspect` for an input made of an LZ4 frame followed by a skippable frame (frame size 3, data `aabbcc`).
The LZ4 frame holds the 17 bytes `abcdabcdabcdefghi`, compressed with `lz4 -1 --content-size -BX`.
All values, offsets, and hex strings are the real output.
Some objects are put on one line for space; the real output has a line break after every key.

```json
{
  "frames": [
    {
      "type": "lz4_frame",
      "magic_number": { "offset": 0, "value": 407708164, "hex": "04224d18" },
      "frame_descriptor": {
        "flg": {
          "offset": 4, "value": 124, "hex": "7c",
          "version": 1,
          "block_independence": true,
          "block_checksum": true,
          "content_size_present": true,
          "content_checksum": true,
          "reserved_bit": 0,
          "dictionary_id_present": false
        },
        "bd": {
          "offset": 5, "value": 64, "hex": "40",
          "reserved_bit_7": 0,
          "block_max_size_code": 4,
          "block_max_size_bytes": 65536,
          "reserved_bits_3_0": 0
        },
        "content_size": { "offset": 6, "value": 17, "hex": "1100000000000000" },
        "dictionary_id": null,
        "header_checksum": { "offset": 14, "value": 197, "hex": "c5" }
      },
      "data_blocks": [
        {
          "block_size": {
            "offset": 15, "value": 13, "hex": "0d000000",
            "data_size": 13, "uncompressed": false
          },
          "data": [
            {
              "literal_length": 4,
              "match_length": 8,
              "token": {
                "type": "token", "offset": 19, "size": 1, "hex": "44",
                "literal_length_code": 4, "match_length_code": 4
              },
              "literal_length_extension": null,
              "literals": {
                "type": "literals", "offset": 20, "size": 4, "hex": "61626364",
                "display": "abcd"
              },
              "match_offset": { "offset": 24, "value": 4, "hex": "0400" },
              "match_length_extension": null
            },
            {
              "literal_length": 5,
              "match_length": null,
              "token": {
                "type": "token", "offset": 26, "size": 1, "hex": "50",
                "literal_length_code": 5, "match_length_code": 0
              },
              "literal_length_extension": null,
              "literals": {
                "type": "literals", "offset": 27, "size": 5, "hex": "6566676869",
                "display": "efghi"
              },
              "match_offset": null,
              "match_length_extension": null
            }
          ],
          "block_checksum": { "offset": 32, "value": 3383175629, "hex": "cd29a7c9" }
        }
      ],
      "end_mark": { "offset": 36, "value": 0, "hex": "00000000" },
      "content_checksum": { "offset": 40, "value": 676887636, "hex": "547c5828" }
    },
    {
      "type": "skippable_frame",
      "magic_number": { "offset": 44, "value": 407710288, "hex": "502a4d18" },
      "frame_size": { "offset": 48, "value": 3, "hex": "03000000" },
      "data": { "offset": 52, "size": 3, "hex": "aabbcc" }
    }
  ]
}
```

This data block is decomposed into two sequences.
The first has the 4 literals `abcd` followed by a match with distance 4 and length 8.
The last one is literal-only and has the 5 literals `efghi`.

### `offset`, `hex`, and `value`

- A numeric field that exists in the binary is a physical field that has `offset`, `value`, and `hex` (`{"offset": ..., "value": ..., "hex": ...}`).
- `offset` is the byte position, with the start of the input as 0.
- `value` is the decimal unsigned integer read with the endianness of the specification (little-endian in the LZ4 frame format).
  It is not the number obtained by reading `hex` as a hexadecimal string.
- `hex` is the byte sequence as it appears in the input, in order, as a lowercase string of an even number of digits without `0x`.
  For example, the magic number `0x184D2204` is stored in little-endian order as `04 22 4d 18`, so its `hex` is `"04224d18"`.
- A field that is split into bit fields, such as FLG, BD, and the block size, holds the parts (`version`, `block_max_size_code`, and so on) in the same object as the `value` and `hex` of the whole.
  The parts are derived attributes without a byte range, so they have no `offset` or `hex`.
- A physical field of a segment type, such as `token`, `literals`, and `uncompressed_data`, has `type`, `offset`, `size`, and `hex` instead of `value`.
  `size` is redundant because it follows from the two hex digits per byte, but it is always written to state the byte count, and `size == len(hex) / 2` holds.
  `match_offset` is always 2 bytes, so it is the exception: it has no `size` and uses the same `offset`/`value`/`hex` form as the other numeric fields.

### `data` of a data block (LZ4 block format)

The shape of `data_blocks[].data` depends on `block_size.uncompressed`.

- `uncompressed: true` (an uncompressed block): `data` is an array with a single physical field of `type: "uncompressed_data"`.
  `hex` holds the original bytes, and `display` is a string for display only, made losslessly with the fixed escape rules below.
- `uncompressed: false` (a compressed block): `data` is an array of one or more sequences of the LZ4 block format.
  A sequence is a semantic group without `offset` or `hex`, and the index in the `data` array is the sequence number (there is no sequence number field).
  Each sequence has the following members.

  | member | content |
  |---|---|
  | `literal_length` | The effective literal length computed from the high nibble of the token and the optional extension. |
  | `match_length` | The effective match length when there is a match (`4 + low nibble + extension`). `null` for the last, literal-only sequence. |
  | `token` | The 1-byte token. It has the two nibbles `literal_length_code` and `match_length_code`, and does not output the undivided `value`. |
  | `literal_length_extension` | A variable-length extension that exists only when the high nibble of the token is 15. `additional_length` is the sum of all extension bytes (255 for each `0xff` byte, plus the final byte). |
  | `literals` | The literal bytes. `hex` is the only authority, and `display` is a lossless string for display only. It is not omitted even when the size is 0. |
  | `match_offset` | The 2-byte little-endian match distance. `null` for the last, literal-only sequence. It has no `size` because it is always 2 bytes. |
  | `match_length_extension` | A variable-length extension that exists only when there is a match and the low nibble of the token is 15. |

  The byte ranges of the non-empty physical fields (`token`, `literal_length_extension`, `literals`, `match_offset`, `match_length_extension`) are contiguous without gaps or overlaps, and their total size equals `block_size.data_size`.

- The user data of a skippable frame has a single object as `data` with only `offset`, `size`, and `hex`.
  Its content is not interpreted, and it has no `display`.

`display` (held by `literals` and `uncompressed_data`) is a helper string for display only.
It converts the original bytes represented by `hex` byte by byte with the following fixed rules, so it carries the same information as `hex` in a human-readable form.

- `\` (`0x5c`) and `"` (`0x22`): `\\` and `\"`.
- Other printable ASCII (`0x20` to `0x7e`): output as is.
- LF, CR, and TAB: `\n`, `\r`, and `\t`.
- Any other byte: `\xHH` with two uppercase hexadecimal digits.

For example, the uncompressed data `hello` (`68656c6c6f`) becomes `display: "hello"`.

These rules describe the string value of `display`.
When the JSON is written, `inspect` also escapes `<`, `>`, `&`, U+2028, and U+2029 in every string as `\u003c`, `\u003e`, `\u0026`, `\u2028`, and `\u2029`, so a JSON parser reads the same string.

### Optional fields that become `null`

`content_size`, `dictionary_id`, `block_checksum`, and `content_checksum` are optional fields enabled or disabled by bits of FLG.
They are `null` when absent, to keep the shape of the JSON stable; the fields are not omitted.
Likewise, `match_length`, `literal_length_extension`, `match_offset`, and `match_length_extension` of a sequence are `null`, not omitted, when the corresponding match or extension does not exist (see the sequence table above).

### Restoring in offset order

- `restore` walks everything under `frames` recursively and collects, as physical fields, only the elements that have both `offset` and `hex` in the same object.
  Derived or descriptive objects that have only `offset` are not collected.
  An object that has `hex` without `offset` is rejected.
- It sorts the collected physical fields by `offset`, checks that the first starts at 0 and that there are no gaps or overlaps, and then concatenates the `hex` strings in offset order to restore the original binary.
- Derived attributes such as `value`, bit fields, and `size` are not used to build the binary.
  Only the `offset` and `hex` of each physical field decide the result, and the order of members inside a JSON object does not matter.
- Each array itself, such as `frames`, `data_blocks`, and `data`, must contain physical fields in monotonically non-decreasing offset order (equal offsets are allowed).
  JSON that violates this is rejected before restoring.
- Any number of zero-length fields (the data of an empty uncompressed block, or the data of a skippable frame of size 0) may exist at the same offset.
  At most one field of length 1 or more may exist at an offset.

## Limitations

- The header checksum, block checksums, and content checksum are output as stored, and are **not recomputed or verified**.
- A compressed block is only **decomposed structurally** into sequences of the LZ4 block format (token, length extension, literals, match offset).
  The literals and matches are **not expanded** into the actual uncompressed data.
  The earlier data that a match refers to and the contents of a dictionary are not verified either.
  The user data of a skippable frame is still not interpreted and is kept as `hex` of the input bytes.
- If the sequence structure of a compressed block violates the LZ4 block format specification, the parse is rejected with an error that has an offset.
  Examples are match offset `0`, a length extension that does not terminate, a decompressed size over the block maximum size, and an end-of-block violation of the last sequence.
  Even an input that can be read as a frame structure may fail in `inspect` depending on the content of a block.
- **Legacy frames** (magic number `0x184C2102`) are not supported.
  They produce an error when they appear in the input.
- The parse result and the hex strings of all the data are **held in memory** before output.
  The hex form alone doubles the number of characters, and `literals` and `uncompressed_data` also hold the `display` string of the same data, so there is no streaming optimization for large files.
- Restoring from JSON other than the output of this tool (arbitrary JSON edited by hand) checks only the schema and the consistency of the physical fields.
  It does not guarantee that descriptive attributes such as `value` and `size` match the actual `hex` content.

## Conformance

It conforms to the [LZ4 Frame Format Description v1.6.4](https://github.com/lz4/lz4/blob/v1.10.0/doc/lz4_Frame_format.md) and the [LZ4 Block Format Description](https://github.com/lz4/lz4/blob/v1.10.0/doc/lz4_Block_format.md).
Both are pinned to the [LZ4 v1.10.0 tag](https://github.com/lz4/lz4/tree/v1.10.0), which contains the specifications, so that the references do not change in the future.

It handles regular LZ4 frames (magic number `0x184D2204`) and skippable frames (magic numbers `0x184D2A50` to `0x184D2A5F`), and parses a concatenated stream up to EOF.

## Round-trip verification with the official LZ4 CLI

`scripts/verify-roundtrip.sh` is an end-to-end script that inspects and restores a binary produced by the official `lz4` CLI with this tool, and checks that it matches the original binary byte for byte.

It needs `cargo`, the official `lz4` CLI, and `cmp` on the PATH.
Give the file to verify as the single argument.

```sh
scripts/verify-roundtrip.sh testdata/sample.txt
```

The script finds the repository root itself, so it works from any directory; the example path is relative to the repository root.
It compresses the file with `lz4`, checks that the binary restored by `inspect` and `restore` matches the original compressed bytes, and checks that decompressing both with `lz4 -d` gives the given file.

`cargo test --release` also includes integration tests that use the official `lz4` CLI (`tests/lz4frame_json_official.rs`).
They are skipped with a reason when `lz4` is not on the PATH, so a plain `cargo test` does not fail.

## Development

The parsing module is `src/lz4/framejson`, and the command is in `src/bin/lz4frame-json/`.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --release
```

JSON syntax error messages come from `serde_json`.
`serde_json` also normalizes the number `1e2` to `1e+2`, so error messages that reject an offset format differ in that one spelling.
