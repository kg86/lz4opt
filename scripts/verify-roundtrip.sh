#!/usr/bin/env bash
# Verifies that lz4frame-json can round-trip a Frame produced by the
# official lz4 CLI: compress with lz4, inspect to JSON, restore back to
# .lz4, and confirm the restored bytes are identical to the original
# compressed bytes and that both decompress to the original fixture.
set -eu

usage() {
	cat <<EOF
Usage: $(basename "${BASH_SOURCE[0]}") FILE

Verify that lz4frame-json can round-trip a Frame produced by the
official lz4 CLI for FILE.

Arguments:
  FILE  Path to the file to compress and round-trip.

Options:
  -h, --help  Show this help message and exit.
EOF
}

case "${1:-}" in
-h | --help)
	usage
	exit 0
	;;
"")
	usage >&2
	exit 1
	;;
esac

for cmd in cargo lz4 cmp; do
	if ! command -v "$cmd" >/dev/null 2>&1; then
		echo "verify-roundtrip: required command not found: $cmd" >&2
		exit 1
	fi
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sample="$1"

if [ ! -f "$sample" ]; then
	echo "verify-roundtrip: file not found: $sample" >&2
	exit 1
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

(cd "$repo_root" && cargo build --release --quiet --bin lz4frame-json)
bin="$repo_root/target/release/lz4frame-json"

original_lz4="$tmp_dir/original.lz4"
frame_json="$tmp_dir/frame.json"
restored_lz4="$tmp_dir/restored.lz4"
original_decompressed="$tmp_dir/original.decompressed"
restored_decompressed="$tmp_dir/restored.decompressed"

lz4 -q -f -k "$sample" "$original_lz4"

"$bin" inspect "$original_lz4" >"$frame_json"
"$bin" restore "$frame_json" "$restored_lz4"

cmp "$original_lz4" "$restored_lz4"

lz4 -q -d -f -k "$original_lz4" "$original_decompressed"
lz4 -q -d -f -k "$restored_lz4" "$restored_decompressed"

cmp "$sample" "$original_decompressed"
cmp "$sample" "$restored_decompressed"

echo "verify-roundtrip: OK"
