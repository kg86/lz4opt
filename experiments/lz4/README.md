# LZ4 comparison experiments

Compare the official `lz4` CLI with `lz4opt` (greedy and optimal parsers; the Rust command in `src/bin/lz4opt.rs`) on the complete [Silesia corpus](https://sun.aei.polsl.pl/~sdeor/index.php?page=silesia).
The [official LZ4 benchmark](https://github.com/lz4/lz4/blob/v1.10.0/README.md#benchmarks) uses this corpus.
This experiment measures CLI processes on individual files; its timings are not directly comparable with the official in-memory lzbench numbers.

## Run

Requirements: Python 3.11+, a stable Rust toolchain (`cargo`), `curl`, and official LZ4 1.10.0 or newer with thread-count support.
Peak RSS measurement supports macOS and Linux.
No third-party Python packages are required.

From the repository root:

```sh
python3 experiments/lz4/download.py
python3 experiments/lz4/compare.py --repeats 3
```

The downloader fetches the corpus author's `silesia.zip` over HTTPS, verifies its pinned SHA-256, extracts only the 12 expected filenames, checks published file sizes and ZIP CRCs, and records each file's SHA-256 in `data/manifest.json`.
The archive digest was obtained from the author's archive on 2026-09-27; it is a reproducibility pin, not a separately signed upstream checksum.
Total input size is 211,938,580 bytes.
Downloads and extracted inputs stay under ignored `data/`.
Rerunning the downloader reuses and validates the archive.

### Pizza&Chili repetitive corpus

A second, highly repetitive corpus is available with `--corpus repcorpus`.
[DATA.md](DATA.md) describes every file in both corpora and how far their repetitions lie from each other, which decides whether LZ4 can use them.
It additionally requires `7z` (p7zip or 7-Zip) to extract the archives:

```sh
python3 experiments/lz4/download_repcorpus.py
python3 experiments/lz4/compare.py --corpus repcorpus --repeats 1 \
  --encoders official-default official-highest greedy optimal
```

The downloader fetches the 29 `.7z` archives (67 MB) that the Chilean mirror lists, verifies SHA-256 digests pinned on 2026-09-27 (the corpus publishes none), extracts each single expected member into `data/repcorpus/`, checks its size, and records its SHA-256 in `data/repcorpus-manifest.json`.
The extracted corpus is 5,705,674,774 bytes.
Rerunning reuses valid archives and extracted files, and fails if an extracted file's digest has changed.
`--files NAME...` fetches a subset.

`corpus_stats.py` (requires `zstd`) prints the per-file characteristics table in DATA.md.

The 2026-09-27 run over all 29 files is summarized, with per-file results, in DATA.md.
Its full report is kept locally in the ignored `results/repcorpus-2026-09-27/` directory:

```sh
python3 experiments/lz4/compare.py --corpus repcorpus --repeats 1 \
  --encoders official-default official-highest greedy optimal \
  --output experiments/lz4/runs/repcorpus-20260927 \
  --publish experiments/lz4/results/repcorpus-2026-09-27
```

It was interrupted after 6 files and completed with `--resume` (see below).

The runner validates input hashes, builds the Rust `lz4opt` and `framestats` commands with `cargo build --release` into ignored `bin/`, and creates a fresh ignored `runs/<timestamp>/` directory.
Run directories cannot be overwritten.
For a small smoke run, use `--files xml --repeats 1`.
Use `--data-dir`, `--output`, and `--seed` to override paths or execution order.
All files are used at their full length; files are not concatenated.

To also write a compact summary directory under `results/`:

```sh
python3 experiments/lz4/compare.py --repeats 3 --levels standard \
  --output experiments/lz4/runs/silesia \
  --publish experiments/lz4/results/silesia
```

Choose new output paths for subsequent runs.
Published reports contain Markdown, raw timing/RSS samples in CSV, and aggregated statistics and metadata in JSON.
They contain no corpus bytes or compressed data.
Full histograms and per-block details remain in the run directory.
Like `data/`, `bin/`, and `runs/`, `results/` is ignored by Git; reports are local artifacts.
Temporary compressed frames are removed after validation and analysis of each input file.

## Compression settings

According to the [official CLI manual](https://github.com/lz4/lz4/blob/v1.10.0/programs/lz4.1.md#operation-modifiers), ordinary levels range from 1 to 12 and the default is 1.
Consequently, literal `lowest / default / highest` is **not three distinct compression settings**:

| Label (`--levels standard`) | Official options |
| --- | --- |
| `official-lowest` | `-1` |
| `official-default` | no level option |
| `official-highest` | `-12` |
| `greedy` | `lz4opt -block-size 4194304 -parser greedy` |
| `optimal` | `lz4opt -block-size 4194304 -parser optimal` |

Both LZ4 environment overrides (`LZ4_CLEVEL`, `LZ4_NBWORKERS`) are removed.
Lowest and default should produce identical frames; their timing differences must not be interpreted as an algorithmic difference.

Optional alternatives are `--levels fast` (`--fast=8`, default, `-12`) and `--levels hc` (`-3`, `-9`, `-12`).
The first explicitly labels its accelerated mode `fast8`, not "lowest": acceleration 8 is a selected operating point, not a global minimum.
The HC set uses HC's default of 9, which is different from the CLI's default of 1.
`--levels span` (default, `-9`, `-10`, `-12`) covers the fast compressor and both kinds of HC parser in LZ4 1.10.0 (see `LZ4HC_CLEVEL_DEFAULT` and `LZ4HC_CLEVEL_OPT_MIN` in `lz4hc.h`): `-9` is HC's default, and `-10` (labeled `opt-lowest`) and `-12` use HC's optimal parser.
Every set also includes the greedy and optimal encoders.

### LZ77 and LZ76 phrases

`lz77stats` (`src/bin/lz77stats.rs`) parses each file with the `lz77` module twice: with a window of 65,535 bytes (LZ4's largest offset, "lz77") and with an unlimited window ("lz76").
Unlike the LZ4 encoders, it parses each file as a whole, not in 4 MiB blocks.
For each parse it prints a JSON line with the phrase count, the number of phrases with a copy, the longest copy, and the size of the `lz77` format with fixed distances and varint lengths, without and with Huffman coding.
Like `lz77 encode`, the window and maximum length are capped at `size - 1`.
Each encoding is decoded and checked against the input.

```sh
cargo build --release --bin lz77stats
target/release/lz77stats experiments/lz4/data/silesia/* > lz77stats.jsonl
```

Huffman-coded sizes come from `miniz_oxide`'s Huffman-only deflate.

### Tree throughput across implementations

`matchbench` (`src/bin/matchbench.rs`) times only the sliding suffix tree work behind the LZ4 encoders, so that other implementations of the tree can run the same workload.
For every position of each file it computes the longest match whose source starts at most `-window` bytes earlier (65,535 by default), as the LZ4 encoder's longest-match search does, but over the whole file rather than 4 MiB blocks.
It times the matching only, not reading the file, and prints a JSON line per file and repeat with the time per input byte and a checksum of all matches: FNV-1a 64 over each position's match length and then its offset, each as 8 little-endian bytes.
A faithful implementation computes the same matches, breaking ties between equally long ones the same way, and prints the same checksum.

```sh
cargo build --release --bin matchbench
target/release/matchbench -repeats 3 experiments/lz4/data/silesia/* > matchbench.jsonl
```

### A wider window for `-12`'s optimal parser

LZ4 1.10.0's optimal parser (`LZ4HC_compress_optimal` in `lib/lz4hc.c`) optimizes at most `LZ4_OPT_NUM` = 4096 positions at a time, and at level 12 takes any match longer than `LZ4_OPT_NUM - 1` immediately.
On 2026-10-17 a copy of v1.10.0 with `#define LZ4_OPT_NUM (1<<16)` was built and run with `-12` and the official command line options above.
An unmodified build of the same source reproduced the Homebrew `-12` frames byte for byte.
Reports and figures label this build LZ4HC `-12w` (encoder `official-highest-wide`).
The wider window closed 93–100% of the gap between `-12` and `optimal`:

| File | `-12` | `-12`, `LZ4_OPT_NUM` 65536 | `optimal` |
| --- | ---: | ---: | ---: |
| `Horspool` | 3,108,830 | 2,970,715 | 2,960,473 |
| `Quicksort` | 2,294,759 | 2,208,307 | 2,202,070 |
| `NQueens` | 1,694,978 | 1,639,380 | 1,635,027 |
| `fib41` | 1,071,933 | 1,067,806 | 1,067,702 |
| `rs.13` | 868,192 | 866,000 | 865,880 |
| `tm29` | 1,098,511 | 1,092,815 | 1,092,815 |
| `einstein.de.txt` | 9,437,846 | 9,437,771 | 9,437,759 |

In the 2026-11-08 full run, compression of these files took about 2–46 times as long (`Horspool` 8.3 s → 18.6 s, `rs.13` 0.25 s → 8.7 s, `tm29` 0.5 s → 24.1 s; single runs).
On macOS with Command Line Tools for SDK 27.0, linking failed on `libSystem.tbd`; building with `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk make -C programs lz4` worked.

### Measuring a subset and merging earlier results

`--encoders LABEL...` measures only the given encoders.
`--merge PATH` adds the other encoders' records from an earlier run's `results.json` to the report; the earlier run must use the same corpus and level set.
Merged records keep their original samples and frame statistics, and the report and `metadata.json` (`merged_from`) name the run they came from.
Frame sizes and statistics are deterministic and directly comparable; times and RSS come from different sessions and are only indicative.

`--resume PATH` continues an interrupted run into a new output directory.
It requires the same corpus, levels, encoder commands, repeats, and seed; files that the earlier `results.json` completed for every encoder are reused, and the rest are measured.
A rebuild from unchanged sources can still change a binary's SHA-256 (embedded paths, toolchain).
In that case the runner re-encodes the smallest reused file with each rebuilt encoder and stops unless the output matches the earlier run.
The report and `metadata.json` (`resumed_from`) record the reused files and this check.

All official commands use `-z -q -c -BI -B7 -T1 --frame-crc --no-content-size`.
Both implementations use independent 4 MiB blocks, a content checksum, no block checksum, no content-size field, and no dictionary.
Inputs are supplied on stdin and compressed bytes go to a temporary file.
The Rust encoders are single-threaded.

## Measurements

For each file, every encoder gets one untimed warmup, followed by three measured invocations by default.
Encoder order is shuffled each trial with a recorded seed.
Runs are sequential.
The input is read immediately before each invocation to warm its file cache.

* Compression wall time uses `perf_counter` around process creation and wait.
  It includes startup, buffered file I/O, checksumming and runtime overhead; it excludes builds, frame analysis, and decoding.
  Output is not fsynced.
* Peak resident memory uses `wait4` for the particular encoder process, not a cumulative maximum over all children.
  macOS returns bytes; Linux returns KiB, converted to bytes.
  RSS includes runtime and I/O buffers.
  It does not represent cumulative allocations or memory retained by the Python runner.
* Speed is input MiB / elapsed seconds.
  The aggregate divides total input by the sum of per-file median times, rather than averaging per-file speeds.
* Compression ratio is input bytes / full frame bytes; higher is better.
  Output percentage is frame bytes / input bytes × 100; lower is better.
* Aggregate memory is the maximum measured peak across files and trials.
  The report also provides per-file min/median/max time and every raw sample.

No CPU pinning, frequency locking or OS cache eviction is performed.
Short official-LZ4 runs are sensitive to process startup and system activity.
The script records versions, platform/CPU information when available, repository revision and dirty status, commands, binary hashes, relevant environment settings, and input hashes.
Avoid other CPU-heavy work while measuring.

Every measured repetition must produce the same compressed SHA-256 for its file/encoder.
The retained output is decoded using official LZ4 and its SHA-256 compared with the original file.
Parsed literal + match + raw-block bytes must also equal the full input size, and frame settings are checked.
Decompression is a correctness check and is not included in the speed table.

## Frame statistics

`framestats` (`src/bin/framestats/`) streams each frame one block at a time, reading tokens, length extensions, and offsets and skipping literal bytes, so it holds at most one block in memory.
It checks the frame layout, offsets, and decoded block sizes, but not the header or content checksums; the official decoder checks the content (see Measurements).
An earlier version aggregated the `lz4::framejson::parse` model used by `lz4frame-json inspect`, which renders every literal as text and took most of the runner's time on large inputs; its tests compare the two on generated frames.
Analysis runs after all timed compressions for a file; its time and memory are excluded from encoder measurements.

* Counts: frames, data blocks (compressed/raw), sequences, matches, nonempty literal runs, literal bytes, matched bytes, and raw-block bytes.
* Exact histograms: literal length per sequence (including zero), match length and backward offset per match; sequences, matches, literal bytes, decoded bytes and longest emitted match per block.
* Per-block records: frame/block indexes, compressed flag, payload/decoded bytes, sequences, matches, literal bytes, and longest emitted match.
* Distribution summaries: count, mean, nearest-rank p50/p90/p99, and maximum.
  Whole-corpus quantiles pool observations, not per-file quantiles.

Sequences include each compressed block's final literal-only sequence; that sequence contributes no match.
Raw blocks have no sequences, matches or LZ4 literal runs; their bytes are recorded separately.
Per-block histograms include raw blocks with zero sequence/match/literal counts and zero longest match.

"Match length" means the length actually emitted in the frame.
Frame JSON cannot establish whether it was the longest legal match at its source input position.
That would require a separate longest-match oracle.
The greedy and official parsers may visit different match start positions, so their maximum match lengths do not by themselves compare match-search correctness.

Generated files:

| File | Contents |
| --- | --- |
| `report.md` | Aggregate/per-file performance and distribution tables |
| `results.json` | Metadata, all samples, exact histograms, per-block records |
| `measurements.csv` | One row per measured encoder invocation |
| `histograms.csv` | Exact per-file histogram frequencies |
| `metadata.json` | Run configuration and provenance |
| `progress.jsonl` | Completed measurements, retained if a run fails |

The runner writes partial reports after each completed input file.
An aborted run is incomplete; check its record count before using it as a corpus result.

## Validation

```sh
cargo test --release --bin framestats
python3 experiments/lz4/compare.py --files xml --repeats 1
```

The Rust tests cover known literal/match counts, offsets and length histograms, uncompressed fallback, empty input, invalid and truncated frames, and agreement with the earlier `framejson`-based summary on generated frames.

## Rerunning the full comparison

The full comparison measures seven encoders on both corpora, and the LZ77/LZ76 phrase statistics on Silesia, and draws two figures.
`run_full.sh` remeasures all of them after the encoders change, measuring every encoder of a corpus in one session, and writes the summary tables and figures.

### Before running

1. Download both corpora (see Run): `download.py` and `download_repcorpus.py`.
2. Install official LZ4 1.10.0 (`brew install lz4`), Rust (`cargo`), Python 3.11+, and `git` and `make` for the patched LZ4.
   A different official version changes the official rows.
3. The patched LZ4 is built on first use by `build_lz4_wide.sh` into `bin/lz4-optnum65536` (see "A wider window for `-12`'s optimal parser").
   If linking fails, rerun with `SDKROOT` set as described there.
   Delete the binary to rebuild it.
4. Commit the encoder changes first: `metadata.json` records the repository revision and dirty state.
5. Close other CPU-heavy programs.
   LZ76 on `mozilla` needs about 13 GiB of memory.

### Running

```sh
experiments/lz4/run_full.sh            # or: run_full.sh OUTPUT_DIR
```

It runs, in order:

| Step | Command | Time on the Apple M4 machine |
| --- | --- | ---: |
| Silesia, 7 encoders, 3 repeats | `compare.py --repeats 3 --levels span --wide-lz4 …` | about 15 min |
| Repetitive corpus, 7 encoders, 1 repeat | `compare.py --corpus repcorpus --repeats 1 --levels span --wide-lz4 …` | about 2 h 10 min |
| LZ77/LZ76 on Silesia | `lz77stats` | about 12 min |
| Figures and tables | `plot_tradeoff.py`, `summary_tables.py` | seconds |

Times are dominated by `greedy`, `optimal`, and the two `-12` variants, each run twice per file (warmup and measurement); faster encoders shorten them.

### Outputs

`OUTPUT_DIR` (default `runs/full-YYYYMMDD/`) contains:

| File | Content |
| --- | --- |
| `summary-tables.md`, "Silesia corpus overall" | Compressed size, ratio, ratio difference from LZ4opt, speed, and peak RSS of each encoder on Silesia |
| `summary-tables.md`, "Silesia: numbers in the text" | Bytes between `-12` (and `-12w`) and LZ4opt, files that match exactly, sequence counts |
| `summary-tables.md`, "Repetitive corpus overall" and its numbers | The same table and numbers for the repetitive corpus: files where `-10` loses to `-9`, files with a ratio difference under 0.001, files where `-12` is slower than LZ4opt |
| `summary-tables.md`, "Top 5 files by compression ratio gap between -12 and LZ4opt" | Per-file ratios of `-12`, `-12w`, and LZ4opt for the five repetitive files where `-12` falls furthest behind (at least 0.1) |
| `summary-tables.md`, "Match counts for the top 5 files by compression ratio gap between -12 and LZ4opt" | Match counts for the same files and encoders as the per-file ratio table |
| `summary-tables.md`, "Comparison with LZ76 and LZ77" | Phrase counts and sizes of the LZ76 and LZ77 parses next to LZ4 |
| `silesia-tradeoff.svg`, `repcorpus-tradeoff.svg` | Speed against ratio for each file |
| `silesia/metadata.json`, `repcorpus/metadata.json` | Machine, Rust, and LZ4 versions |

Compare the new `summary-tables.md` with the previous one (`diff`) to see what changed.
Frame sizes are deterministic; if an encoder did not change, its sizes must be identical to the previous run.

The 2026-10-17 numbers came from several earlier runs combined (`runs/silesia-20261017-wide`, and for the repetitive corpus `runs/repcorpus-20260927-resumed`, `runs/repcorpus-20261017-hc`, and `runs/repcorpus-20261017-wide`); `summary_tables.py` accepts several `results.json` files per corpus to reproduce them.
A `run_full.sh` run measures each corpus in one session instead.

## The Silesia experiment

The experiment compresses each of the 12 Silesia files in independent 4 MiB blocks with the official LZ4 at its lowest, default and highest levels and with this repository's greedy and optimal parsers, using the standard level set and one thread.
It records the frame size, the time and peak memory of each encoder, and the frame statistics of its output.
Run it on one commit, with the official LZ4 1.10.0, from the repository root.

### Running

1. Install the official CLI (for example `brew install lz4`), Rust and Python 3.11+, and confirm the versions:

   ```sh
   rustc --version
   lz4 --version
   python3 --version
   ```

2. Download and verify the corpus (about 68 MB compressed, 212 MB extracted):

   ```sh
   python3 experiments/lz4/download.py
   ```

3. Optionally, run the smoke test (a few seconds):

   ```sh
   cargo test --release --bin framestats
   python3 experiments/lz4/compare.py --files xml --repeats 1
   ```

4. Run the experiment.
   With one repeat it takes about 10 minutes on an Apple M4, dominated by the greedy and optimal encoders; three repeats take about three times as long.
   Close other CPU-heavy programs first.
   Output and publish directories must not exist yet:

   ```sh
   python3 experiments/lz4/compare.py --repeats 3 --levels standard \
     --output experiments/lz4/runs/silesia \
     --publish experiments/lz4/results/silesia
   ```

   The report is `experiments/lz4/results/silesia/report.md`, and the run's `summary.json` records the machine, the Rust and LZ4 versions, the corpus archive SHA-256, and the repository revision and dirty state.
   Commit your changes first so that the revision is meaningful.

### Expected results

The encoders are deterministic, so for the official LZ4 1.10.0 the frame bytes, ratios and frame statistics must match these totals exactly:

| Encoder | Frame bytes | Sequences | Literal bytes |
| --- | ---: | ---: | ---: |
| `official-lowest` | 100,934,427 | 12,931,064 | 59,739,237 |
| `official-default` | 100,934,427 | 12,931,064 | 59,739,237 |
| `official-highest` | 77,372,720 | 14,821,648 | 31,016,852 |
| `greedy` | 81,186,670 | 16,543,820 | 29,288,795 |
| `optimal` | 77,372,438 | 14,799,845 | 31,087,251 |

`optimal` is no larger than `official-highest` on each of the 12 files, as expected for an exact per-block optimum with the same block boundaries.
Time and peak memory depend on the machine and system load; expect only the relative ordering and rough magnitudes to reproduce.
In the 2026-11-08 full run on an Apple M4 (median of three repeats), the speeds were about 630 MiB/s for the official default level, 15.8 MiB/s for `-12`, 5.2 MiB/s for `greedy` and 4.4 MiB/s for `optimal`.
A different LZ4 version may legitimately change the official frame sizes.

## README summary figure

The root README uses paired bars to compare the compression ratios of lz4opt and official LZ4HC `-12` for both complete corpora and six selected repetitive files.
Regenerate its tracked SVG from a full run with:

```sh
python3 experiments/lz4/plot_readme.py experiments/lz4/runs/chapter-rust-20261108-rerun --output docs/images/results.svg
```

The figure reads input and compressed sizes directly from that run's `results.json` files.
Compression ratios are `input_bytes / compressed_bytes` (higher is better), following the convention used in the official LZ4 benchmark table, with five decimal places printed beside each bar.
Corpus ratios use total input bytes divided by total compressed bytes, rather than an average of per-file ratios.
Both axes start at zero, with a larger range for the selected files.
