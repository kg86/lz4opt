#!/bin/sh
# Reruns the full comparison of the seven encoders on both corpora, in one
# session per corpus, and writes the summary tables and figures. See
# "Rerunning the full comparison" in README.md. Usage, from any directory:
#
#   experiments/lz4/run_full.sh [OUTPUT_DIR]
#
# OUTPUT_DIR defaults to experiments/lz4/runs/full-YYYYMMDD and must not
# exist. Set SDKROOT if the patched LZ4 cannot be linked (see README.md).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:-$here/runs/full-$(date +%Y%m%d)}
if [ -e "$out" ]; then
  echo "$out already exists" >&2
  exit 1
fi
mkdir -p "$(dirname "$out")"
out=$(cd "$(dirname "$out")" && pwd)/$(basename "$out")
cd "$root"

if [ ! -x "$here/bin/lz4-optnum65536" ]; then
  "$here/build_lz4_wide.sh"
fi
wide="$here/bin/lz4-optnum65536"

echo "== Silesia: 7 encoders, 3 repeats"
python3 "$here/compare.py" --repeats 3 --levels span --wide-lz4 "$wide" --output "$out/silesia"
echo "== Repetitive corpus: 7 encoders, 1 repeat"
python3 "$here/compare.py" --corpus repcorpus --repeats 1 --levels span --wide-lz4 "$wide" --output "$out/repcorpus"

echo "== LZ77 and LZ76 on Silesia"
(cd "$root" && cargo build --release --bin lz77stats)
"$root"/target/release/lz77stats "$here"/data/silesia/* > "$out/lz77stats.jsonl"

echo "== Figures and tables"
python3 "$here/plot_tradeoff.py" "$out/silesia/results.json" --output "$out/silesia-tradeoff.svg" \
  --title "Compression speed and ratio of each file in the Silesia corpus" \
  --xrange 1,2000 --yrange 0.95,10 --yticks 1,1.5,2,3,5,7,10 --width 760
python3 "$here/plot_tradeoff.py" "$out/repcorpus/results.json" --output "$out/repcorpus-tradeoff.svg" \
  --title "Compression speed and ratio of each file in the repetitive corpus" --xrange 1,5000 --width 900 --height 760 \
  --group "Real data=Escherichia_Coli,cere,para,influenza,coreutils,kernel,einstein.de.txt,einstein.en.txt,world_leaders" \
  --group "Pseudo-real data=dblp.xml.00001.1,dblp.xml.00001.2,dblp.xml.0001.1,dblp.xml.0001.2,dna.001.1,english.001.2,proteins.001.1,sources.001.2" \
  --group "Artificial data=fib41,rs.13,tm29" \
  --group "Logs and execution traces=Horspool,NQueens,Quicksort,HDFS_1,BGL,Thunderbird,Windows,Android,EDGAR"
python3 "$here/summary_tables.py" --silesia "$out/silesia/results.json" \
  --repcorpus "$out/repcorpus/results.json" --lz77 "$out/lz77stats.jsonl" > "$out/summary-tables.md"
echo "Done: $out"
