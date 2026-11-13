#!/bin/sh
# Builds LZ4 1.10.0 with the optimal parser's window LZ4_OPT_NUM raised from
# 4096 to 65536 into bin/lz4-optnum65536. At level 12 this also raises the
# match length taken without optimization (see README.md). Set SDKROOT if the
# default macOS SDK cannot be linked against.
set -eu
cd "$(dirname "$0")"
tag=v1.10.0
commit=ebb370ca83af193212df4dcbadcc5d87bc0de2f0
src=bin/lz4-src-wide
rm -rf "$src"
git clone -q --depth 1 --branch "$tag" https://github.com/lz4/lz4.git "$src"
test "$(git -C "$src" rev-parse HEAD)" = "$commit"
sed -i.orig 's/^#define LZ4_OPT_NUM   (1<<12)$/#define LZ4_OPT_NUM   (1<<16)/' "$src/lib/lz4hc.c"
grep -q '^#define LZ4_OPT_NUM   (1<<16)$' "$src/lib/lz4hc.c"
make -s -C "$src/programs" lz4 >/dev/null
cp "$src/programs/lz4" bin/lz4-optnum65536
echo "built bin/lz4-optnum65536 from $tag ($commit) with LZ4_OPT_NUM (1<<16)"
