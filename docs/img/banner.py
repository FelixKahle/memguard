"""Writes banner-light.svg and banner-dark.svg next to this file from one template, so the two cannot drift.

Run it after changing the banner: python3 docs/img/banner.py
"""
import os

THEMES = {
    "light": dict(card="#f6f8fa", border="#d0d7de", text="#1f2328", muted="#59636e", grid="#d8dee4",
                  safe="#1a7f37", limit="#9a6700", kill="#cf222e", pill="#ffffff"),
    "dark": dict(card="#161b22", border="#30363d", text="#e6edf3", muted="#9198a1", grid="#262c36",
                 safe="#3fb950", limit="#d29922", kill="#f85149", pill="#0d1117"),
}

# The tree's resident memory over time: climbing, then crossing the limit at x = 1060.
CURVE = [(780, 252), (812, 248), (844, 241), (876, 236), (908, 226), (940, 214), (972, 197), (1004, 176),
         (1032, 150), (1060, 112)]
GHOST = [(1060, 112), (1084, 78), (1104, 40)]
LIMIT_Y, BASE_Y, LEFT, RIGHT = 112, 262, 780, 1216

def path(points):
    return "M" + " L".join(f"{x},{y}" for x, y in points)

def svg(c):
    area = path(CURVE) + f" L1060,{BASE_Y} L{LEFT},{BASE_Y} Z"
    mono = "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, 'Liberation Mono', monospace"
    sans = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif"
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="320" viewBox="0 0 1280 320" role="img" aria-labelledby="title desc">
  <title id="title">memguard</title>
  <desc id="desc">A process tree's memory climbs until it crosses a dashed limit line, where memguard kills the whole tree with SIGKILL and the memory drops to zero.</desc>
  <rect x="0.5" y="0.5" width="1279" height="319" rx="16" fill="{c["card"]}" stroke="{c["border"]}"/>

  <text x="64" y="146" font-family="{mono}" font-size="76" font-weight="700" fill="{c["text"]}" letter-spacing="-2">mem<tspan fill="{c["safe"]}">guard</tspan></text>
  <text x="66" y="194" font-family="{sans}" font-size="23" fill="{c["muted"]}">Kills a runaway process tree before it swaps your machine.</text>
  <rect x="64" y="222" width="404" height="44" rx="8" fill="{c["pill"]}" stroke="{c["border"]}"/>
  <text x="84" y="250" font-family="{mono}" font-size="18" fill="{c["muted"]}">$ <tspan fill="{c["text"]}">memguard run 6G cargo test</tspan></text>

  <g font-family="{mono}" font-size="14">
    <line x1="{LEFT}" y1="{BASE_Y}" x2="{RIGHT}" y2="{BASE_Y}" stroke="{c["grid"]}" stroke-width="2"/>
    <line x1="{LEFT}" y1="{LIMIT_Y}" x2="{RIGHT}" y2="{LIMIT_Y}" stroke="{c["limit"]}" stroke-width="2" stroke-dasharray="8 6"/>
    <text x="{RIGHT}" y="{LIMIT_Y - 12}" text-anchor="end" fill="{c["limit"]}">limit 6G</text>
    <text x="{LEFT}" y="{BASE_Y + 26}" fill="{c["muted"]}">resident memory of the whole tree</text>
    <text x="{RIGHT}" y="{BASE_Y + 26}" text-anchor="end" fill="{c["muted"]}">time →</text>

    <path d="{area}" fill="{c["safe"]}" fill-opacity="0.12"/>
    <path d="{path(GHOST)}" fill="none" stroke="{c["muted"]}" stroke-width="2" stroke-dasharray="2 6" stroke-linecap="round" opacity="0.6"/>
    <path d="{path(CURVE)}" fill="none" stroke="{c["safe"]}" stroke-width="4" stroke-linejoin="round" stroke-linecap="round"/>
    <path d="M1060,{LIMIT_Y} L1060,{BASE_Y - 1} L{RIGHT},{BASE_Y - 1}" fill="none" stroke="{c["kill"]}" stroke-width="3" stroke-dasharray="6 5"/>
    <circle cx="1060" cy="{LIMIT_Y}" r="9" fill="{c["card"]}" stroke="{c["kill"]}" stroke-width="4"/>
    <text x="1078" y="{LIMIT_Y + 42}" font-weight="700" fill="{c["kill"]}">SIGKILL</text>
    <text x="1078" y="{LIMIT_Y + 62}" fill="{c["muted"]}">every process</text>
    <text x="1078" y="{LIMIT_Y + 80}" fill="{c["muted"]}">of the tree</text>
  </g>
</svg>
'''

out = os.path.dirname(os.path.abspath(__file__))
for name, colours in THEMES.items():
    with open(f"{out}/banner-{name}.svg", "w") as f:
        f.write(svg(colours))
