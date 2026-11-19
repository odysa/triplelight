#!/usr/bin/env python3
"""Draw the README benchmark charts as static SVG (light and dark variants). No dependencies.

The values are copied from the published evidence, not re-measured:
  BI 100M:     docs/matched-bi100-v1/round1/template-summary.csv (sum of sum_ms per phase) and
               round1/<phase>/cgroup-memory.json (memory.peak) at commit 13123a2.
  aggregate72: docs/aggregate72-matched-results-v1/round1/verified-summary.json at commit 87fa9e2b
               (labeled_total_seconds summed over LUBM + WatDiv; max container_cgroup_peak_bytes per pass).
Run: python3 bench/charts.py  (writes docs/images/*.svg)
"""
import math, os

GIB = 2**30
WORKLOADS = {
    "bi100m": {
        "title": "BSBM Business Intelligence, 98.9M triples",
        "sub": "45 scored requests per pass, 4 CPUs and 12 GiB per engine, same VM and session",
        "engines": [  # name, (pass1 s, pass2 s), (pass1 bytes, pass2 bytes)
            ("TripleLight", (11.744, 6.731), (3960119296, 4377493504)),
            ("QLever", (24.176, 24.506), (548614144, 422412288)),
            ("DuckDB 2.0", (573.759, 443.534), (12194824192, 12880408576)),
        ],
    },
    "aggregate72": {
        "title": "72 aggregate queries, LUBM-100 (13.4M triples) + WatDiv (10.9M triples)",
        "sub": "72 scored queries per pass, 4 CPUs and 12 GiB per engine, same VM and session",
        "engines": [
            ("TripleLight", (0.5753, 0.5383), (1045774336, 1013239808)),
            ("Virtuoso 7", (4.6105, 4.4997), (11139768320, 11131449344)),
            ("QLever", (5.1056, 4.6043), (312086528, 311226368)),
            ("DuckDB 2.0", (73.1489, 73.5915), (3602198528, 3604578304)),
        ],
    },
}
THEMES = {  # reference palette: categorical slots 1-2, validated light and dark
    "light": dict(bg="#fcfcfb", ink="#0b0b0b", ink2="#52514e", grid="#e4e3df", s1="#2a78d6", s2="#eb6834"),
    "dark": dict(bg="#1a1a19", ink="#f0efec", ink2="#c3c2b7", grid="#383835", s1="#3987e5", s2="#d95926"),
}
FONT = "-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif"

def fmt_s(v): return f"{v:.2f} s" if v < 100 else f"{v:.1f} s"
def fmt_g(b): return f"{b / GIB:.2f} GiB" if b < GIB else f"{b / GIB:.1f} GiB"

def svg(w, t):
    eng = w["engines"]
    W, row, top = 960, 46, 108
    H = top + row * len(eng) + 56
    lab_w, pw, gap = 112, 330, 60
    x1 = lab_w; x2 = x1 + pw + gap + 70  # memory panel left edge (time values column sits between)
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" font-family="{FONT}">',
           f'<rect width="{W}" height="{H}" fill="{t["bg"]}"/>',
           f'<text x="16" y="28" font-size="17" font-weight="600" fill="{t["ink"]}">{w["title"]}</text>',
           f'<text x="16" y="48" font-size="12.5" fill="{t["ink2"]}">{w["sub"]}. Lower is better in both panels.</text>']
    # legend
    lx = 16
    for i, name in enumerate(("Pass 1", "Pass 2 (reversed engine order)")):
        c = t["s1"] if i == 0 else t["s2"]
        out.append(f'<rect x="{lx}" y="60" width="12" height="12" rx="3" fill="{c}"/>')
        out.append(f'<text x="{lx + 18}" y="70.5" font-size="12.5" fill="{t["ink2"]}">{name}</text>')
        lx += 80 if i == 0 else 0
    out.append(f'<text x="{x1}" y="{top - 12}" font-size="12.5" font-weight="600" fill="{t["ink"]}">Total query time, seconds (log scale)</text>')
    out.append(f'<text x="{x2}" y="{top - 12}" font-size="12.5" font-weight="600" fill="{t["ink"]}">Peak container memory, GiB (cgroup memory.peak)</text>')
    # time panel: log axis
    vals = [v for _, p, _ in eng for v in p]
    lo = 10 ** math.floor(math.log10(min(vals))); hi = 10 ** math.ceil(math.log10(max(vals)))
    def tx(v): return x1 + pw * (math.log10(v) - math.log10(lo)) / (math.log10(hi) - math.log10(lo))
    # memory panel: linear 0..12 GiB (the per-engine cap)
    mmax = 12 * GIB; mw = W - x2 - 90
    def mx(b): return x2 + mw * b / mmax
    yb = top + row * len(eng)
    d = lo
    while d <= hi * 1.0001:
        x = tx(d); out.append(f'<line x1="{x:.1f}" y1="{top - 4}" x2="{x:.1f}" y2="{yb}" stroke="{t["grid"]}"/>')
        lbl = f"{d:g}"
        out.append(f'<text x="{x:.1f}" y="{yb + 16}" font-size="11.5" text-anchor="middle" fill="{t["ink2"]}">{lbl}</text>')
        d *= 10
    for g in range(0, 13, 4):
        x = mx(g * GIB); out.append(f'<line x1="{x:.1f}" y1="{top - 4}" x2="{x:.1f}" y2="{yb}" stroke="{t["grid"]}"/>')
        out.append(f'<text x="{x:.1f}" y="{yb + 16}" font-size="11.5" text-anchor="middle" fill="{t["ink2"]}">{g}</text>')
    for r, (name, secs, mem) in enumerate(eng):
        y = top + r * row
        weight = "600" if r == 0 else "400"
        out.append(f'<text x="16" y="{y + row / 2 + 4}" font-size="13" font-weight="{weight}" fill="{t["ink"]}">{name}</text>')
        for i, (s, b) in enumerate(zip(secs, mem)):
            c = t["s1"] if i == 0 else t["s2"]
            by = y + 10 + i * 14
            # time: dot on the log axis (bar length is not meaningful on a log scale)
            out.append(f'<line x1="{x1}" y1="{by + 5}" x2="{tx(s):.1f}" y2="{by + 5}" stroke="{c}" stroke-width="2" stroke-opacity="0.35"/>')
            out.append(f'<circle cx="{tx(s):.1f}" cy="{by + 5}" r="5" fill="{c}" stroke="{t["bg"]}" stroke-width="2"><title>{name}, pass {i + 1}: {fmt_s(s)}</title></circle>')
            out.append(f'<text x="{x1 + pw + 14}" y="{by + 9.5}" font-size="12" fill="{t["ink"]}">{fmt_s(s)}</text>')
            # memory: bar from zero
            bw = max(mx(b) - x2, 2)
            out.append(f'<rect x="{x2}" y="{by}" width="{bw:.1f}" height="11" rx="3" fill="{c}"><title>{name}, pass {i + 1}: {fmt_g(b)}</title></rect>')
            out.append(f'<text x="{x2 + bw + 6:.1f}" y="{by + 9.5}" font-size="12" fill="{t["ink"]}">{fmt_g(b)}</text>')
    out.append(f'<text x="16" y="{H - 12}" font-size="11.5" fill="{t["ink2"]}">Time is end-to-end HTTP, summed over the scored requests of one pass. Memory includes page cache and kernel memory, not process RSS.</text>')
    out.append("</svg>")
    return "\n".join(out) + "\n"

if __name__ == "__main__":
    d = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "docs", "images")
    os.makedirs(d, exist_ok=True)
    for key, w in WORKLOADS.items():
        for theme, t in THEMES.items():
            with open(os.path.join(d, f"bench-{key}-{theme}.svg"), "w") as f: f.write(svg(w, t))
