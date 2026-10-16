#!/usr/bin/env python3
"""pixel_diff.py — compare our pages with LibreOffice's pixel by pixel.

Usage: pixel_diff.py OUT_DIR [--app letters] [--tier A] [--tol 64] [--overlay]

For every fixture under OUT_DIR/<app>/<name>/ with LibreOffice pages
(lo-N.png) and ours (<tier>-N.png), each page is compared at LibreOffice's
size: ours is resampled to it, both go to grey, and a pixel differs when
the two greys are more than --tol levels apart. Two numbers per page:

  ink_mismatch   differing pixels / pixels inked in either render — the
                 parity figure: 0.0 is pixel-identical text and rules,
                 1.0 is nothing in common. Blank paper cannot dilute it.
  page_mismatch  differing pixels / all pixels on the page.

No OCR, no alignment search, no per-metric budgets: the number says how
much of what either renderer drew the other did not draw in the same
place. With --overlay each page gets pixel-N.png beside it: black where
both have ink, red where only LibreOffice does, blue where only we do.

Prints one line per fixture (its worst page) sorted worst first, and
writes OUT_DIR/pixel-<app>-<tier>.json.
"""

import argparse
import glob
import json
import os
import re
import sys

import numpy as np
from PIL import Image

INK = 200  # grey at or below this is ink, above it paper


def page_no(path):
    return int(re.search(r"-(\d+)\.png$", path).group(1))


def grey(path, size=None):
    im = Image.open(path).convert("L")
    if size is not None and im.size != size:
        im = im.resize(size, Image.LANCZOS)
    return np.asarray(im, dtype=np.int16)


def compare(lo_path, ours_path, tol, overlay_path=None):
    lo = grey(lo_path)
    ours = grey(ours_path, (lo.shape[1], lo.shape[0]))
    differs = np.abs(lo - ours) > tol
    inked = (lo <= INK) | (ours <= INK)
    n_ink = int(inked.sum())
    result = {
        "ink_mismatch": float((differs & inked).sum() / n_ink) if n_ink else 0.0,
        "page_mismatch": float(differs.mean()),
        "ink_pixels": n_ink,
    }
    if overlay_path:
        rgb = np.full(lo.shape + (3,), 255, dtype=np.uint8)
        lo_ink, our_ink = lo <= INK, ours <= INK
        rgb[lo_ink & our_ink] = (0, 0, 0)
        rgb[lo_ink & ~our_ink] = (220, 30, 30)
        rgb[our_ink & ~lo_ink] = (30, 80, 220)
        Image.fromarray(rgb).save(overlay_path)
    return result


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("out")
    ap.add_argument("--app", default="letters")
    ap.add_argument("--tier", default="A")
    ap.add_argument("--tol", type=int, default=64)
    ap.add_argument("--overlay", action="store_true")
    a = ap.parse_args(argv)

    report = {}
    for d in sorted(glob.glob(os.path.join(a.out, a.app, "*"))):
        lo_pages = {page_no(p): p for p in glob.glob(os.path.join(d, "lo-[0-9]*.png"))}
        ours = {page_no(p): p for p in glob.glob(os.path.join(d, f"{a.tier}-[0-9]*.png"))}
        if not lo_pages or not ours:
            continue
        pages = {}
        for n in sorted(lo_pages):
            if n not in ours:
                pages[n] = {"ink_mismatch": 1.0, "page_mismatch": None, "missing": True}
                continue
            overlay = os.path.join(d, f"pixel-{n}.png") if a.overlay else None
            pages[n] = compare(lo_pages[n], ours[n], a.tol, overlay)
        extra = sorted(set(ours) - set(lo_pages))
        worst = max(p["ink_mismatch"] for p in pages.values())
        report[f"{a.app}/{os.path.basename(d)}"] = {
            "worst_ink_mismatch": worst,
            "mean_ink_mismatch": float(np.mean([p["ink_mismatch"] for p in pages.values()])),
            "pages_lo": len(lo_pages),
            "pages_ours": len(ours),
            "extra_pages": extra,
            "pages": pages,
        }

    with open(os.path.join(a.out, f"pixel-{a.app}-{a.tier}.json"), "w") as f:
        json.dump(report, f, indent=1, sort_keys=True)
    for name, r in sorted(report.items(), key=lambda kv: -kv[1]["worst_ink_mismatch"]):
        pc = "" if r["pages_lo"] == r["pages_ours"] else f"  pages {r['pages_ours']}/{r['pages_lo']}"
        print(f"{name:60} worst {r['worst_ink_mismatch']:.3f}  mean {r['mean_ink_mismatch']:.3f}{pc}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
