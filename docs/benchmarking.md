# Benchmarking

`src/bin/bench.rs` times the encoders on files or on synthetic inputs, and `scripts/bench-compare.py` compares two revisions with it:

```sh
cargo build --release --bin bench
target/release/bench -synthetic 4194304                      # unary, periodic, random
target/release/bench -workload lz4-optimal experiments/lz4/data/silesia/xml

scripts/bench-compare.py HEAD~1 HEAD                      # two commits
scripts/bench-compare.py HEAD WORKTREE -- -workload lz77 experiments/lz4/data/silesia/*

scripts/bench-compare.py --workload lz4-optimal abc1234 WORKTREE        # one workload, a commit against the working tree
scripts/bench-compare.py --workload lz4-optimal --preset silesia-quick HEAD~3 HEAD
```

`--workload NAME` (repeatable) limits the comparison to the named workloads; without it all default workloads run.
`--preset silesia-quick` (xml, dickens, mr, x-ray, sao) or `--preset silesia` (all 12 files) compares on the Silesia corpus, which `experiments/lz4/download.py` fetches.
Arguments after `--` go to `bench` and add to these.

The workloads are `lz77`, `lz77-huffman`, `lz4-greedy` and `lz4-optimal` (the default set), `lz77-decode` and `lz77-huffman-decode` (decoding what the matching encoder wrote), `match` (only the suffix-tree matching, with the checksum of `matchbench` as output, to see whether a change speeds up the tree or the encoder around it), and `framejson` (parsing and rendering a frame, which needs `-max-bytes` on large inputs because its JSON is about 50 times larger).
Each run prints a JSON line with the time and an FNV-1a hash of the output.
The comparison script builds each revision in its own worktree, runs the two builds alternately for several rounds, and prints the minimum and median time of each, the ratio A / B (above 1 means B is faster), the spread of each build's rounds, and whether the output hashes agree: a hash that differs means the change altered the output, not only the speed.
Output sizes must also agree; the table shows both sizes and their ratio B / A (below 1 means B compresses better).
The verdict column says "B faster" or "B slower" only when the minimums differ by more than the larger spread of the two builds (at least 2%), and "same speed" otherwise.
A geometric mean of A / B per workload summarizes the inputs.
The report also compares the peak resident set size of the `bench` process (minimum over the rounds).
It is the largest over every workload and input of one run, so combine it with `--workload` and a single input to measure one case.
Empty or mismatched measurement sets, duplicate records, and missing rounds are rejected rather than silently omitted.
Use `WORKTREE` for uncommitted changes.
`--rounds N` sets the number of alternating runs of each build (default 5), and `--json FILE` also writes the aggregated results to FILE.
Both revisions must contain `src/bin/bench.rs`.

Synthetic inputs are quick but unrepresentative (the random input is the slowest case, about 1 microsecond per byte); decide on real files such as the Silesia corpus when a change matters.

## Comparing revisions of snakree

The tree lives in the separate `snakree` repository, and `Cargo.lock` pins the revision that each commit of this repository builds.
To compare two snakree revisions with the same encoders, change the pin in the working tree and compare against `HEAD`:

```sh
cargo update -p snakree --precise <snakree-commit>
scripts/bench-compare.py --workload match --preset silesia-quick HEAD WORKTREE
git checkout Cargo.lock
```

For a local checkout of snakree, add `[patch."https://github.com/kg86/snakree.git"] snakree = { path = "../snakree" }` to `Cargo.toml` in the working tree instead, and revert it afterwards.
The `match` workload shows the effect on the tree alone; `lz4-optimal` shows the effect on the whole encoder.
