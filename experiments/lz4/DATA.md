# Experiment corpora

The LZ4 experiments use two corpora.
Both are downloaded into the ignored `data/` directory; see [README.md](README.md) for the download commands.

| Corpus | Option | Files | Bytes | Downloader |
| --- | --- | ---: | ---: | --- |
| [Silesia](https://sun.aei.polsl.pl/~sdeor/index.php?page=silesia) | `--corpus silesia` (default) | 12 | 211,938,580 | `download.py` |
| [Pizza&Chili repetitive corpus](https://pizzachili.dcc.uchile.cl/repcorpus.html) | `--corpus repcorpus` | 29 | 5,705,674,774 | `download_repcorpus.py` |

Silesia is the corpus of the official LZ4 benchmark and represents ordinary mixed data.
The Pizza&Chili repetitive corpus (Ferragina and Navarro, maintained at Universidad de Chile) is the standard testbed for repetition-aware compressors and compressed indexes: LZ77 and Re-Pair papers and r-index / RLBWT work routinely report results on it.

## What limits LZ4 on repetitive data

An LZ4 match offset is at most 65,535 bytes, and `lz4opt` further splits input into independent 4 MiB blocks.
A repetition helps only if an earlier copy lies within the preceding 64 KiB of the same block.
Many repetitive collections are built by concatenating versions of a document or genomes of one species; there each copy is one full version or genome away, often far beyond 64 KiB.
Such files are highly compressible for 7z or Re-Pair but behave like ordinary data for every LZ4 encoder.

To separate the two cases, `corpus_stats.py` compresses each file with zstd at level 3 twice: with a 64 KiB window (`--zstd=wlog=16`) and with long-distance matching over a 2 GiB window (`--long=31`).
The *64 KiB / long* ratio of their output sizes estimates how much redundancy is out of LZ4's reach:

* close to 1× — repetitions are local, so a better LZ4 parser can profit;
* much larger than 1× — most repetitions are distant; LZ4 sees only the local redundancy of the underlying text.

This is a heuristic based on one zstd match finder, not a bound; the measured LZ4 results in this document are authoritative.
On BGL, for example, the long-distance matcher's overhead makes the long window slightly worse.

Column key for the tables below:

* σ — number of distinct byte values; H0 — order-0 entropy in bits per byte.
* zstd 64 KiB % and zstd long % — zstd output as a percentage of input.
* 7z % — size of the corpus's own `.7z` archive (LZMA with a large dictionary), a reference for long-range compressibility.

Regenerate the characteristics (about 40 s for this corpus on an Apple M4):

```sh
python3 experiments/lz4/corpus_stats.py --corpus repcorpus
python3 experiments/lz4/corpus_stats.py --corpus silesia
```

Values below were measured with zstd 1.5.7.

## Silesia

Descriptions follow the corpus page.
All Silesia files have a 64 KiB / long ratio of 1.0–1.3×: nearly all redundancy that zstd finds is within LZ4's window.

| File | Contents | Bytes | σ | H0 | zstd 64 KiB % | zstd long % | 64 KiB / long |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `dickens` | Collected works of Charles Dickens (English text) | 10,192,446 | 100 | 4.53 | 41.02 | 35.99 | 1.1× |
| `mozilla` | Tarred Mozilla 1.0 executables (Tru64 UNIX edition) | 51,220,480 | 256 | 6.22 | 38.98 | 35.54 | 1.1× |
| `mr` | Medical magnetic resonance image | 9,970,564 | 256 | 3.68 | 36.65 | 35.59 | 1.0× |
| `nci` | Chemical database of structures | 33,553,445 | 62 | 2.43 | 9.59 | 7.21 | 1.3× |
| `ooffice` | OpenOffice.org 1.01 DLL | 6,152,192 | 256 | 6.64 | 54.49 | 50.86 | 1.1× |
| `osdb` | Sample MySQL database (Open Source Database Benchmark) | 10,085,684 | 256 | 6.59 | 41.38 | 34.71 | 1.2× |
| `reymont` | *Chłopi* by W. Reymont (Polish, PDF) | 6,627,202 | 256 | 4.84 | 32.37 | 29.37 | 1.1× |
| `samba` | Tarred Samba 2.2.3 source | 21,606,400 | 256 | 6.09 | 26.99 | 22.75 | 1.2× |
| `sao` | SAO star catalog (binary) | 7,251,944 | 256 | 7.53 | 79.70 | 76.33 | 1.0× |
| `webster` | 1913 Webster Unabridged Dictionary (HTML) | 41,458,703 | 98 | 4.97 | 32.95 | 29.20 | 1.1× |
| `xml` | Collected XML files | 5,345,280 | 104 | 5.52 | 15.00 | 11.26 | 1.3× |
| `x-ray` | X-ray medical picture | 8,474,240 | 256 | 6.60 | 76.66 | 71.90 | 1.1× |

## Pizza&Chili repetitive corpus

The corpus has four categories.
The descriptions below summarize the corpus's own statistics pages ([real](https://pizzachili.dcc.uchile.cl/real-stats.html), [pseudo-real](https://pizzachili.dcc.uchile.cl/pseudo-real-stats.html), [artificial](https://pizzachili.dcc.uchile.cl/artificial-stats.html), [logs](https://pizzachili.dcc.uchile.cl/log-stats.html)), which also give bzip2, gzip, ppmdi, and Re-Pair ratios and repetitiveness measures such as z (LZ77 phrases) and r (BWT runs).
The statistics pages list some collections (Nobel and Turing articles, most mutation variants, several bacteria) that the download directories do not contain; the downloader fetches exactly the 29 files present on the server.

### Real

Real repetitive sources.
DNA collections are concatenated in random order; the others are concatenated from the oldest to the newest version.

| File | Contents | Bytes | σ | H0 | zstd 64 KiB % | zstd long % | 7z % | 64 KiB / long |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `Escherichia_Coli` | 23 *E. coli* genomes (NCBI) | 112,689,515 | 15 | 2.00 | 31.73 | 10.01 | 4.67 | 3.2× |
| `cere` | 37 *S. cerevisiae* genomes (Saccharomyces Genome Resequencing Project) | 461,286,644 | 5 | 2.19 | 29.58 | 2.16 | 1.14 | 13.7× |
| `para` | 36 *S. paradoxus* genomes (same project) | 429,265,758 | 5 | 2.12 | 30.65 | 3.26 | 1.46 | 9.4× |
| `influenza` | 78,041 *Haemophilus influenzae* sequences (NCBI) | 154,808,555 | 15 | 1.97 | 9.40 | 2.26 | 1.34 | 4.2× |
| `coreutils` | All 9 GNU Coreutils 5.x source releases, binaries removed | 205,281,778 | 236 | 5.47 | 27.51 | 3.15 | 1.90 | 8.7× |
| `kernel` | 36 Linux kernel 1.0.x and 1.1.x source releases | 257,961,616 | 160 | 5.38 | 30.51 | 1.25 | 0.81 | 24.4× |
| `einstein.de.txt` | All revisions of the German Wikipedia article *Albert Einstein* up to 2010-01-12 | 92,758,441 | 117 | 5.04 | 25.18 | 0.15 | 0.11 | 167.0× |
| `einstein.en.txt` | All revisions of the English article up to 2006-11-10 | 467,626,544 | 139 | 4.96 | 31.14 | 0.09 | 0.07 | 334.6× |
| `world_leaders` | CIA *World Leaders* PDFs, 2003-01 to 2009-12, via `pdftotext` | 46,968,181 | 89 | 3.47 | 20.00 | 2.15 | 1.29 | 9.3× |

DNA files contain N and other IUPAC codes, hence σ > 4.

### Pseudo-real

A 1 MiB prefix of a text from the Pizza&Chili (non-repetitive) corpus, repeated to 100 MiB with random single-character substitutions.
In scheme 1 each copy mutates the first text; in scheme 2 each copy mutates the previous copy, resembling successive versions.
The name encodes base text, mutation rate, and scheme: `dblp.xml.0001.2` is DBLP XML, 0.01% mutation rate, scheme 2 (`00001` = 0.001%, `0001` = 0.01%, `001` = 0.1%).

| File | Base text | Bytes | σ | H0 | zstd 64 KiB % | zstd long % | 7z % | 64 KiB / long |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `dblp.xml.00001.1` | DBLP bibliography XML | 104,857,600 | 89 | 5.22 | 20.14 | 0.19 | 0.15 | 107.8× |
| `dblp.xml.00001.2` | DBLP bibliography XML | 104,857,600 | 89 | 5.22 | 20.28 | 0.20 | 0.15 | 103.5× |
| `dblp.xml.0001.1` | DBLP bibliography XML | 104,857,600 | 89 | 5.22 | 20.17 | 0.30 | 0.18 | 68.3× |
| `dblp.xml.0001.2` | DBLP bibliography XML | 104,857,600 | 89 | 5.22 | 21.47 | 0.30 | 0.18 | 71.3× |
| `dna.001.1` | Gutenberg human-genome DNA | 104,857,600 | 5 | 2.00 | 30.83 | 1.50 | 0.51 | 20.6× |
| `english.001.2` | Gutenberg English texts | 104,857,600 | 106 | 4.59 | 44.82 | 1.53 | 0.55 | 29.4× |
| `proteins.001.1` | Swissprot protein sequences | 104,857,600 | 21 | 4.18 | 34.48 | 1.48 | 0.59 | 23.3× |
| `sources.001.2` | Linux 2.6.11.6 and GCC 4.0.0 C/Java sources | 104,857,600 | 98 | 5.50 | 37.05 | 1.32 | 0.44 | 28.0× |

### Artificial

Binary words (bytes `0`/`1`) with extreme combinatorial repetitiveness.

| File | Contents | Bytes | σ | H0 | zstd 64 KiB % | zstd long % | 7z % | 64 KiB / long |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `fib41` | Fibonacci word F(41): F(n) = F(n−1)F(n−2); Sturmian | 267,914,296 | 2 | 0.96 | 1.88 | 0.02 | 0.18 | 113.1× |
| `rs.13` | Run-rich string R(13) of Franek et al. (runs ≈ 0.927 n) | 216,747,218 | 2 | 1.00 | 1.98 | 1.18 | 0.16 | 1.7× |
| `tm29` | Thue–Morse word T(29), length 2²⁸ | 268,435,456 | 2 | 1.00 | 0.16 | 0.11 | 0.36 | 1.5× |

### Logs

Program execution traces (AspectJ; function calls and returns without arguments) and the first 100 MiB of real system logs from [Loghub](https://github.com/logpai/loghub) and the SEC EDGAR log file data set.
Loghub has its own terms of use; check them before redistributing results that include log content.

| File | Contents | Bytes | σ | H0 | zstd 64 KiB % | zstd long % | 7z % | 64 KiB / long |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `Horspool` | Trace of Boyer–Moore–Horspool searching `ACGTACGT` in Pizza&Chili `dna` | 571,839,680 | 39 | 4.52 | 0.57 | 0.28 | 0.24 | 2.1× |
| `NQueens` | Trace of an N-Queens solver, N = 11 | 302,590,158 | 38 | 4.59 | 0.81 | 0.37 | 0.36 | 2.2× |
| `Quicksort` | Trace of quicksorting 100,000 random integers | 381,494,534 | 37 | 4.59 | 1.13 | 0.66 | 0.42 | 1.7× |
| `HDFS_1` | Hadoop distributed file system log | 104,857,600 | 73 | 5.22 | 11.17 | 9.50 | 6.62 | 1.2× |
| `BGL` | Blue Gene/L supercomputer log | 104,857,600 | 78 | 4.93 | 8.07 | 9.04 | 4.25 | 0.9× |
| `Thunderbird` | Thunderbird supercomputer log | 104,857,600 | 92 | 5.16 | 10.20 | 6.39 | 4.24 | 1.6× |
| `Windows` | Windows (CBS) log | 104,857,600 | 91 | 5.17 | 5.88 | 0.63 | 0.44 | 9.3× |
| `Android` | Android log | 104,857,600 | 176 | 5.51 | 15.52 | 7.60 | 4.86 | 2.0× |
| `EDGAR` | EDGAR log `log20170630` (SEC web server access log) | 104,857,600 | 71 | 3.83 | 13.78 | 11.42 | 7.78 | 1.2× |

## Measured LZ4 results

The 2026-09-27 run (`compare.py --corpus repcorpus --repeats 1 --encoders official-default official-highest greedy optimal`, LZ4 1.10.0, Apple M4) covers all 29 files.
It was interrupted after 6 files and continued with `--resume`; the rebuilt `lz4opt` reproduced the earlier frames byte for byte.
Percentages are frame bytes / input bytes; speeds are single measured runs, including process startup, and are indicative only.

| Encoder | Frame bytes | Output % | Total time (s) | MiB/s | Peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: |
| `official-default` | 1,620,776,282 | 28.41 | 5.5 | 991.9 | 25.0 |
| `official-highest` (`-12`) | 956,532,249 | 16.76 | 669.1 | 8.1 | 15.0 |
| `greedy` | 1,001,228,381 | 17.55 | 884.2 | 6.2 | 499.3 |
| `optimal` | 956,106,286 | 16.76 | 1,132.0 | 4.8 | 536.0 |

| File | default % | `-12` % | greedy % | optimal % | optimal − `-12` (bytes) | `-12` MiB/s | greedy MiB/s | optimal MiB/s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `Escherichia_Coli` | 55.96 | 35.65 | 36.70 | 35.64 | -10,616 | 2.1 | 4.7 | 3.9 |
| `cere` | 51.89 | 33.28 | 34.25 | 33.27 | -41,365 | 2.3 | 4.9 | 4.0 |
| `coreutils` | 38.44 | 27.76 | 29.37 | 27.76 | -183 | 20.9 | 4.5 | 3.5 |
| `einstein.de.txt` | 27.97 | 10.17 | 10.67 | 10.17 | -87 | 87.1 | 5.5 | 4.5 |
| `einstein.en.txt` | 37.69 | 21.11 | 22.20 | 21.11 | -66 | 44.1 | 4.3 | 3.8 |
| `influenza` | 28.25 | 7.53 | 8.49 | 7.52 | -9,132 | 9.4 | 6.5 | 4.8 |
| `kernel` | 43.05 | 31.27 | 32.94 | 31.27 | -46 | 23.4 | 4.5 | 3.7 |
| `para` | 53.75 | 34.38 | 35.39 | 34.38 | -38,805 | 1.6 | 4.6 | 3.9 |
| `world_leaders` | 28.39 | 20.05 | 20.90 | 20.05 | -3 | 27.0 | 5.8 | 4.3 |
| `dblp.xml.00001.1` | 27.31 | 19.51 | 20.80 | 19.51 | 0 | 32.8 | 5.7 | 4.2 |
| `dblp.xml.00001.2` | 27.60 | 19.69 | 20.98 | 19.69 | 0 | 32.5 | 5.7 | 4.3 |
| `dblp.xml.0001.1` | 27.37 | 19.55 | 20.84 | 19.55 | 0 | 32.8 | 5.7 | 4.3 |
| `dblp.xml.0001.2` | 30.22 | 21.30 | 22.58 | 21.30 | 0 | 28.6 | 5.3 | 4.1 |
| `dna.001.1` | 55.12 | 34.14 | 35.29 | 34.13 | -10,356 | 2.0 | 4.7 | 3.7 |
| `english.001.2` | 70.64 | 49.54 | 52.47 | 49.54 | 0 | 17.5 | 3.4 | 3.0 |
| `proteins.001.1` | 62.34 | 49.44 | 50.11 | 49.44 | -31 | 38.3 | 3.9 | 3.3 |
| `sources.001.2` | 55.91 | 40.50 | 42.88 | 40.50 | -2 | 15.8 | 3.5 | 3.1 |
| `fib41` | 27.21 | 0.40 | 0.40 | 0.40 | -4,231 | 818.3 | 13.3 | 8.8 |
| `rs.13` | 0.51 | 0.40 | 0.40 | 0.40 | -2,312 | 844.4 | 12.4 | 8.4 |
| `tm29` | 27.04 | 0.41 | 0.41 | 0.41 | -5,696 | 492.3 | 12.6 | 8.5 |
| `Android` | 20.04 | 14.05 | 15.17 | 14.05 | -92 | 37.6 | 6.3 | 4.4 |
| `BGL` | 16.29 | 11.64 | 12.87 | 11.64 | 0 | 15.4 | 6.6 | 5.2 |
| `EDGAR` | 21.85 | 14.09 | 15.71 | 14.09 | -2 | 17.0 | 6.7 | 4.5 |
| `HDFS_1` | 18.17 | 12.07 | 13.35 | 12.07 | -14 | 26.1 | 5.5 | 4.5 |
| `Horspool` | 1.01 | 0.54 | 0.53 | 0.52 | -148,357 | 65.8 | 10.6 | 7.3 |
| `NQueens` | 1.22 | 0.56 | 0.56 | 0.54 | -59,951 | 87.6 | 10.4 | 7.2 |
| `Quicksort` | 1.22 | 0.60 | 0.61 | 0.58 | -92,689 | 70.2 | 10.6 | 7.3 |
| `Thunderbird` | 13.67 | 10.09 | 10.89 | 10.08 | -857 | 27.7 | 7.6 | 5.2 |
| `Windows` | 9.04 | 6.99 | 7.40 | 6.99 | -1,070 | 68.5 | 8.1 | 5.5 |

Observations:

* **`optimal` is never larger than `-12`** on any of the 29 files, as on Silesia.
  The two are within 0.08% on real, pseudo-real, and system-log files, and 0.3–0.5% apart on the artificial words.
  The clear gains are on execution traces (`Horspool` 4.8%, `Quicksort` 4.0%, `NQueens` 3.5% smaller than `-12`), whose long, dense matches give a parser many alternative splits.
* **`greedy`** is larger than `-12` by up to 12.8% (`influenza`); system logs are 8–12% larger.
  It is 1.8% smaller on `Horspool` and within 0.04% on the artificial words.
* **Distant repetition is invisible to LZ4.**
  For `einstein.en.txt` the best LZ4 frame is 21.11% of the input against 0.07% for 7z; pseudo-real files compress to the level of their 1 MiB base text (for example `english.001.2` at 49.5%).
  This matches the 64 KiB / long ratios above.
* **The artificial words reach the format's limit.**
  A long match costs about one length-extension byte per 255 bytes, so no LZ4 frame can go much below 1/255 ≈ 0.39%.
  `fib41`, `rs.13`, and `tm29` all land at 0.40–0.41% for `-12`, `greedy`, and `optimal`.
  The default fast mode, which skips input when it finds no match, collapses on `fib41` and `tm29` (27%).
* **Speed depends on the data for `-12`, much less for `lz4opt`.**
  On DNA (`cere`, `para`, `Escherichia_Coli`, `dna.001.1`) `-12` runs at 1.6–2.3 MiB/s, slower than both `greedy` (4.6–4.9) and `optimal` (3.7–4.0), while on the artificial words it exceeds 490 MiB/s.
  `lz4opt` stays between 3 and 13 MiB/s on every file, and its peak RSS stays below 540 MiB with 4 MiB blocks.
* The whole run took about 2 h 20 min of wall time, dominated by `greedy` and `optimal` (warmup plus one measured pass each).

## Choosing files

* **Local repetition (LZ4 can exploit it):** the logs (except `Windows`), the execution traces, `tm29`, and `rs.13`.
  These show how parsers compare when matches are long and plentiful.
* **Distant repetition only:** `einstein.*`, `kernel`, `cere`, `para`, `world_leaders`, and all pseudo-real files.
  LZ4 results reflect the underlying text, not the corpus's repetitiveness; they are useful as a control and as a reminder of LZ4's window limit.
* **Mixed:** `Escherichia_Coli`, `influenza`, `coreutils`, `fib41`, and `Windows` have some redundancy on both scales.
* **Parser differences:** the execution traces show the largest `optimal` vs `-12` gap; the artificial words show none, because every parser reaches the format limit.
* **Run time:** one pass of `greedy` or `optimal` over the whole corpus takes 15–19 minutes; a full run with warmups takes about 2.5 hours.
  Use `--files` for a quicker subset, for example the traces and a few logs.
