"""Regression tests for incomplete or misleading benchmark comparisons."""
import contextlib
import importlib.util
import io
from pathlib import Path
from types import SimpleNamespace
import unittest

spec = importlib.util.spec_from_file_location("bench_compare", Path(__file__).with_name("bench-compare.py"))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


class ComparisonTests(unittest.TestCase):
    def samples(self):
        return {label: {("lz77", "input"): {"seconds": [1.0, 2.0], "hashes": {"abc"}, "bytes": {12}}}
                for label in ("a", "b")}

    def test_missing_measurements_on_either_side(self):
        for label in ("a", "b"):
            samples = self.samples()
            samples[label][("extra", "input")] = samples[label][("lz77", "input")]
            with self.assertRaises(ValueError):
                bench.validate_samples(samples, 2)

    def test_empty_measurements(self):
        with self.assertRaises(ValueError):
            bench.validate_samples({"a": {}, "b": {}}, 2)

    def test_missing_round(self):
        samples = self.samples()
        samples["b"][("lz77", "input")]["seconds"].pop()
        with self.assertRaises(ValueError):
            bench.validate_samples(samples, 2)

    def test_output_size_and_hash_must_both_be_stable_and_equal(self):
        for field, value in [("bytes", {13}), ("bytes", {12, 13}), ("hashes", {"xyz"}), ("hashes", {"abc", "xyz"})]:
            samples = self.samples()
            samples["b"][("lz77", "input")][field] = value
            self.assertFalse(self.report(samples)["results"][0]["same_output"])
        self.assertTrue(self.report(self.samples())["results"][0]["same_output"])

    def test_size_columns(self):
        samples = self.samples()
        samples["b"][("lz77", "input")]["bytes"] = {6}
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.report(samples, quiet=False)
        self.assertIn("| 12 | 6 | 0.5000 | DIFFERENT |", out.getvalue())

    def test_verdict_and_geomean(self):
        self.assertEqual(bench.verdict([1.0, 1.01], [0.5, 0.505]), "B faster")
        self.assertEqual(bench.verdict([0.5, 0.505], [1.0, 1.01]), "B slower")
        self.assertEqual(bench.verdict([1.0, 1.3], [0.9, 1.2]), "same speed")
        self.assertAlmostEqual(bench.geomean([2.0, 0.5]), 1.0)

    def test_peak_rss_line(self):
        a, b = SimpleNamespace(peak_rss=[2 * 2**20, 3 * 2**20]), SimpleNamespace(peak_rss=[1 * 2**20])
        self.assertEqual(bench.peak_rss_line(a, b),
                         "Peak RSS of bench (minimum over rounds): A 2.0 MiB, B 1.0 MiB, B / A 0.500")
        self.assertIsNone(bench.peak_rss_line(SimpleNamespace(), b))

    def report(self, samples, quiet=True):
        args = SimpleNamespace(rev_a="HEAD", rev_b="WORKTREE", rounds=2, bench_args=[])
        with contextlib.redirect_stdout(io.StringIO()) if quiet else contextlib.nullcontext():
            return bench.report(args, [SimpleNamespace(commit="a", peak_rss=[1]), SimpleNamespace(commit="b", peak_rss=[1])], samples)


if __name__ == "__main__":
    unittest.main()
