#!/usr/bin/env python3
"""Plot compression ratio against speed for every file and encoder as SVG.

Reads one or more results.json files written by compare.py; for each
(file, encoder) the first file that has it wins, so a later run can add
encoders to an earlier one. Each point is one file compressed by one
encoder: speed is input MiB divided by the median time, and ratio is input
bytes divided by frame bytes. Both axes are logarithmic. Points of the same
file are joined by a thin line (greedy excepted: it is not one of the
official levels, and joining it would draw a dip that reads as a trade-off),
and file names are listed at the right of each panel, next to the file's
fastest point. --group splits files into panels with their own ratio axes. No third-party packages are required.
"""

import argparse
import json
import math
from xml.sax.saxutils import escape

# Encoder label -> (display name, family, marker), in plotting order. The
# line through a file's points follows this order.
ENCODERS = {
    "official-default": ("LZ4 default", "official", "circle-open"),
    "official-hc-default": ("LZ4HC -9", "official", "triangle"),
    "official-opt-lowest": ("LZ4HC -10", "official", "triangle-down"),
    "official-highest": ("LZ4HC -12", "official", "circle"),
    "official-highest-wide": ("LZ4HC -12w", "official", "star"),
    "optimal": ("LZ4opt", "optimal", "diamond"),
    "greedy": ("greedy", "greedy", "square"),
}
LINE_ORDER = ["official-default", "official-hc-default", "official-opt-lowest",
              "official-highest", "official-highest-wide", "optimal"]
# Legend order: slowest first, like the points from left to right in the plot.
LEGEND_ORDER = ["optimal", "greedy", "official-highest-wide", "official-highest",
                "official-opt-lowest", "official-hc-default", "official-default"]
# Categorical slots 1-3 of the dataviz reference palette (validated, light).
COLORS = {"official": "#2a78d6", "greedy": "#eb6834", "optimal": "#1baf7a"}
TEXT, MUTED, GRID, LINK, SURFACE = "#0b0b0b", "#52514e", "#e4e3df", "#c3c2b7", "#fcfcfb"

LEFT, TOP, BOTTOM = 70, 52, 56
LABEL_W = 150  # right column for file names
FONT = 12
LABEL_GAP = 13


def load(paths):
    records = {}
    for path in paths:
        for r in json.load(open(path))["records"]:
            records.setdefault((r["file"], r["encoder"]), r)
    points = {}
    for (name, encoder), r in records.items():
        if encoder in ENCODERS:
            speed = r["input_bytes"] / 2**20 / r["median_seconds"]
            points.setdefault(name, {})[encoder] = (speed, r["input_bytes"] / r["compressed_bytes"])
    return points


def marker(kind, color, x, y):
    ring = f'stroke="{SURFACE}" stroke-width="1.5"'
    if kind == "circle":
        return f'<circle cx="{x:.1f}" cy="{y:.1f}" r="4.5" fill="{color}" {ring}/>'
    if kind == "circle-open":
        return f'<circle cx="{x:.1f}" cy="{y:.1f}" r="4" fill="{SURFACE}" stroke="{color}" stroke-width="2"/>'
    if kind == "star":
        pts = []
        for k in range(10):
            r = 6.5 if k % 2 == 0 else 2.8
            a = math.pi / 2 + k * math.pi / 5
            pts.append(f"{x + r * math.cos(a):.1f},{y - r * math.sin(a):.1f}")
        return f'<path d="M{" L".join(pts)} Z" fill="{color}" {ring}/>'
    if kind == "square":
        return f'<rect x="{x - 4:.1f}" y="{y - 4:.1f}" width="8" height="8" rx="1" fill="{color}" {ring}/>'
    if kind == "triangle":
        return (f'<path d="M{x:.1f},{y - 5.5:.1f} L{x + 5:.1f},{y + 3.5:.1f} L{x - 5:.1f},{y + 3.5:.1f} Z" '
                f'fill="{color}" {ring}/>')
    if kind == "triangle-down":
        return (f'<path d="M{x:.1f},{y + 5.5:.1f} L{x + 5:.1f},{y - 3.5:.1f} L{x - 5:.1f},{y - 3.5:.1f} Z" '
                f'fill="{color}" {ring}/>')
    return (f'<path d="M{x:.1f},{y - 6:.1f} L{x + 6:.1f},{y:.1f} L{x:.1f},{y + 6:.1f} '
            f'L{x - 6:.1f},{y:.1f} Z" fill="{color}" {ring}/>')


def log_ticks(lo, hi, mults):
    ticks, decade = [], 10 ** math.floor(math.log10(lo))
    while decade <= hi * 1.0001:
        for m in mults:
            v = decade * m
            if lo * 0.9999 <= v <= hi * 1.0001:
                ticks.append((v, m == 1))
        decade *= 10
    return ticks


def spread(items, top, bottom, gap):
    """Moves label positions apart by at least gap, keeping them in [top, bottom]."""
    items = sorted(items, key=lambda t: t[0])
    ys = [y for y, _ in items]
    for i in range(1, len(ys)):
        ys[i] = max(ys[i], ys[i - 1] + gap)
    overflow = ys[-1] - bottom if ys else 0
    if overflow > 0:
        ys[-1] -= overflow
        for i in range(len(ys) - 2, -1, -1):
            ys[i] = min(ys[i], ys[i + 1] - gap)
    ys = [max(y, top) for y in ys]
    return [(y, item) for y, (_, item) in zip(ys, items)]


def auto_range(points):
    ratios = [r for enc in points.values() for _, r in enc.values()]
    lo, hi = min(ratios) / 1.08, max(ratios) * 1.08
    ticks = [v * d for d in (0.1, 1, 10, 100, 1000) for v in (1, 1.5, 2, 3, 5, 7)]
    ticks = [v for v in ticks if lo <= v <= hi]
    if len(ticks) > 7:
        ticks = [v for v in ticks if str(v).rstrip("0").rstrip(".")[0] in "125"]
    return (lo, hi), ticks


def panel(out, points, box, xrange_, yrange, yticks, heading):
    x0, y0, w, h = box
    pw, ph = w - LABEL_W, h
    (xlo, xhi), (ylo, yhi) = xrange_, yrange

    def sx(v):
        return x0 + (math.log10(v) - math.log10(xlo)) / (math.log10(xhi) - math.log10(xlo)) * pw

    def sy(v):
        return y0 + (math.log10(yhi) - math.log10(v)) / (math.log10(yhi) - math.log10(ylo)) * ph

    if heading:
        out.append(f'    <text x="{x0}" y="{y0 - 8}" fill="{TEXT}" font-weight="bold">{escape(heading)}</text>')
    for v, major in log_ticks(xlo, xhi, (1, 2, 5)):
        x = sx(v)
        out.append(f'    <line x1="{x:.1f}" y1="{y0}" x2="{x:.1f}" y2="{y0 + ph}" stroke="{GRID}" '
                   f'stroke-width="{1 if major else 0.6}"/>')
        if major or (xhi / xlo) < 1e3:
            out.append(f'    <text x="{x:.1f}" y="{y0 + ph + 15}" text-anchor="middle" fill="{MUTED}">{v:g}</text>')
    for v in yticks:
        y = sy(v)
        out.append(f'    <line x1="{x0}" y1="{y:.1f}" x2="{x0 + pw}" y2="{y:.1f}" stroke="{GRID}" stroke-width="0.8"/>')
        out.append(f'    <text x="{x0 - 6}" y="{y + 4:.1f}" text-anchor="end" fill="{MUTED}">{v:g}</text>')
    out.append(f'    <rect x="{x0}" y="{y0}" width="{pw}" height="{ph}" fill="none" stroke="{MUTED}" stroke-width="1"/>')

    # Join each file's points in fixed encoder order, then draw points on top.
    for name, enc in points.items():
        coords = [(sx(enc[e][0]), sy(enc[e][1])) for e in LINE_ORDER if e in enc]
        d = " ".join(f"{'M' if i == 0 else 'L'}{x:.1f},{y:.1f}" for i, (x, y) in enumerate(coords))
        out.append(f'    <path d="{d}" fill="none" stroke="{LINK}" stroke-width="1"/>')
    # The wider -12 often coincides with -12, so it is drawn on top.
    order = [e for e in reversed(ENCODERS) if e != "official-highest-wide"] + ["official-highest-wide"]
    for encoder in order:
        _, family, kind = ENCODERS[encoder]
        for name, enc in points.items():
            if encoder in enc:
                out.append("    " + marker(kind, COLORS[family], sx(enc[encoder][0]), sy(enc[encoder][1])))

    # File names in the right column, at the height of the file's fastest point.
    anchors = []
    for name, enc in points.items():
        speed, ratio = max(enc.values())
        anchors.append((sy(ratio), (name, sx(speed), sy(ratio))))
    column = x0 + pw + 14
    for y, (name, px, py) in spread(anchors, y0 + 4, y0 + ph, LABEL_GAP):
        out.append(f'    <path d="M{px + 6:.1f},{py:.1f} L{column - 3:.1f},{y:.1f}" fill="none" '
                   f'stroke="{LINK}" stroke-width="0.8" stroke-dasharray="2,2"/>')
        out.append(f'    <text x="{column:.1f}" y="{y + 4:.1f}" fill="{MUTED}">{escape(name)}</text>')


def render(groups, title, xrange_, yrange, yticks, width, height, columns):
    # Wrap the legend before an item would cross the right margin.
    legend = []
    lx, ly = LEFT, 22
    for encoder in LEGEND_ORDER:
        name = ENCODERS[encoder][0]
        item_width = 30 + sum(FONT if ord(c) > 0x2e80 else FONT * 0.6 for c in name)
        if lx > LEFT and lx + item_width > width - 10:
            lx, ly = LEFT, ly + 24
        legend.append((encoder, lx, ly))
        lx += item_width
    plot_top = TOP + ly - 22
    rows = math.ceil(len(groups) / columns)
    gap_x, gap_y = 60, 64 if len(groups) > 1 else 0
    cell_w = (width - LEFT - 10 - gap_x * (columns - 1)) / columns
    cell_h = (height - plot_top - BOTTOM - gap_y * (rows - 1) - (24 if len(groups) > 1 else 0)) / rows
    files = sum(len(points) for _, points in groups)
    desc = (f"Compression speed (x axis, logarithmic) and compression ratio (y axis, logarithmic) "
            f"of {files} files, each compressed by {len(ENCODERS)} encoders. "
            "For each file, points are joined in the order LZ4 default, LZ4HC -9, -10, -12, "
            "-12w, and LZ4opt.")
    out = ['<?xml version="1.0" encoding="UTF-8"?>',
           f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
           f'viewBox="0 0 {width} {height}" role="img" aria-labelledby="title desc">',
           f'  <title id="title">{escape(title)}</title>',
           f'  <desc id="desc">{escape(desc)}</desc>',
           f'  <rect width="{width}" height="{height}" fill="{SURFACE}"/>',
           f'  <g font-family="sans-serif" font-size="{FONT}">']
    for encoder, lx, ly in legend:
        name, family, kind = ENCODERS[encoder]
        out.append("    " + marker(kind, COLORS[family], lx + 6, ly))
        out.append(f'    <text x="{lx + 16:.1f}" y="{ly + 4}" fill="{TEXT}">{escape(name)}</text>')
    top = plot_top + (24 if len(groups) > 1 else 0)
    for i, (heading, points) in enumerate(groups):
        r, c = divmod(i, columns)
        yr, yt = (yrange, yticks) if yrange else auto_range(points)
        box = (LEFT + c * (cell_w + gap_x), top + r * (cell_h + gap_y), cell_w, cell_h)
        panel(out, points, box, xrange_, yr, yt, heading)
    out.append(f'    <text x="{LEFT + (width - LEFT - LABEL_W) / 2:.1f}" y="{height - 14}" text-anchor="middle" '
               f'fill="{TEXT}" font-size="13">Compression speed (MiB/s, log scale)</text>')
    out.append(f'    <text transform="translate(18,{top + (height - top - BOTTOM) / 2:.1f}) rotate(-90)" '
               f'text-anchor="middle" fill="{TEXT}" font-size="13">Compression ratio (log scale)</text>')
    out += ['  </g>', '</svg>', '']
    return "\n".join(out)


def floats(text):
    return [float(v) for v in text.split(",")]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", nargs="+", help="results.json files, earliest first wins")
    parser.add_argument("--output", required=True)
    parser.add_argument("--title", required=True)
    parser.add_argument("--xrange", type=floats, default=[1, 1000], help="LO,HI in MiB/s")
    parser.add_argument("--yrange", type=floats, help="LO,HI ratio; default: per panel")
    parser.add_argument("--yticks", type=floats)
    parser.add_argument("--group", action="append", default=[],
                        help="HEADING=FILE,FILE,... draws these files in their own panel")
    parser.add_argument("--columns", type=int, default=2)
    parser.add_argument("--width", type=int, default=680)
    parser.add_argument("--height", type=int, default=460)
    args = parser.parse_args()
    points = load(args.results)
    groups = []
    for g in args.group:
        heading, names = g.split("=")
        groups.append((heading, {n: points.pop(n) for n in names.split(",")}))
    if points:
        if groups:
            raise SystemExit(f"files in no group: {sorted(points)}")
        groups = [("", points)]
    points = {n: e for _, ps in groups for n, e in ps.items()}
    speeds = [s for enc in points.values() for s, _ in enc.values()]
    ratios = [r for enc in points.values() for _, r in enc.values()]
    print(f"{len(points)} files; speed {min(speeds):.2f}-{max(speeds):.1f} MiB/s; "
          f"ratio {min(ratios):.3f}-{max(ratios):.1f}")
    with open(args.output, "w") as f:
        f.write(render(groups, args.title, args.xrange, args.yrange, args.yticks, args.width, args.height,
                       args.columns if len(groups) > 1 else 1))


if __name__ == "__main__":
    main()
