#!/usr/bin/env python3
"""Render gitty's `cells:Y` output (one block per screen row) as an SVG terminal window.

Usage: term2svg.py CELLS.txt OUT.svg [TITLE]
"""
import html
import re
import sys

CW, LH, FS = 8.4, 18, 14  # cell width, line height, font size (px)
PAD, BAR = 16, 30  # padding around the screen, title bar height
DEFAULT_FG, DEFAULT_BG = "#c5cbd3", "#1f2329"

COL = r"(Rgb\(\d+, \d+, \d+\)|Indexed\(\d+\)|\w+)"
RUN = re.compile(r"^\s*(\d+)-(\d+)\s+fg=" + COL + r" bg=" + COL + r" (.+?) (\".*\")$")


def color(c, default):
    m = re.match(r"Rgb\((\d+), (\d+), (\d+)\)", c)
    if m:
        return "#%02x%02x%02x" % tuple(int(v) for v in m.groups())
    named = {"Black": "#000000", "White": "#ffffff", "Gray": "#a0a0a0", "DarkGray": "#606060"}
    return named.get(c, default)


def unescape(s):
    s = s[1:-1]
    out, i = [], 0
    while i < len(s):
        ch = s[i]
        if ch == "\\":
            nxt = s[i + 1]
            if nxt == "u":
                j = s.index("}", i)
                out.append(chr(int(s[i + 3 : j], 16)))
                i = j + 1
                continue
            out.append({"n": "\n", "t": "\t", "\\": "\\", '"': '"', "'": "'", "0": "\0"}.get(nxt, nxt))
            i += 2
            continue
        out.append(ch)
        i += 1
    return "".join(out)


def parse(path):
    rows, cur = [], []
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if not line.strip():
            if cur:
                rows.append(cur)
                cur = []
            continue
        m = RUN.match(line)
        if not m:
            continue
        a, b, fg, bg, mods, text = m.groups()
        cur.append((int(a), int(b), fg, bg, mods, unescape(text)))
    if cur:
        rows.append(cur)
    return rows


def render(rows, title):
    width = max(r[-1][1] + 1 for r in rows)
    w, h = width * CW + 2 * PAD, len(rows) * LH + 2 * PAD + BAR
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0f}" height="{h:.0f}" viewBox="0 0 {w:.0f} {h:.0f}">',
        "<style>text{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,'DejaVu Sans Mono',monospace;"
        f"font-size:{FS}px;white-space:pre;dominant-baseline:central}}</style>",
        f'<rect width="{w:.0f}" height="{h:.0f}" rx="10" fill="{DEFAULT_BG}"/>',
        f'<rect width="{w:.0f}" height="{BAR}" rx="10" fill="#2b3038"/><rect y="{BAR - 10}" width="{w:.0f}" height="10" fill="#2b3038"/>',
    ]
    for i, c in enumerate(("#ff5f57", "#febc2e", "#28c840")):
        out.append(f'<circle cx="{18 + i * 20}" cy="{BAR / 2}" r="6" fill="{c}"/>')
    out.append(f'<text x="{w / 2:.0f}" y="{BAR / 2}" fill="#9aa3ad" text-anchor="middle">{html.escape(title)}</text>')
    y0 = BAR + PAD
    for y, runs in enumerate(rows):
        ty = y0 + y * LH
        for a, b, fg, bg, mods, text in runs:
            f, g = color(fg, DEFAULT_FG), color(bg, DEFAULT_BG)
            if "REVERSED" in mods:
                f, g = g, f
            x, cells = PAD + a * CW, b - a + 1
            if g != DEFAULT_BG:
                out.append(f'<rect x="{x:.1f}" y="{ty}" width="{cells * CW:.1f}" height="{LH}" fill="{g}" shape-rendering="crispEdges"/>')
            if not text.strip():
                continue
            style = []
            if "BOLD" in mods:
                style.append('font-weight="bold"')
            if "ITALIC" in mods:
                style.append('font-style="italic"')
            if "UNDERLINED" in mods:
                style.append('text-decoration="underline"')
            if "DIM" in mods:
                style.append('opacity="0.6"')
            # One <text> per cell keeps box drawing and wide glyphs on the grid.
            col = a
            for ch in text:
                if ch != " ":
                    out.append(
                        f'<text x="{PAD + col * CW + CW / 2:.1f}" y="{ty + LH / 2}" fill="{f}" '
                        f'text-anchor="middle" {" ".join(style)}>{html.escape(ch)}</text>'
                    )
                col += 2 if ord(ch) >= 0x1100 and is_wide(ch) else 1
    out.append("</svg>")
    return "\n".join(out)


def is_wide(ch):
    import unicodedata

    return unicodedata.east_asian_width(ch) in ("W", "F")


if __name__ == "__main__":
    rows = parse(sys.argv[1])
    title = sys.argv[3] if len(sys.argv) > 3 else "gitty"
    open(sys.argv[2], "w", encoding="utf-8").write(render(rows, title))
