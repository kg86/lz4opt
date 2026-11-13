#!/usr/bin/env python3
"""Measure how much of each corpus file's redundancy lies within LZ4's reach.

For every file this reports its alphabet size, order-0 entropy, and the output
size of the same zstd level with a 64 KiB window (LZ4's maximum offset is
65,535) and with a 2 GiB long-distance window. The gap between the two shows
how much repetition is too far away for any LZ4 encoder to exploit.
"""

import argparse
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
import json
import math
from pathlib import Path
import shutil
import subprocess

import download
import download_repcorpus
from download import BASE

MIB = 1024 * 1024
CORPORA = {
    "silesia": (download.FILES, "silesia", "manifest.json"),
    "repcorpus": (download_repcorpus.SIZES, "repcorpus", "repcorpus-manifest.json"),
}
WINDOWS = {"zstd_64k_bytes": ["--zstd=wlog=16"], "zstd_long_bytes": ["--long=31"]}


def compressed_size(zstd, path, flags):
    # A path argument, unlike stdin, lets zstd shrink the window to the file size.
    child = subprocess.Popen([zstd, "-q", "-c", "-3", "-T1", *flags, str(path)], stdout=subprocess.PIPE)
    total = 0
    while chunk := child.stdout.read(MIB):
        total += len(chunk)
    if child.wait():
        raise RuntimeError(f"zstd failed on {path}")
    return total


def measure(zstd, path):
    counts = Counter()
    with path.open("rb") as stream:
        while chunk := stream.read(16 * MIB):
            counts.update(chunk)
    size = sum(counts.values())
    entropy = -sum(c / size * math.log2(c / size) for c in counts.values()) if size else 0.0
    result = {"bytes": size, "alphabet": len(counts), "h0_bits": entropy}
    for key, flags in WINDOWS.items():
        result[key] = compressed_size(zstd, path, flags)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", choices=CORPORA, default="repcorpus")
    parser.add_argument("--data-dir", type=Path, default=BASE / "data")
    parser.add_argument("--output", type=Path, help="Write per-file results as JSON")
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    zstd = shutil.which("zstd")
    if not zstd:
        parser.error("zstd is required")
    sizes, subdirectory, manifest_name = CORPORA[args.corpus]
    manifest = json.loads((args.data_dir / manifest_name).read_text())
    paths = {name: args.data_dir / subdirectory / name for name in sizes}
    with ProcessPoolExecutor(args.jobs) as pool:
        futures = {name: pool.submit(measure, zstd, path) for name, path in paths.items()}
        results = {name: future.result() for name, future in futures.items()}
    for name, result in results.items():
        if result["bytes"] != sizes[name]:
            raise ValueError(f"Unexpected size for {name}")
        archive = manifest["files"][name].get("archive_bytes")
        if archive is not None:
            result["archive_7z_bytes"] = archive
    if args.output:
        args.output.write_text(json.dumps({"zstd": subprocess.run([zstd, "--version"], text=True,
                                           stdout=subprocess.PIPE).stdout.strip(), "files": results}, indent=2) + "\n")
    print("| File | Bytes | σ | H0 (bits) | zstd 64 KiB % | zstd long % | 7z % | 64 KiB / long |")
    print("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
    for name, r in results.items():
        pct = lambda n: f"{100 * n / r['bytes']:.2f}"
        archive = pct(r["archive_7z_bytes"]) if "archive_7z_bytes" in r else "—"
        print(f"| `{name}` | {r['bytes']:,} | {r['alphabet']} | {r['h0_bits']:.2f} | {pct(r['zstd_64k_bytes'])} | "
              f"{pct(r['zstd_long_bytes'])} | {archive} | {r['zstd_64k_bytes'] / r['zstd_long_bytes']:.1f}× |")


if __name__ == "__main__":
    main()
