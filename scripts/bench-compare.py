#!/usr/bin/env python3
"""Compare the speed of two revisions of the Rust crate with the `bench` program.

Each revision is built in its own Git worktree and target directory, then the
two builds are run alternately for several rounds on the same inputs, which
spreads out noise from the machine. For every workload and input the report
shows the minimum and median time of each revision, the ratio A / B (above 1
means B is faster), and whether the output hashes agree. A hash that differs
means the encoder's output changed, not only its speed.

Both revisions must contain src/bin/bench.rs (or rust/src/bin/bench.rs, the
layout used before the crate moved to the repository root). The pseudo-revision
WORKTREE means the current working tree, including uncommitted changes.

Examples:

  scripts/bench-compare.py HEAD~1 HEAD
  scripts/bench-compare.py --workload lz4-optimal --preset silesia-quick abc1234 WORKTREE
  scripts/bench-compare.py HEAD WORKTREE -- -workload lz4-optimal path/to/file
  scripts/bench-compare.py --rounds 9 main WORKTREE -- -synthetic 1048576

--workload NAME (repeatable) limits the comparison to the named workloads, for
example --workload lz4-optimal; without it bench runs its default set.
--preset silesia-quick or silesia compares on files of the Silesia corpus
(download it first). Arguments after `--` go to `bench` (see its usage) and add
to these; with neither a preset nor arguments the default is `-synthetic 4194304`. `-repeats` is controlled by --rounds, not by bench.
"""

import argparse
import json
import math
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
WORKTREE = "WORKTREE"
SYNTHETIC = ["-synthetic", str(4 * 1024 * 1024)]
SILESIA = ROOT / "experiments" / "lz4" / "data" / "silesia"
# Input files of --preset, relative to the Silesia directory (see
# experiments/lz4/download.py). "quick" spans text, XML, binary and image data.
PRESETS = {
    "silesia-quick": ["xml", "dickens", "mr", "x-ray", "sao"],
    "silesia": ["dickens", "mozilla", "mr", "nci", "ooffice", "osdb", "reymont", "samba", "sao", "webster", "x-ray",
                "xml"],
}


def preset_files(name):
    files = [SILESIA / f for f in PRESETS[name]]
    missing = [str(f) for f in files if not f.exists()]
    if missing:
        raise SystemExit(f"preset {name}: missing {', '.join(missing)}; run python3 experiments/lz4/download.py")
    return [str(f) for f in files]


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, check=True, text=True, capture_output=True).stdout.strip()


class Build:
    """A buildable copy of one revision and the path of its bench binary."""

    def __init__(self, rev, label, temporary):
        self.rev, self.label = rev, label
        self.peak_rss = []  # of each bench run, in bytes
        self.worktree = None
        if rev == WORKTREE:
            self.source = ROOT
            self.target = None  # use the working tree's own target directory
            self.commit = "working tree"
        else:
            self.commit = git("rev-parse", "--short", rev)
            self.worktree = temporary / f"src-{label}"
            git("worktree", "add", "--detach", str(self.worktree), rev)
            self.source = self.worktree
            self.target = temporary / f"target-{label}"
        for crate in (self.source, self.source / "rust"):
            if (crate / "src" / "bin" / "bench.rs").exists():
                self.crate = crate
                break
        else:
            raise SystemExit(f"{rev}: src/bin/bench.rs does not exist in this revision")

    def build(self):
        env = dict(os.environ)
        if self.target is not None:
            env["CARGO_TARGET_DIR"] = str(self.target)
        print(f"building {self.label} ({self.rev}, {self.commit})", file=sys.stderr, flush=True)
        subprocess.run(["cargo", "build", "--release", "--bin", "bench"], cwd=self.crate, env=env, check=True)
        target = self.target if self.target is not None else self.crate / "target"
        self.binary = target / "release" / "bench"

    def run(self, bench_args):
        """Runs bench once, notes its peak RSS in bytes, and returns its records."""
        with tempfile.TemporaryFile("w+") as out, tempfile.TemporaryFile("w+") as err:
            process = subprocess.Popen([str(self.binary), "-repeats", "1", *bench_args], stdout=out, stderr=err)
            # wait4 reports the resource usage of this child alone, unlike getrusage(RUSAGE_CHILDREN).
            _, status, usage = os.wait4(process.pid, 0)
            process.returncode = os.waitstatus_to_exitcode(status)  # keeps Popen from waiting again
            if process.returncode:
                err.seek(0)
                raise SystemExit(f"{self.label}: bench failed ({process.returncode}): {err.read().strip()}")
            # ru_maxrss is in bytes on macOS and in KiB on Linux.
            self.peak_rss.append(usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024))
            out.seek(0)
            return [json.loads(line) for line in out.read().splitlines()]

    def remove(self):
        if self.worktree is not None:
            subprocess.run(["git", "worktree", "remove", "--force", str(self.worktree)], cwd=ROOT, check=False,
                           capture_output=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], epilog="See the file header for details.")
    parser.add_argument("rev_a", help="baseline revision, or WORKTREE")
    parser.add_argument("rev_b", help="revision to compare, or WORKTREE")
    parser.add_argument("--rounds", type=int, default=5, help="alternating runs of each build (default 5)")
    parser.add_argument("--json", type=Path, help="also write the aggregated results to this file")
    parser.add_argument("--workload", action="append", default=[], metavar="NAME",
                        help="run only this bench workload; may be repeated")
    parser.add_argument("--preset", choices=sorted(PRESETS), help="compare on files of the Silesia corpus")
    parser.add_argument("bench_args", nargs="*", help="arguments for bench, after --")
    args = parser.parse_args()
    if args.rounds < 1:
        parser.error("--rounds must be at least 1")
    args.bench_args = bench_args_of(args)

    temporary = Path(tempfile.mkdtemp(prefix="lz4opt-bench-"))
    builds = []
    try:
        for rev, label in [(args.rev_a, "a"), (args.rev_b, "b")]:
            builds.append(Build(rev, label, temporary))
        for build in builds:
            build.build()
        samples = {build.label: {} for build in builds}
        for round_number in range(args.rounds):
            # Alternate which build goes first, so that neither always runs
            # while the machine is cooler or warmer.
            order = builds if round_number % 2 == 0 else builds[::-1]
            for build in order:
                print(f"round {round_number + 1}/{args.rounds}: {build.label}", file=sys.stderr, flush=True)
                seen = set()
                for record in build.run(args.bench_args):
                    key = (record["bench"], record["input"])
                    if key in seen:
                        raise ValueError(f"{build.label}: duplicate measurement {key}")
                    seen.add(key)
                    seconds = record["seconds"]
                    if not math.isfinite(seconds) or seconds <= 0:
                        raise ValueError(f"{build.label}: invalid time for {key}: {seconds}")
                    entry = samples[build.label].setdefault(key, {"seconds": [], "hashes": set(), "bytes": set()})
                    entry["seconds"].append(record["seconds"])
                    entry["hashes"].add(record["output_hash"])
                    entry["bytes"].add(record["output_bytes"])
        results = report(args, builds, samples)
        if args.json:
            args.json.write_text(json.dumps(results, indent=2) + "\n")
    finally:
        for build in builds:
            build.remove()
        shutil.rmtree(temporary, ignore_errors=True)


def validate_samples(samples, rounds):
    keys = set(samples["a"])
    if not keys or keys != set(samples["b"]):
        raise ValueError("measurement sets are empty or differ between builds")
    for label, measurements in samples.items():
        for key, entry in measurements.items():
            if len(entry["seconds"]) != rounds:
                raise ValueError(f"{label}: {key}: expected {rounds} measurements")


def spread(seconds):
    """Returns (max - min) / min of the times: how much the repeats disagree."""
    return (max(seconds) - min(seconds)) / min(seconds)


def verdict(a_seconds, b_seconds, floor=0.02):
    """Says whether B is faster or slower than A by more than the noise.

    The noise is the larger spread of the two builds, but at least `floor`.
    """
    threshold = max(spread(a_seconds), spread(b_seconds), floor)
    ratio = min(a_seconds) / min(b_seconds)
    if ratio > 1 + threshold:
        return "B faster"
    if ratio < 1 / (1 + threshold):
        return "B slower"
    return "same speed"


def geomean(values):
    return math.exp(sum(math.log(v) for v in values) / len(values))


def peak_rss_line(a, b):
    """Returns a line comparing the peak RSS of the bench processes, or None without measurements.

    The peak is the largest over all workloads and inputs of one run, so use --workload to isolate one.
    """
    ra, rb = getattr(a, "peak_rss", []), getattr(b, "peak_rss", [])
    if not ra or not rb:
        return None
    ma, mb = min(ra), min(rb)
    return f"Peak RSS of bench (minimum over rounds): A {ma / 2**20:.1f} MiB, B {mb / 2**20:.1f} MiB, B / A {mb / ma:.3f}"


def size_cell(sizes):
    return str(next(iter(sizes))) if len(sizes) == 1 else "unstable"


def report(args, builds, samples):
    validate_samples(samples, args.rounds)
    a, b = builds
    print(f"A = {args.rev_a} ({a.commit}), B = {args.rev_b} ({b.commit}); {args.rounds} rounds; "
          f"bench {' '.join(args.bench_args)}")
    print()
    print("| Workload | Input | A min (s) | B min (s) | A / B (min) | A median (s) | B median (s) | A / B (median) | A spread | B spread | Verdict | A bytes | B bytes | B / A (bytes) | Output |")
    print("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | --- |")
    results = []
    ratios = {}
    differing = 0
    for key in sorted(samples["a"]):
        sa, sb = samples["a"][key], samples["b"][key]
        a_min, b_min = min(sa["seconds"]), min(sb["seconds"])
        a_med, b_med = statistics.median(sa["seconds"]), statistics.median(sb["seconds"])
        same = (sa["hashes"] == sb["hashes"] and len(sa["hashes"]) == 1
                and sa["bytes"] == sb["bytes"] and len(sa["bytes"]) == 1)
        differing += not same
        ratios.setdefault(key[0], []).append(a_min / b_min)
        a_size, b_size = size_cell(sa["bytes"]), size_cell(sb["bytes"])
        size_ratio = f"{min(sb['bytes']) / min(sa['bytes']):.4f}" if min(sa["bytes"]) and len(sa["bytes"]) == len(sb["bytes"]) == 1 else "-"
        print(f"| {key[0]} | {key[1]} | {a_min:.4f} | {b_min:.4f} | {a_min / b_min:.2f} | {a_med:.4f} | {b_med:.4f} | "
              f"{a_med / b_med:.2f} | {spread(sa['seconds']):.1%} | {spread(sb['seconds']):.1%} | "
              f"{verdict(sa['seconds'], sb['seconds'])} | {a_size} | {b_size} | {size_ratio} | {'same' if same else 'DIFFERENT'} |")
        results.append({"bench": key[0], "input": key[1], "a_seconds": sa["seconds"], "b_seconds": sb["seconds"],
                        "a_hashes": sorted(sa["hashes"]), "b_hashes": sorted(sb["hashes"]),
                        "a_output_bytes": sorted(sa["bytes"]), "b_output_bytes": sorted(sb["bytes"]), "same_output": same})
    rss = peak_rss_line(a, b)
    if rss:
        print()
        print(rss)
    print()
    print("Geometric mean of A / B (min) per workload, over its inputs:")
    for workload in sorted(ratios):
        print(f"- {workload}: {geomean(ratios[workload]):.3f}")
    print()
    print("A / B above 1 means B is faster. Spread is (max - min) / min over the rounds; the verdict needs a "
          "difference of the minimums larger than the larger spread (at least 2%). "
          "Times of a few milliseconds or less are dominated by noise.")
    print("B / A (bytes) below 1 means B's output is smaller (better compression).")
    if differing:
        print(f"{differing} workload(s) produced different output: the change is not only a speed change.")
    return {"a": args.rev_a, "b": args.rev_b, "rounds": args.rounds, "bench_args": args.bench_args,
            "a_peak_rss": a.peak_rss, "b_peak_rss": b.peak_rss,
            "results": results}


def bench_args_of(args):
    """Returns the arguments for bench: the workloads, the preset's files, then the arguments after `--`."""
    result = [x for name in args.workload for x in ("-workload", name)]
    explicit = args.bench_args
    if args.preset:
        explicit = [*explicit, *preset_files(args.preset)]
    return result + (explicit or SYNTHETIC)


if __name__ == "__main__":
    main()
