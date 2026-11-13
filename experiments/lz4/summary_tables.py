#!/usr/bin/env python3
"""Print the summary tables of the full comparison as Markdown.

Reads the results.json of a Silesia run and of a repetitive-corpus run made
by compare.py (both with --levels span and --wide-lz4), and the lz77stats
JSON lines for Silesia. Each (file, encoder) may come from several
results.json files; the first one that has it wins. Differences are each encoder's compression ratio minus that of the optimal encoder (LZ4opt).
"""

import argparse
import json
import math

NAMES = {
    "official-default": "LZ4 default",
    "official-hc-default": "LZ4HC -9",
    "official-opt-lowest": "LZ4HC -10",
    "official-highest": "LZ4HC -12",
    "official-highest-wide": "LZ4HC -12w",
    "greedy": "greedy",
    "optimal": "LZ4opt",
}
MIB = 2**20


def load(paths):
    records = {}
    for path in paths:
        for r in json.load(open(path))["records"]:
            records.setdefault(r["file"], {}).setdefault(r["encoder"], r)
    return records


def points(d, decimals=3):
    """Formats a ratio difference with the given decimals, or one significant digit below them."""
    if d == 0:
        return "0"
    if abs(d) >= 10**-decimals:
        digits = decimals
    else:
        d = float(f"{d:.0e}")
        digits = -math.floor(math.log10(abs(d)))
    text = f"{d:+.{digits}f}"
    return "0" if float(text) == 0 else text.replace("-", "−")


def totals(records):
    out = {}
    for enc in NAMES:
        rs = [files[enc] for files in records.values() if enc in files]
        if len(rs) != len(records):
            continue
        inp = sum(r["input_bytes"] for r in rs)
        out[enc] = {
            "bytes": sum(r["compressed_bytes"] for r in rs),
            "ratio": inp / sum(r["compressed_bytes"] for r in rs),
            "speed": inp / MIB / sum(r["median_seconds"] for r in rs),
            "rss": max(r["max_peak_rss_bytes"] for r in rs) / MIB,
            "sequences": sum(r["stats"]["counts"]["sequences"] for r in rs),
        }
    return out


def corpus_table(title, records, rss):
    t = totals(records)
    base = t["optimal"]["ratio"]
    head = "| Encoder | Compressed size (bytes) | Ratio | Ratio difference | Compression speed (MiB/s) |" + (" Peak RSS (MiB) |" if rss else "")
    print(f"### {title}\n\n{head}\n| --- | ---: | ---: | ---: | ---: |" + (" ---: |" if rss else ""))
    for enc, v in t.items():
        row = (f"| {NAMES[enc]} | {v['bytes']:,} | {v['ratio']:.3f} | {points(v['ratio'] - base)} "
               f"| {v['speed']:.1f} |")
        print(row + (f" {v['rss']:.1f} |" if rss else ""))
    print()
    return t


def file_ratio(files, enc):
    r = files[enc]
    return r["input_bytes"] / r["compressed_bytes"]


def facts(title, records):
    print(f"### {title}: numbers in the text\n")
    n = len(records)
    for enc in ["official-highest", "official-highest-wide"]:
        if not all(enc in f for f in records.values()):
            continue
        gaps = {name: f[enc]["compressed_bytes"] - f["optimal"]["compressed_bytes"] for name, f in records.items()}
        pts = {name: file_ratio(f, "optimal") - file_ratio(f, enc) for name, f in records.items()}
        print(f"- Gap between {NAMES[enc]} and LZ4opt: {sum(gaps.values()):,} bytes in total, "
              f"identical in {sum(1 for g in gaps.values() if g == 0)}/{n} files, "
              f"at most {max(gaps.values()):,} bytes, ratio difference under 0.001 in {sum(1 for p in pts.values() if p < 0.001)}/{n} files")
    if all("official-opt-lowest" in f and "official-hc-default" in f for f in records.values()):
        worse = [name for name, f in records.items()
                 if f["official-opt-lowest"]["compressed_bytes"] > f["official-hc-default"]["compressed_bytes"]]
        print(f"- Files where -10 is larger than -9: {len(worse)}/{n} {worse}")
    seq = {enc: sum(f[enc]["stats"]["counts"]["sequences"] for f in records.values())
           for enc in ["greedy", "optimal"]}
    print(f"- Sequences: greedy {seq['greedy']:,}, LZ4opt {seq['optimal']:,} ({seq['greedy'] / seq['optimal'] - 1:+.1%})")
    slower = [name for name, f in records.items()
              if f["official-highest"]["median_seconds"] > f["optimal"]["median_seconds"]]
    print(f"- Files where -12 is slower than LZ4opt: {slower}")
    print()


def gap_table(records, threshold, limit=5):
    print(f"### Top {limit} files by compression ratio gap between -12 and LZ4opt\n")
    print("| File | LZ4HC -12 | LZ4HC -12w | LZ4opt | Ratio difference of -12 | Ratio difference of -12w |")
    print("| --- | ---: | ---: | ---: | ---: | ---: |")
    rows = []
    for name, f in records.items():
        opt = file_ratio(f, "optimal")
        if opt - file_ratio(f, "official-highest") >= threshold:
            rows.append((file_ratio(f, "official-highest") - opt, name, f, opt))
    for d, name, f, opt in sorted(rows)[:limit]:
        wide = file_ratio(f, "official-highest-wide") if "official-highest-wide" in f else float("nan")
        print(f"| {name} | {file_ratio(f, 'official-highest'):.2f} | {wide:.2f} | {opt:.2f} "
              f"| {points(round(d, 2), 2)} | {points(round(wide - opt, 2), 2)} |")
    print()



def match_count_table(records, threshold, limit=5):
    print(f"### Match counts for the top {limit} files by compression ratio gap between -12 and LZ4opt\n")
    encoders = ["official-highest", "official-highest-wide", "optimal"]
    print("| File | " + " | ".join(NAMES[enc] for enc in encoders) + " | Gap of -12 | Gap of -12w |")
    print("| --- | ---: | ---: | ---: | ---: | ---: |")
    rows = []
    for name, files in records.items():
        gap = file_ratio(files, "official-highest") - file_ratio(files, "optimal")
        if -gap >= threshold:
            rows.append((gap, name, files))
    for _, name, files in sorted(rows)[:limit]:
        counts = [f"{files[enc]['stats']['counts']['matches']:,}" if enc in files else "-"
                  for enc in encoders]
        base = files["optimal"]["stats"]["counts"]["matches"]
        gaps = []
        for enc in encoders[:2]:
            if enc not in files:
                gaps.append("-")
                continue
            gap = files[enc]["stats"]["counts"]["matches"] - base
            gaps.append(f"{gap:+,}".replace("-", "−") if gap else "0")
        print("| " + name + " | " + " | ".join(counts + gaps) + " |")
    print()


def lz77_table(records, lz77_path):
    parses = {}
    for line in open(lz77_path):
        j = json.loads(line)
        p = parses.setdefault(j["parse"], {"phrases": 0, "none": 0, "huffman": 0, "input": 0})
        p["phrases"] += j["phrases"]
        p["input"] += j["input_bytes"]
        for e in j["encodings"]:
            p[e["name"]] += e["bytes"]
    t = totals(records)
    print("### Comparison with LZ76 and LZ77\n")
    print("| Parse | Phrases | Compressed (byte-oriented) | Compressed (Huffman) |\n| --- | ---: | ---: | ---: |")
    for key, name in [("lz76", "LZ76"), ("lz77", "LZ77 ($w=65535$)")]:
        p = parses[key]
        print(f"| {name} | {p['phrases']:,} | {p['none']:,} | {p['huffman']:,} |")
    for enc in ["official-default", "official-highest", "official-highest-wide", "optimal"]:
        if enc in t:
            label = NAMES[enc]
            print(f"| {label} | {t[enc]['sequences']:,} | {t[enc]['bytes']:,} | - |")
    base = t["optimal"]["ratio"]
    print()
    for key in ["lz77", "lz76"]:
        p = parses[key]
        r = p["input"] / p["huffman"]
        print(f"- {key} Huffman ratio {r:.3f} (ratio difference from LZ4opt {points(r - base)})")
    print(f"- Phrases LZ77/LZ76 = {parses['lz77']['phrases'] / parses['lz76']['phrases'] - 1:+.0%}")
    print()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--silesia", nargs="+", required=True, help="Silesia results.json files")
    parser.add_argument("--repcorpus", nargs="+", required=True, help="repetitive corpus results.json files")
    parser.add_argument("--lz77", required=True, help="lz77stats JSON lines for Silesia")
    args = parser.parse_args()
    silesia, repcorpus = load(args.silesia), load(args.repcorpus)
    corpus_table("Silesia corpus overall", silesia, rss=True)
    facts("Silesia", silesia)
    corpus_table("Repetitive corpus overall", repcorpus, rss=True)
    facts("Repetitive corpus", repcorpus)
    gap_table(repcorpus, 0.1)
    match_count_table(repcorpus, 0.1)
    lz77_table(silesia, args.lz77)


if __name__ == "__main__":
    main()
