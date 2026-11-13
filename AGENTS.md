# AGENTS.md

## What this repository implements

lz4opt implements an LZ4 frame encoder with greedy and optimal parsing, a greedy LZ77 encoder with a container format, and tools around them.
The matches come from the sliding suffix tree of the `snakree` crate (a separate repository), which the encoders use only through its public API.
The crate is Rust, at the repository root.

- `lz4` (src/lz4): LZ4 frames with greedy and optimal parsing.
- `lz4::framejson` (src/lz4/framejson): LZ4 frames to JSON and back.
- `lz77` (src/lz77): greedy LZ77 phrases and a container format.
- Command-line programs live in `src/bin/`: `lz4opt`, `lz77`, `lz4frame-json`, and the experiment tools `bench`, `matchbench`, `framestats`, `lz77stats`.
  Their shared flag parsing lives in `src/bin/common/`, which `lz4opt`, `lz77`, `bench`, and `matchbench` include with `#[path]`.

Changes to the tree itself (`Snakree`) belong in that repository, not here.

## Language

This is a public repository.
Code, comments, commit messages, pull request titles and descriptions, issues, review comments, and repository documentation stay in English unless asked otherwise.
This includes the report text and figure labels that scripts generate into ignored directories such as `experiments/lz4/results/` and `experiments/lz4/runs/`.
When conversing with a user, reply in the language the user writes in.

## Markdown style

When writing prose in `.md` files, put each sentence on its own line.
Do not wrap a sentence across lines, and do not join two sentences on one line.

## Doc comment style

Doc comments start with the description itself, not a restatement of the name they document: write "Returns the parent of v" rather than "Parent returns the parent of v".
The name is already given by the comment's position directly above the field or method, so restating it is redundant.

## Building and testing

Run Cargo commands from the repository root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --release
```

CI runs the same three commands.
Some tests compare against the official `lz4` command-line tool (`tests/` and `src/lz4/tests.rs`); they skip with a message when it is not installed, so install it to run them.
The tests under `tests/` also read `testdata/sample.txt`.
`scripts/bench-compare.py` compares the speed of two revisions; its tests run with `python3 -m unittest discover -s scripts`.

## Other references

- README.md and docs/ describe the encoders, the LZ77 container format, and the frame JSON schema.
  `docs/benchmarking.md` has the benchmarking instructions.
- `CITATION.cff` gives the citation; the README's Citation section only points to it.
  Keep its version in line with `Cargo.toml` when the version changes.
- `snakree` depends on the algorithm of Leonard, Inenaga, Bannai and Mieno, *Sliding Suffix Trees Revisited* <https://arxiv.org/html/2307.01412v4>.
