#!/usr/bin/env python3
"""Renders Sleeve headlessly in every state that matters and builds one contact sheet.

    python3 scripts/check.py [OUT_DIR] [--only NAME]

Each shot is a scene, a theme and a window size. Headless frames have no window chrome, so the
traffic lights are drawn in where `main.rs` places them, to check they line up with the
toolbar. Every shot also writes its element tree, and the script reports elements that spill
out of the window and any warnings Rune printed.
"""

import json
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target/debug/sleeve"
SCALE = 2

# Where main.rs puts the traffic lights: the left edge and top of the first light, in points.
LIGHTS = (26.0, 25.0)

SHOTS = [
    # name, scene, theme, size, script
    ("dark", "default", "dark", "1280x800", None),
    ("light", "default", "light", "1280x800", None),
    ("selected-dark", "selected", "dark", "1280x800", None),
    ("selected-light", "selected", "light", "1280x800", None),
    ("busy", "busy", "dark", "1280x800", None),
    ("activity", "activity", "dark", "1280x800", None),
    ("playing", "playing", "dark", "1280x800", None),
    ("playing-narrow", "playing", "light", "900x600", None),
    ("narrow", "selected", "dark", "900x600", None),
    ("glyphs", "glyphs", "dark", "1280x800", None),
    ("collapsed", "collapsed", "dark", "1280x800", None),
    ("log", "log", "dark", "1280x800", None),
    ("log-light", "log", "light", "1280x800", None),
]


def lights(img: Image.Image) -> None:
    d = ImageDraw.Draw(img)
    x, y = LIGHTS
    for i, colour in enumerate([(255, 95, 87), (254, 188, 46), (40, 200, 64)]):
        cx, cy = (x + 7 + i * 20) * SCALE, (y + 7) * SCALE
        r = 6 * SCALE
        d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=colour)


# Elements that clip their children on purpose, so what's inside may run past their edges.
CLIPPED = ("/deck-zoom/",)


def spills(tree: dict, w: float, h: float) -> list[str]:
    out = []
    deck = [e["rect"] for e in tree.get("elements", []) if e["path"].endswith("/deck")]
    deck_right = deck[0][0] + deck[0][2] if deck else None
    for e in tree.get("elements", []):
        x, y, ew, eh = e["rect"]
        if ew <= 0 or eh <= 0 or e.get("opacity", 1) == 0 or any(c in e["path"] for c in CLIPPED):
            continue
        # Content below the fold is a scroll view's business; sideways spills never are.
        if x < -0.5 or x + ew > w + 0.5 or y < -0.5:
            out.append(f"{e['path']} {[round(v) for v in e['rect']]}")
        # Nothing on the deck may be cut off by the panel's edge.
        if "/deck/" in e["path"] and deck_right is not None and x + ew > deck_right + 0.5:
            out.append(f"{e['path']} runs past the deck {[round(v) for v in e['rect']]}")
        # The footer and toolbar must always be on screen, whole.
        if e["path"].endswith(("/statusbar", "/toolbar")) and (y + eh > h + 0.5 or y < -0.5):
            out.append(f"{e['path']} is off screen {[round(v) for v in e['rect']]}")
    return out


def main() -> None:
    only = sys.argv[sys.argv.index("--only") + 1] if "--only" in sys.argv else None
    # Everything else that isn't a flag is the output folder. The value after --only isn't.
    args = [a for i, a in enumerate(sys.argv[1:], 1) if not a.startswith("--") and sys.argv[i - 1] != "--only"]
    out = Path(args[0]) if args else ROOT / "target/check"
    out.mkdir(parents=True, exist_ok=True)
    subprocess.run(["cargo", "build", "-q"], cwd=ROOT, check=True)
    shots = []
    problems = 0
    for name, scene, theme, size, script in SHOTS:
        if only and only != name:
            continue
        png, tree = out / f"{name}.png", out / f"{name}.json"
        cmd = [str(BIN), "--snap", str(png), "--tree", str(tree), "--theme", theme, "--size", size, "--scale", str(SCALE), "--at", "0.8"]
        if scene != "default":
            cmd += ["--scene", scene]
        if script:
            cmd += ["--script", script]
        r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
        noise = [l for l in r.stderr.splitlines() if l and not l.startswith("rune: ") or "warn" in l.lower()]
        if r.returncode != 0:
            print(f"{name}: exited {r.returncode}\n{r.stderr}")
            problems += 1
            continue
        img = Image.open(png).convert("RGB")
        lights(img)
        img.save(png)
        w, h = (float(v) for v in size.split("x"))
        t = json.loads(tree.read_text())
        found = spills(t, w, h) + [f"warning: {x}" for x in t.get("warnings", [])] + noise
        for f in found:
            print(f"{name}: {f}")
        problems += len(found)
        shots.append((name, img))
    if shots:
        thumb_w = 1280
        rows = []
        for name, img in shots:
            t = img.resize((thumb_w, int(img.height * thumb_w / img.width)))
            label = Image.new("RGB", (thumb_w, 36), (30, 30, 30))
            ImageDraw.Draw(label).text((12, 8), name, fill=(230, 230, 230), font=ImageFont.load_default(20))
            rows += [label, t]
        sheet = Image.new("RGB", (thumb_w, sum(r.height for r in rows)), (0, 0, 0))
        y = 0
        for r in rows:
            sheet.paste(r, (0, y))
            y += r.height
        sheet.save(out / "sheet.png")
    print(f"{len(shots)} shots, {problems} problems -> {out}")


if __name__ == "__main__":
    main()
