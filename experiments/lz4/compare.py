#!/usr/bin/env python3
"""Compare official LZ4 levels and the greedy and optimal encoders on a corpus."""

import argparse
from collections import Counter
from datetime import datetime, timezone
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import random
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

import download
import download_repcorpus
from download import BASE, sha256

ROOT = BASE.parent.parent
MIB = 1024 * 1024
# Name -> (file sizes, data subdirectory, manifest file, report title).
CORPORA = {
    "silesia": (download.FILES, "silesia", "manifest.json", "Silesia corpus"),
    "repcorpus": (download_repcorpus.SIZES, "repcorpus", "repcorpus-manifest.json",
                  "Pizza&Chili repetitive corpus"),
}
LEVELS = {
    "standard": {"lowest": ["-1"], "default": [], "highest": ["-12"]},
    "fast": {"fast8": ["--fast=8"], "default": [], "highest": ["-12"]},
    "hc": {"hc-lowest": ["-3"], "hc-default": ["-9"], "highest": ["-12"]},
    "span": {"default": [], "hc-default": ["-9"], "opt-lowest": ["-10"], "highest": ["-12"]},
}


def capture(command):
    result = subprocess.run(command, cwd=ROOT, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, check=True)
    return result.stdout.strip()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def measure(command, source, destination, env):
    """Measure one child, including process startup and buffered file I/O."""
    # Read before every measurement to give every encoder a warm input cache.
    with source.open("rb") as stream:
        while stream.read(MIB):
            pass
    with source.open("rb") as src, destination.open("wb") as dst, tempfile.TemporaryFile() as err:
        start = time.perf_counter()
        with subprocess.Popen(command, stdin=src, stdout=dst, stderr=err, env=env) as child:
            _, status, usage = os.wait4(child.pid, 0)
            elapsed = time.perf_counter() - start
            child.returncode = os.waitstatus_to_exitcode(status)
            if child.returncode:
                err.seek(0)
                raise RuntimeError(f"{command} exited {child.returncode}: {err.read().decode(errors='replace')}")
    rss = usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024)
    return {"seconds": elapsed, "peak_rss_bytes": rss,
            "user_seconds": usage.ru_utime, "system_seconds": usage.ru_stime}


def verify(command, compressed, expected_hash, env):
    digest = hashlib.sha256()
    with compressed.open("rb") as src, tempfile.TemporaryFile() as err:
        with subprocess.Popen(command, stdin=src, stdout=subprocess.PIPE, stderr=err, env=env) as child:
            while chunk := child.stdout.read(MIB):
                digest.update(chunk)
            if child.wait():
                err.seek(0)
                raise RuntimeError(err.read().decode(errors="replace"))
    if digest.hexdigest() != expected_hash:
        raise ValueError(f"Round-trip mismatch: {compressed}")


def histogram_summary(histogram):
    pairs = sorted((int(k), v) for k, v in histogram.items())
    count = sum(v for _, v in pairs)
    if not count:
        return {"count": 0, "mean": None, "p50": None, "p90": None, "p99": None, "max": None}
    result = {"count": count, "mean": sum(k * v for k, v in pairs) / count, "max": pairs[-1][0]}
    for label, q in [("p50", .5), ("p90", .9), ("p99", .99)]:
        position, cumulative = math.ceil(count * q), 0
        for length, frequency in pairs:
            cumulative += frequency
            if cumulative >= position:
                result[label] = length
                break
    return result


def aggregate(records):
    groups = {}
    for record in records:
        group = groups.setdefault(record["encoder"], {
            "input_bytes": 0, "compressed_bytes": 0, "seconds": 0,
            "peak_rss_bytes": 0, "counts": Counter(), "histograms": {},
        })
        group["input_bytes"] += record["input_bytes"]
        group["compressed_bytes"] += record["compressed_bytes"]
        group["seconds"] += record["median_seconds"]
        group["peak_rss_bytes"] = max(group["peak_rss_bytes"], record["max_peak_rss_bytes"])
        group["counts"].update(record["stats"]["counts"])
        for name, hist in record["stats"]["histograms"].items():
            group["histograms"].setdefault(name, Counter()).update(hist)
    for group in groups.values():
        group["ratio"] = group["input_bytes"] / group["compressed_bytes"]
        group["compressed_percent"] = 100 * group["compressed_bytes"] / group["input_bytes"]
        group["mib_per_second"] = group["input_bytes"] / MIB / group["seconds"]
        group["distributions"] = {name: histogram_summary(hist)
                                  for name, hist in group["histograms"].items()}
    return groups


def report(output, metadata, records):
    groups = aggregate(records)
    write_json(output / "results.json", {"metadata": metadata, "records": records, "aggregate": groups})
    with (output / "measurements.csv").open("w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(["file", "encoder", "trial", "input_bytes", "compressed_bytes", "seconds", "peak_rss_bytes", "user_seconds", "system_seconds"])
        for record in records:
            for trial, sample in enumerate(record["samples"], 1):
                writer.writerow([record["file"], record["encoder"], trial, record["input_bytes"], record["compressed_bytes"],
                                 *[sample[k] for k in ["seconds", "peak_rss_bytes", "user_seconds", "system_seconds"]]])
    with (output / "histograms.csv").open("w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(["file", "encoder", "histogram", "value_bytes_or_count", "frequency"])
        for record in records:
            for name, hist in record["stats"]["histograms"].items():
                for value, frequency in sorted(hist.items(), key=lambda p: int(p[0])):
                    writer.writerow([record["file"], record["encoder"], name, value, frequency])
    (output / "report.md").write_text(render(metadata, groups, records, published=False))


def frame_breakdown(g):
    """Splits frame bytes into LZ4 fields; every frame has the checked fixed layout."""
    c = g["counts"]
    # Magic, FLG, BD, HC, end mark, content checksum; 4-byte block headers.
    headers = 15 * c["frames"] + 4 * c["blocks"]
    tokens, offsets = c["sequences"], 2 * c["matches"]
    extensions = g["compressed_bytes"] - headers - tokens - offsets - c["literal_bytes"] - c["uncompressed_bytes"]
    return {"literals": c["literal_bytes"], "tokens": tokens, "offsets": offsets, "extensions": extensions,
            "raw": c["uncompressed_bytes"], "headers": headers}


def render(metadata, groups, records, published):
    title = CORPORA[metadata.get("corpus_name", "silesia")][3]
    lines = [f"# LZ4 compression comparison on the {title}", "",
             f"Started (UTC): {metadata['started_utc']}", "",
             f"Platform: {metadata['platform']}, CPU: {metadata['cpu']}",
             f"Rust: {metadata['rustc_version']}, LZ4: {metadata['lz4_version']}", "",
             "## Measurement conditions", "",
             f"* After one unmeasured warm-up run per file and encoder, each was measured {metadata['repeats']} times. "
             f"The run order was shuffled for every trial with seed {metadata['seed']}.",
             "* 4 MiB independent blocks with a content checksum. The official LZ4 ran with one worker (`-T1`), "
             "and greedy and optimal ran as the single-threaded Rust implementation.",
             "* The input was read just before each run to warm the file cache. The elapsed time includes process startup "
             "and buffered file I/O, and excludes building, frame parsing, and decoding. fsync and cold-cache measurements were not made.",
             "* The total time is the sum of the per-file medians, and the throughput is the total input size divided by the total time.",
             "* The peak RSS is the maximum over all trials of each process's peak, and includes the runtime and I/O buffers.",
             "* The compression ratio is the original bytes divided by the frame bytes (larger is better), and the output rate is its inverse as a percentage (smaller is better).",
             "* This is a local CLI measurement, not a reproduction of the official benchmark (in-memory measurement with lzbench).",
             f"* Level set: `{metadata['levels']}`. In `standard`, lowest (`-1`) and default "
             "(no level given, `LZ4_CLEVEL` removed) are the same setting, so their time difference is noise."]
    merged = metadata.get("merged_from")
    if merged:
        lines += [f"* The results of {', '.join(f'`{e}`' for e in merged['encoders'])} were taken from an earlier run "
                  f"(started {merged['started_utc']}, commit `{merged['git_commit'][:7]}`). "
                  f"This run measured only {', '.join(f'`{e}`' for e in metadata['encoders'])}. "
                  "Times and RSS were measured at different times, so treat comparisons between encoders as a rough guide. "
                  "Frame sizes and statistics are deterministic and can be compared directly."]
    resumed = metadata.get("resumed_from")
    if resumed:
        lines += [f"* The results of {', '.join(f'`{f}`' for f in resumed['files'])} were carried over from an interrupted run "
                  f"with the same settings and binary (started {resumed['started_utc']}). "
                  "Times and RSS are the values measured in that run."]
        check = resumed["rebuilt_check"]
        if check:
            lines += [f"* The binary hash of {', '.join(f'`{e}`' for e in check['encoders'])} changed because of a rebuild, "
                      f"so `{check['file']}` was compressed again and the output matched the interrupted run."]
    lines += ["", "## Overall results", "",
             "| Encoder | Frame size (bytes) | Ratio | Output % | Time (s) | MiB/s | Peak RSS (MiB) |",
             "| --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for label, g in groups.items():
        lines.append(f"| {label} | {g['compressed_bytes']:,} | {g['ratio']:.3f} | {g['compressed_percent']:.2f} | {g['seconds']:.3f} | {g['mib_per_second']:.2f} | {g['peak_rss_bytes']/MIB:.2f} |")
    lines += ["", "## Frame statistics", "",
              "Literal bytes count only the literals in compressed blocks; the bytes of uncompressed blocks are shown separately. "
              "The sequence count includes the literal-only sequence at the end of each compressed block. "
              "A match length is the length of the match actually emitted, not necessarily the longest match at that position.", "",
              "| Encoder | Blocks (uncompressed) | Sequences | Matches | Literal runs | Literal bytes | Match bytes | Uncompressed bytes |",
              "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for label, g in groups.items():
        c = g["counts"]
        lines.append(f"| {label} | {c['blocks']} ({c['uncompressed_blocks']}) | {c['sequences']:,} | {c['matches']:,} | {c['literal_runs']:,} | {c['literal_bytes']:,} | {c['match_bytes']:,} | {c['uncompressed_bytes']:,} |")
    lines += ["", "## Frame size breakdown", "",
              "A token takes 1 byte per sequence, and an offset takes 2 bytes per match. "
              "Length extensions are the bytes added for literal lengths of 15 or more and match lengths of 19 or more, "
              "and were computed by subtracting the other components from the frame size. "
              "Headers are the frame header, the end mark, the content checksum, and the block headers.", "",
              "| Encoder | Literals | Tokens | Offsets | Length extensions | Uncompressed blocks | Headers | Total |",
              "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for label, g in groups.items():
        b = frame_breakdown(g)
        lines.append(f"| {label} | {b['literals']:,} | {b['tokens']:,} | {b['offsets']:,} | {b['extensions']:,} | {b['raw']:,} | {b['headers']:,} | {g['compressed_bytes']:,} |")
    lines += ["", "## Short matches and consecutive matches", "",
              "| Encoder | Matches of length 4 | Length 5 | Length 6 | Length 19 or more | Sequences with literal length 0 |",
              "| --- | ---: | ---: | ---: | ---: | ---: |"]
    for label, g in groups.items():
        m = {int(k): v for k, v in g["histograms"]["match_length"].items()}
        literal = {int(k): v for k, v in g["histograms"]["literal_length"].items()}
        long = sum(v for k, v in m.items() if k >= 19)
        lines.append(f"| {label} | {m.get(4, 0):,} | {m.get(5, 0):,} | {m.get(6, 0):,} | {long:,} | {literal.get(0, 0):,} |")
    titles = {"match_length": "Match length", "literal_length": "Literal length (per sequence, including 0)",
              "match_offset": "Match offset", "sequences_per_block": "Sequences per block",
              "longest_match_per_block": "Longest match length in a block"}
    for name, title in titles.items():
        lines += ["", f"## Distribution: {title} (`{name}`)", "", "| Encoder | Count | Mean | p50 | p90 | p99 | Max |",
                  "| --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
        for label, g in groups.items():
            d = g["distributions"][name]
            mean = f"{d['mean']:.2f}" if d["mean"] is not None else "n/a"
            lines.append(f"| {label} | {d['count']:,} | {mean} | {d['p50']} | {d['p90']} | {d['p99']} | {d['max']} |")
    lines += ["", "## Results per file", "", "| File | Encoder | Frame size (bytes) | Ratio | Median time s (min–max) | MiB/s | Peak RSS (MiB) |",
              "| --- | --- | ---: | ---: | ---: | ---: | ---: |"]
    for r in records:
        times = [s["seconds"] for s in r["samples"]]
        lines.append(f"| {r['file']} | {r['encoder']} | {r['compressed_bytes']:,} | {r['input_bytes']/r['compressed_bytes']:.3f} | {r['median_seconds']:.3f} ({min(times):.3f}–{max(times):.3f}) | {r['input_bytes']/MIB/r['median_seconds']:.2f} | {r['max_peak_rss_bytes']/MIB:.2f} |")
    lines += ["", "## Verification", "",
              "The SHA-256 of every compressed output was the same across the repeated runs. Each output was decoded "
              "with the official LZ4, and its SHA-256 matched the input. The frame settings were checked, and the total "
              "of the decoded bytes matched the input size.", ""]
    if published:
        lines += ["The individual measurements are in `measurements.csv`, and the aggregate statistics and the metadata for reproduction are in `summary.json`. "
                  "The full histograms and the per-block details are in the run directory (`results.json`, "
                  "`histograms.csv`).", ""]
    else:
        lines += ["The distributions and the per-block details are in `results.json`, the individual measurements in `measurements.csv`, "
                  "and the per-file histograms in `histograms.csv`.", ""]
    return "\n".join(lines)


def combined(previous, records, files, labels):
    """Orders merged and new records by input file, then by encoder."""
    return sorted(previous + records, key=lambda r: (files.index(r["file"]), labels.index(r["encoder"])))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", type=Path, default=BASE / "data")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--publish", type=Path, help="Also save a compact summary report directory")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--levels", choices=LEVELS, default="standard")
    parser.add_argument("--corpus", choices=CORPORA, default="silesia")
    parser.add_argument("--files", nargs="+", help="Measure only these files of the corpus")
    parser.add_argument("--seed", type=int, default=20260927)
    parser.add_argument("--encoders", nargs="+", help="Measure only these encoder labels")
    parser.add_argument("--wide-lz4", type=Path,
                        help="lz4 built by build_lz4_wide.sh; adds official-highest-wide (-12 with LZ4_OPT_NUM 65536)")
    parser.add_argument("--merge", type=Path, help="Include other encoders' records from a previous results.json")
    parser.add_argument("--resume", type=Path,
                        help="Reuse files completed by an interrupted run's results.json with identical settings")
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error("--repeats must be positive")
    if args.publish is not None and args.publish.exists():
        parser.error("--publish directory already exists; choose a new directory")
    if sys.platform not in ("darwin", "linux"):
        parser.error("Peak RSS measurement supports macOS and Linux only")
    lz4 = shutil.which("lz4")
    if not lz4:
        parser.error("Official lz4 CLI is required")
    sizes, subdirectory, manifest_name, _ = CORPORA[args.corpus]
    unknown = set(args.files or []) - set(sizes)
    if unknown:
        parser.error(f"not in {args.corpus}: {', '.join(sorted(unknown))}")
    manifest = json.loads((args.data_dir / manifest_name).read_text())
    files = args.files or list(sizes)
    for name in files:
        path = args.data_dir / subdirectory / name
        if path.stat().st_size != sizes[name] or sha256(path) != manifest["files"][name]["sha256"]:
            raise ValueError(f"Corpus validation failed: {name}")
    output = (args.output or BASE / "runs" / datetime.now().strftime("%Y%m%d-%H%M%S")).resolve()
    output.mkdir(parents=True, exist_ok=False)
    binaries = BASE / "bin"
    binaries.mkdir(exist_ok=True)
    env = dict(os.environ)
    env.pop("LZ4_CLEVEL", None)
    env.pop("LZ4_NBWORKERS", None)
    names = ["lz4opt", "framestats"]
    subprocess.run(["cargo", "build", "--release", *[f"--bin={name}" for name in names]], cwd=ROOT, env=env,
                   check=True)
    for name in names:
        shutil.copy2(ROOT / "target" / "release" / name, binaries / name)
    common = [lz4, "-z", "-q", "-c", "-BI", "-B7", "-T1", "--frame-crc", "--no-content-size"]
    commands = {f"official-{name}": common + flags for name, flags in LEVELS[args.levels].items()}
    commands["greedy"] = [str(binaries / "lz4opt"), "-block-size", str(4 * MIB), "-parser", "greedy"]
    commands["optimal"] = [str(binaries / "lz4opt"), "-block-size", str(4 * MIB), "-parser", "optimal"]
    if args.wide_lz4:
        commands["official-highest-wide"] = [str(args.wide_lz4.resolve())] + common[1:] + ["-12"]
    labels = list(commands)
    if args.encoders:
        unknown = set(args.encoders) - set(commands)
        if unknown:
            parser.error(f"unknown encoders: {', '.join(sorted(unknown))}; choose from {', '.join(commands)}")
        commands = {label: command for label, command in commands.items() if label in args.encoders}
    previous = []
    merged_from = None
    if args.merge is not None:
        prior = json.loads(args.merge.read_text())
        pm = prior["metadata"]
        if pm["corpus"]["files"] != manifest["files"] or pm["levels"] != args.levels:
            parser.error("--merge results use a different corpus or level set")
        previous = [r for r in prior["records"] if r["encoder"] not in commands and r["file"] in files]
        merged_from = {"path": str(args.merge.resolve()), "started_utc": pm["started_utc"],
                       "git_commit": pm["git_commit"], "script_sha256": pm["script_sha256"],
                       "repeats": pm["repeats"], "seed": pm["seed"],
                       "encoders": sorted({r["encoder"] for r in previous}, key=labels.index),
                       "commands": {k: v for k, v in pm["commands"].items() if k not in commands}}
    binary_sha256 = {name: sha256(Path(command[0])) for name, command in commands.items()}
    resumed_from = None
    if args.resume is not None:
        if args.merge is not None:
            parser.error("--resume and --merge cannot be combined")
        prior = json.loads(args.resume.read_text())
        pm = prior["metadata"]
        same = [pm["corpus"]["files"] == manifest["files"], pm["levels"] == args.levels,
                pm["commands"] == commands, pm["repeats"] == args.repeats, pm["seed"] == args.seed]
        if not all(same):
            parser.error("--resume run uses a different corpus or settings")
        completed = {(r["file"], r["encoder"]) for r in prior["records"]}
        reused = [name for name in files if all((name, label) in completed for label in commands)]
        previous = [r for r in prior["records"] if r["file"] in reused and r["encoder"] in commands]
        resumed_from = {"path": str(args.resume.resolve()), "started_utc": pm["started_utc"],
                        "git_commit": pm["git_commit"], "script_sha256": pm["script_sha256"], "files": reused,
                        "binary_sha256": pm["binary_sha256"], "rebuilt_check": None}
        # A rebuild of unchanged sources can still produce different binary
        # bytes (embedded paths, toolchain). Require identical output on the
        # smallest reused file.
        rebuilt = [label for label in commands if pm["binary_sha256"][label] != binary_sha256[label]]
        if rebuilt and reused:
            name = min(reused, key=sizes.get)
            expected = {r["encoder"]: r["compressed_sha256"] for r in previous if r["file"] == name}
            with tempfile.TemporaryDirectory() as temporary:
                for label in rebuilt:
                    print(f"{name}: checking rebuilt {label} against the resumed run", flush=True)
                    destination = Path(temporary) / f"{label}.lz4"
                    measure(commands[label], args.data_dir / subdirectory / name, destination, env)
                    if sha256(destination) != expected[label]:
                        parser.error(f"rebuilt {label} output differs from the resumed run on {name}")
            resumed_from["rebuilt_check"] = {"file": name, "encoders": rebuilt}
    cpu = platform.processor()
    if sys.platform == "darwin":
        try:
            cpu = capture(["sysctl", "-n", "machdep.cpu.brand_string"])
        except subprocess.CalledProcessError:
            pass
    metadata = {"started_utc": datetime.now(timezone.utc).isoformat(), "platform": platform.platform(),
                "cpu": cpu, "rustc_version": capture(["rustc", "--version"]), "lz4_version": capture([lz4, "--version"]),
                "git_commit": capture(["git", "rev-parse", "HEAD"]), "git_status": capture(["git", "status", "--short"]),
                "commands": commands, "encoders": list(commands), "merged_from": merged_from, "resumed_from": resumed_from,
                "repeats": args.repeats, "seed": args.seed,
                "levels": args.levels, "files": files, "corpus_name": args.corpus, "corpus": manifest,
                "environment": {k: env.get(k) for k in ["RUSTFLAGS", "LZ4_CLEVEL", "LZ4_NBWORKERS"]},
                "binary_sha256": binary_sha256,
                "script_sha256": sha256(Path(__file__))}
    write_json(output / "metadata.json", metadata)
    records = []
    rng = random.Random(args.seed)
    for name in files:
        if resumed_from and name in resumed_from["files"]:
            for _ in range(args.repeats):  # Keep later shuffles as in the uninterrupted run.
                rng.shuffle(list(commands))
            continue
        source = args.data_dir / subdirectory / name
        sample_map = {label: [] for label in commands}
        hash_map = {}
        with tempfile.TemporaryDirectory(prefix="frames-", dir=output) as temporary:
            for label, command in commands.items():
                print(f"{name}: {label} warmup", flush=True)
                measure(command, source, Path(temporary) / f"{label}.lz4", env)
            for trial in range(args.repeats):
                order = list(commands)
                rng.shuffle(order)
                for label in order:
                    destination = Path(temporary) / f"{label}.lz4"
                    print(f"{name}: {label} trial {trial + 1}/{args.repeats}", flush=True)
                    sample = measure(commands[label], source, destination, env)
                    sample_map[label].append(sample)
                    digest = sha256(destination)
                    if hash_map.setdefault(label, digest) != digest:
                        raise ValueError(f"Non-deterministic output: {name}/{label}")
                    with (output / "progress.jsonl").open("a") as log:
                        log.write(json.dumps({"file": name, "encoder": label, "trial": trial + 1, **sample}) + "\n")
            for label in commands:
                destination = Path(temporary) / f"{label}.lz4"
                verify([lz4, "-d", "-q", "-c"], destination, manifest["files"][name]["sha256"], env)
                print(f"{name}: summarizing {label} with framestats", flush=True)
                stats = json.loads(capture([str(binaries / "framestats"), str(destination)]))
                counts = stats["counts"]
                if sum(counts[k] for k in ["literal_bytes", "match_bytes", "uncompressed_bytes"]) != sizes[name]:
                    raise ValueError(f"Decoded size mismatch: {name}/{label}")
                if stats["block_max_bytes"] != 4 * MIB or any(b["decoded_bytes"] > 4 * MIB for b in stats["block_details"]):
                    raise ValueError("Unexpected block size")
                samples = sample_map[label]
                records.append({"file": name, "encoder": label, "input_bytes": sizes[name],
                                "compressed_bytes": destination.stat().st_size, "compressed_sha256": hash_map[label],
                                "samples": samples, "median_seconds": statistics.median(s["seconds"] for s in samples),
                                "max_peak_rss_bytes": max(s["peak_rss_bytes"] for s in samples), "stats": stats})
        done = set(files[:files.index(name) + 1])
        report(output, metadata, combined([r for r in previous if r["file"] in done], records, files, labels))
    records = combined(previous, records, files, labels)
    if args.publish is not None:
        args.publish.mkdir(parents=True, exist_ok=False)
        groups = aggregate(records)
        (args.publish / "report.md").write_text(render(metadata, groups, records, published=True))
        for group in groups.values():
            del group["histograms"]
        write_json(args.publish / "summary.json", {"metadata": metadata, "aggregate": groups})
        shutil.copyfile(output / "measurements.csv", args.publish / "measurements.csv")
    print(f"Results: {output / 'report.md'}")


if __name__ == "__main__":
    main()
