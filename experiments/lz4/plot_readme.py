#!/usr/bin/env python3
"""Generate the README comparison SVG from a run_full.sh output directory.

Usage: python3 experiments/lz4/plot_readme.py RUN_DIR --output docs/images/results.svg
"""

import argparse
import json
from pathlib import Path
from xml.sax.saxutils import escape

BLUE = '#2563eb'
GREEN = '#087f5b'
GRAY = '#94a3b8'
INK = '#172033'
MUTED = '#526078'
GRID = '#dce3ed'


def records(path):
    with path.open() as f:
        return json.load(f)['records']


def compression_ratio(rows, encoder):
    selected = [r for r in rows if r['encoder'] == encoder]
    return sum(r['input_bytes'] for r in selected) / sum(r['compressed_bytes'] for r in selected)


def render(silesia, repetitive):
    out = [
        '<svg xmlns="http://www.w3.org/2000/svg" width="900" height="985" viewBox="0 0 900 985" role="img" aria-labelledby="title desc">',
        '<title id="title">lz4opt, LZ4HC -12 and LZ4 default: compression ratios</title>',
        '<desc id="desc">Grouped bars compare original size divided by compressed size; higher is better. The top panel shows ratios of corpus byte totals, and the bottom panel shows six selected repetitive files. Both axes start at zero, with different scales for the two panels. Labels show ratios to five decimal places.</desc>',
        '<rect width="900" height="985" rx="12" fill="#ffffff"/>',
        '<g font-family="Arial, Helvetica, sans-serif">',
    ]

    def text(x, y, value, size=14, color=INK, anchor='start', bold=False):
        weight = ' font-weight="700"' if bold else ''
        out.append(f'<text x="{x}" y="{y}" font-size="{size}" fill="{color}" text-anchor="{anchor}"{weight}>{escape(str(value))}</text>')

    def rect(x, y, width, height, color):
        out.append(f'<rect x="{x:.2f}" y="{y}" width="{width:.2f}" height="{height}" rx="3" fill="{color}"/>')

    def axis(y0, y1, maximum, ticks):
        for tick in ticks:
            x = 180 + tick / maximum * 610
            out.append(f'<path d="M{x:.2f} {y0}V{y1}" stroke="{GRID}"/>')
            text(x, y1 + 22, f'{tick:g}', 12, MUTED, 'middle')

    text(28, 35, 'Compression ratio', 22, bold=True)
    text(28, 64, 'Original size / compressed size · higher is better', color=MUTED)
    text(28, 87, 'Corpus totals · total input bytes / total compressed bytes', 13, MUTED)
    rect(360, 23, 14, 14, GRAY)
    text(382, 36, 'LZ4 default')
    rect(520, 23, 14, 14, BLUE)
    text(542, 36, 'LZ4HC -12')
    rect(675, 23, 14, 14, GREEN)
    text(697, 36, 'lz4opt')
    axis(105, 280, 7, [0, 1, 2, 3, 4, 5, 6, 7])
    for name, rows, y in [('Silesia', silesia, 116), ('Repetitive', repetitive, 200)]:
        text(160, y + 36, name, anchor='end', bold=True)
        for encoder, color, offset in [('official-default', GRAY, 0), ('official-highest', BLUE, 26), ('optimal', GREEN, 52)]:
            value = compression_ratio(rows, encoder)
            width = value / 7 * 610
            rect(180, y + offset, width, 21, color)
            text(190 + width, y + offset + 16, f'{value:.5f}', bold=True)

    text(28, 346, 'Selected repetitive files', 22, bold=True)
    text(28, 373, 'Same ratio · larger axis range · selected for larger relative improvements', color=MUTED)
    axis(396, 875, 300, [0, 50, 100, 150, 200, 250, 300])
    for i, name in enumerate(['Horspool', 'Quicksort', 'NQueens', 'tm29', 'fib41', 'rs.13']):
        rows = [r for r in repetitive if r['file'] == name]
        y = 407 + i * 78
        text(160, y + 36, name, anchor='end')
        for encoder, color, offset in [('official-default', GRAY, 0), ('official-highest', BLUE, 25), ('optimal', GREEN, 50)]:
            value = compression_ratio(rows, encoder)
            width = value / 300 * 610
            rect(180, y + offset, width, 21, color)
            text(190 + width, y + offset + 16, f'{value:.5f}', bold=True)
    text(28, 932, 'Bars start at zero · five decimal places retain small differences', 13, MUTED)
    text(28, 959, 'Apple M4 · LZ4 1.10.0 · one thread · independent 4 MiB blocks', 13, MUTED)
    out.extend(['</g>', '</svg>', ''])
    return '\n'.join(out)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run_dir', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    svg = render(records(args.run_dir / 'silesia/results.json'), records(args.run_dir / 'repcorpus/results.json'))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(svg)


if __name__ == '__main__':
    main()
