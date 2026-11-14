"""Generate the README banner, architecture diagram, and social preview (light + dark).

Usage: python3 scripts/gen_readme_assets.py [OUT_DIR]   (default: docs/assets)
social-preview.png is exported from social-preview.svg at 1280x640.
"""
import sys
from pathlib import Path

OUT = Path(sys.argv[1] if len(sys.argv) > 1 else "docs/assets")

THEMES = {
    "light": dict(bg="#F7F5F0", ink="#1A1714", muted="#6B645A", faint="#DDD7CB", dim="#AAA294", accent="#C2410C"),
    "dark": dict(bg="#0F0E0C", ink="#EDE8DF", muted="#8F887C", faint="#2C2925", dim="#5C564E", accent="#F0703A"),
}
MONO = "ui-monospace,'SF Mono',SFMono-Regular,Menlo,Consolas,'Liberation Mono',monospace"

GLYPHS = {
    "R": ["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"],
    "e": ["....", "....", ".##.", "#..#", "####", "#...", ".###"],
    "u": ["....", "....", "#..#", "#..#", "#..#", "#..#", ".###"],
}


def style(t):
    return f"""<style>
    .bg{{fill:{t['bg']}}} .ink{{fill:{t['ink']}}} .muted{{fill:{t['muted']}}}
    .dim{{fill:{t['dim']}}} .acc{{fill:{t['accent']}}}
    .rule{{stroke:{t['faint']};stroke-width:1}} .box{{fill:none;stroke:{t['muted']};stroke-width:1.25}}
    .box-acc{{fill:none;stroke:{t['accent']};stroke-width:1.75}}
    .wire{{stroke:{t['muted']};stroke-width:1.25;fill:none}} .wire-acc{{stroke:{t['accent']};stroke-width:1.5;fill:none}}
    text{{font-family:{MONO}}}
  </style>"""


def logotype(x0, y0, m):
    """Pixel-grid 'ReRu' plus an accent cursor. Returns (svg, width)."""
    rects, col = [], 0
    for ch in "ReRu":
        rows = GLYPHS[ch]
        for r, row in enumerate(rows):
            c = 0
            while c < len(row):  # merge horizontal runs into one rect
                if row[c] == "#":
                    start = c
                    while c < len(row) and row[c] == "#":
                        c += 1
                    rects.append((col + start, r, c - start))
                else:
                    c += 1
        col += len(rows[0]) + 1
    # one path, so adjacent blocks rasterize without anti-aliased seams
    d = "".join(f"M{x0 + c * m} {y0 + r * m}h{w * m}v{m}h{-w * m}z" for c, r, w in rects)
    out = [f'<path class="ink" shape-rendering="crispEdges" d="{d}"/>']
    out.append(f'<rect class="acc" x="{x0 + col * m}" y="{y0 + 6 * m}" width="{3 * m}" height="{m}"/>')
    return "\n    ".join(out), (col + 3) * m


def resp(prefix, frames, size):
    """One C:/S: transcript line. frames: list of (type_char, payload) pairs; '' type = bare payload."""
    parts = [f'<tspan class="muted">{prefix}  </tspan>']
    for typ, payload in frames:
        if typ:
            parts.append(f'<tspan class="acc">{typ}</tspan>')
        parts.append(f'<tspan class="ink">{payload}</tspan><tspan class="dim">\\r\\n</tspan>')
    return "".join(parts)


SET = [("*", "3"), ("$", "3"), ("", "SET"), ("$", "5"), ("", "hello"), ("$", "5"), ("", "world")]
OK = [("+", "OK")]
GET = [("*", "2"), ("$", "3"), ("", "GET"), ("$", "5"), ("", "hello")]
BULK = [("$", "5"), ("", "world")]


def svg(w, h, t, title, body, rx=12):
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" aria-label="{title}">
  <title>{title}</title>
  {style(t)}
  <rect class="bg" width="{w}" height="{h}" rx="{rx}"/>
  {body}
</svg>
"""


def frame_lines(w, top, bottom, left_l, mid_l, right_l, left_b, mid_b, right_b, pad, size=12):
    def row(y, l, mid, r):
        return (f'<text class="muted" x="{pad}" y="{y}" font-size="{size}" letter-spacing="1">{l}</text>'
                f'<text class="muted" x="{w / 2}" y="{y}" font-size="{size}" letter-spacing="1" text-anchor="middle">{mid}</text>'
                f'<text class="muted" x="{w - pad}" y="{y}" font-size="{size}" letter-spacing="1" text-anchor="end">{r}</text>')
    return f"""{row(top - 16, left_l, mid_l, right_l)}
  <line class="rule" x1="{pad}" y1="{top}" x2="{w - pad}" y2="{top}"/>
  <line class="rule" x1="{pad}" y1="{bottom}" x2="{w - pad}" y2="{bottom}"/>
  {row(bottom + 26, left_b, mid_b, right_b)}"""


TITLE = "ReRu: a Redis-compatible data server written in Rust"
REPO = "github.com/mansiverma897993/ReRu"


def banner(t):
    W, H, P = 1280, 340, 56
    logo, _ = logotype(P, 100, 12)
    lines = "\n  ".join(
        f'<text x="680" y="{150 + i * 32}" font-size="14.5">{resp(p, f, 14.5)}</text>'
        for i, (p, f) in enumerate([("C:", SET), ("S:", OK), ("C:", GET), ("S:", BULK)]))
    body = f"""{frame_lines(W, 60, 284, "RERU(1)", "Redis-compatible data server", "RERU(1)",
                          "AGPL-3.0-only", "RESP2 over TCP", REPO, P)}
  {logo}
  <text class="ink" x="{P}" y="230" font-size="17">A Redis-compatible data server in safe Rust.</text>
  <text class="muted" x="{P}" y="256" font-size="14">Speaks RESP2. Existing Redis clients connect unchanged.</text>
  <line class="rule" x1="640" y1="88" x2="640" y2="260"/>
  <text class="muted" x="680" y="104" font-size="11" letter-spacing="2">TRANSCRIPT · PORT 6379</text>
  {lines}"""
    return svg(W, H, t, TITLE, body)


def box(x, y, w, h, eyebrow, title, detail, cls="box", tsize=17, dsize=13):
    cx = x + w / 2
    return f"""<rect class="{cls}" x="{x}" y="{y}" width="{w}" height="{h}"/>
  <text class="ink" x="{cx}" y="{y + h / 2 - 2}" font-size="{tsize}" font-weight="600" text-anchor="middle">{title}</text>
  <text class="muted" x="{cx}" y="{y + h / 2 + 20}" font-size="{dsize}" text-anchor="middle">{detail}</text>""" + (
        f'\n  <text class="muted" x="{x + 10}" y="{y + 16}" font-size="10" letter-spacing="1.5">{eyebrow}</text>' if eyebrow else "")


def arrow(x1, y1, x2, y2, cls):
    return f'<line class="{cls}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" marker-end="url(#{"ah-acc" if "acc" in cls else "ah"})"/>'


def architecture(t):
    W, H, P = 1280, 560, 56
    stages = [(128, "01  CONNECT"), (224, "02  PARSE"), (328, "03  EXECUTE"), (442, "04  PERSIST")]
    gutter = "\n  ".join(f'<text class="muted" x="{P}" y="{y}" font-size="12" letter-spacing="1.5">{s}</text>' for y, s in stages)
    body = f"""<defs>
    <marker id="ah" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0 L10 5 L0 10 z" class="muted"/></marker>
    <marker id="ah-acc" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0 L10 5 L0 10 z" class="acc"/></marker>
  </defs>
  {frame_lines(W, 60, 500, "FIG. 1", "REQUEST PATH", "docs/ARCHITECTURE.md", "", "", "", P)}
  {gutter}
  {box(340, 92, 600, 64, "", "Redis clients", "redis-cli · redis-py · ioredis · Lettuce")}
  {arrow(640, 156, 640, 186, "wire")}
  {box(340, 188, 600, 64, "", "RESP2 parser + command dispatch", "MULTI queue · blocking commands · subscriptions")}
  {arrow(640, 252, 640, 282, "wire")}
  {box(340, 284, 600, 80, "", "Shared in-memory store", "command gate · TTL index · eviction · client limits", "box-acc", 19)}
  {arrow(432, 364, 432, 402, "wire-acc")}
  {arrow(640, 364, 640, 402, "wire-acc")}
  {arrow(848, 364, 848, 402, "wire-acc")}
  {arrow(940, 324, 988, 324, "wire-acc")}
  {box(340, 404, 184, 68, "", "Snapshot", ".snap · ReRu format", tsize=15, dsize=12)}
  {box(548, 404, 184, 68, "", "Append-only file", "replay · online rewrite", tsize=15, dsize=12)}
  {box(756, 404, 184, 68, "", "Redis RDB", "subset · SAVE / BGSAVE", tsize=15, dsize=12)}
  {box(990, 292, 234, 64, "", "Replicas", "snapshot + stream", tsize=15, dsize=12)}
  <text class="muted" x="{P}" y="528" font-size="12.5">One thread per connection. The command gate serializes writes; one write-out point feeds every sink.</text>
  <line class="wire-acc" x1="1080" y1="524" x2="1112" y2="524"/>
  <text class="muted" x="{W - P}" y="528" font-size="12.5" text-anchor="end">write path</text>"""
    return svg(W, H, t, "ReRu request path: clients, RESP2 parser and dispatch, shared store, then snapshots, AOF, RDB, and replicas", body)


def social(t):
    W, H, P = 1280, 640, 80
    logo, _ = logotype(P, 150, 22)
    body = f"""{frame_lines(W, 92, 560, "RERU(1)", "Redis-compatible data server", "RERU(1)",
                          "AGPL-3.0-only", "RESP2 over TCP", REPO, P, size=15)}
  {logo}
  <text class="ink" x="{P}" y="378" font-size="28">A Redis-compatible data server in safe Rust.</text>
  <text class="muted" x="{P}" y="418" font-size="20">Speaks RESP2. Existing Redis clients connect unchanged.</text>
  <text x="{P}" y="482" font-size="19">{resp("C:", GET, 19)}</text>
  <text x="{P}" y="518" font-size="19">{resp("S:", BULK, 19)}</text>"""
    return svg(W, H, t, TITLE, body, rx=0)


for name, t in THEMES.items():
    (OUT / f"banner-{name}.svg").write_text(banner(t))
    (OUT / f"architecture-{name}.svg").write_text(architecture(t))
(OUT / "social-preview.svg").write_text(social(THEMES["light"]))
print("ok")
