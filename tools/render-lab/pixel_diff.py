#!/usr/bin/env python3
"""pixel_diff.py — compare our pages with LibreOffice's pixel by pixel.

Usage: pixel_diff.py OUT_DIR [--app APP]... [--tier A] [--tol 64] [--overlay]
                     [--scoreboard FILE [--title T] [--previous JSON] [--limit N]]

For every fixture under OUT_DIR/<app>/<name>/ with LibreOffice pages
(lo-N.png) and ours (<tier>-N.png), each page is compared at LibreOffice's
size: ours is resampled to it, both go to grey, and a pixel differs when
the two greys are more than --tol levels apart. Two numbers per page:

  ink_mismatch   differing pixels / pixels inked in either render — the
                 parity figure: 0.0 is pixel-identical text and rules,
                 1.0 is nothing in common. Blank paper cannot dilute it.
  page_mismatch  differing pixels / all pixels on the page.
  band_miss      ink_miss_1px with vertical drift forgiven: each band of
                 LibreOffice's page (its ink rows, split at blank gaps) is
                 matched against ours at the vertical shift within +-SHIFT
                 px that agrees best. What remains is what differs within
                 lines (indents, wrapping, glyphs, rules), however far an
                 earlier miss pushed them; the gap to ink_miss_1px is drift.
  ink_miss_1px   ink pixels with no ink within 1 px in the other render /
                 ink pixels in either — the same figure with anti-aliasing
                 and sub-pixel glyph placement forgiven. Strict ink_mismatch
                 sits near 0.2 on visually identical text; this one near 0.

No OCR, no alignment search, no per-metric budgets: the number says how
much of what either renderer drew the other did not draw in the same
place. With --overlay each page gets pixel-N.png beside it: black where
either has ink, red where only LibreOffice has ink within 1 px, blue where
only we do.

Prints one line per fixture (its worst page by ink_miss_1px) sorted worst first, and
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


def near(mask, r=1):
    """mask grown by r pixels in every direction."""
    out = mask.copy()
    h, w = mask.shape
    for dy in range(-r, r + 1):
        for dx in range(-r, r + 1):
            out[max(dy, 0):h + min(dy, 0), max(dx, 0):w + min(dx, 0)] |= \
                mask[max(-dy, 0):h + min(-dy, 0), max(-dx, 0):w + min(-dx, 0)]
    return out


SHIFT = 40  # px a band may slide to meet its counterpart (band_miss)


def bands(ink):
    """Row ranges of `ink` holding ink, split at blank rows."""
    rows = np.where(ink.any(1))[0]
    if not len(rows):
        return []
    cuts = np.where(np.diff(rows) > 1)[0] + 1
    return [(int(g[0]), int(g[-1]) + 1) for g in np.split(rows, cuts)]


def band_miss(lo_ink, our_ink):
    """(missed, inked) over LibreOffice's bands, each at its best shift."""
    h = lo_ink.shape[0]
    near_ours = near(our_ink)
    near_lo = near(lo_ink)
    missed = inked = 0
    for top, bottom in bands(lo_ink):
        best = None
        for dy in range(-SHIFT, SHIFT + 1):
            t, b = top + dy, bottom + dy
            if t < 0 or b > h:
                continue
            lo_b = lo_ink[top:bottom]
            ours_b = our_ink[t:b]
            miss = int((lo_b & ~near_ours[t:b]).sum() + (ours_b & ~near_lo[top:bottom]).sum())
            ink = int((lo_b | ours_b).sum())
            if best is None or miss < best[0]:
                best = (miss, ink)
        if best:
            missed += best[0]
            inked += best[1]
    return missed, inked


def compare(lo_path, ours_path, tol, overlay_path=None):
    lo = grey(lo_path)
    ours = grey(ours_path, (lo.shape[1], lo.shape[0]))
    differs = np.abs(lo - ours) > tol
    inked = (lo <= INK) | (ours <= INK)
    n_ink = int(inked.sum())
    lo_ink, our_ink = lo <= INK, ours <= INK
    lo_only = lo_ink & ~near(our_ink)
    our_only = our_ink & ~near(lo_ink)
    bm, bi = band_miss(lo_ink, our_ink)
    result = {
        "band_miss": bm / bi if bi else 0.0,
        "ink_miss_1px": float((lo_only | our_only).sum() / n_ink) if n_ink else 0.0,
        "ink_mismatch": float((differs & inked).sum() / n_ink) if n_ink else 0.0,
        "page_mismatch": float(differs.mean()),
        "ink_pixels": n_ink,
    }
    if overlay_path:
        rgb = np.full(lo.shape + (3,), 255, dtype=np.uint8)
        rgb[lo_ink | our_ink] = (0, 0, 0)
        rgb[lo_only] = (220, 30, 30)
        rgb[our_only] = (30, 80, 220)
        Image.fromarray(rgb).save(overlay_path)
    return result


def score(pages, pages_ours):
    """A document's pixel miss: ink_miss_1px averaged over its pages, a
    page either side has and the other lacks counting as a full miss. 0.0
    is LibreOffice's pages to the pixel, 1.0 nothing in common."""
    total = sum(p["ink_miss_1px"] for p in pages.values())
    extra = max(0, pages_ours - len(pages))
    n = len(pages) + extra
    return (total + extra) / n if n else 1.0


def measure(out, app, tier, tol, overlay):
    """{"<app>/<fixture>": entry} for every fixture of `app` LibreOffice
    rendered. A fixture we failed to capture scores 1.0."""
    report = {}
    for d in sorted(glob.glob(os.path.join(out, app, "*"))):
        lo_pages = {page_no(p): p for p in glob.glob(os.path.join(d, "lo-[0-9]*.png"))}
        ours = {page_no(p): p for p in glob.glob(os.path.join(d, f"{tier}-[0-9]*.png"))}
        if not lo_pages:
            continue
        pages = {}
        for n in sorted(lo_pages):
            if n not in ours:
                pages[n] = {"ink_mismatch": 1.0, "ink_miss_1px": 1.0, "band_miss": 1.0,
                            "page_mismatch": None, "missing": True}
                continue
            path = os.path.join(d, f"pixel-{n}.png") if overlay else None
            pages[n] = compare(lo_pages[n], ours[n], tol, path)
        report[f"{app}/{os.path.basename(d)}"] = {
            "score": score(pages, len(ours)),
            "band": float(np.mean([p["band_miss"] for p in pages.values()])),
            "worst": max(p["ink_miss_1px"] for p in pages.values()),
            "worst_strict": max(p["ink_mismatch"] for p in pages.values()),
            "worst_band": max(p["band_miss"] for p in pages.values()),
            "first_page": pages[min(pages)]["ink_miss_1px"],
            "pages_lo": len(lo_pages),
            "pages_ours": len(ours),
            "extra_pages": sorted(set(ours) - set(lo_pages)),
            "pages": pages,
        }
    return report


def scoreboard(report, title, previous=None, limit=None):
    """The report as a Markdown table, worst document first: the work queue.
    `previous` ({fixture: score}) adds each score's change since."""
    rows = sorted(report.items(), key=lambda kv: (-kv[1]["score"], kv[0]))
    n = len(rows)
    identical = sum(1 for _, r in rows if r["score"] < 0.005)
    mean = float(np.mean([r["score"] for _, r in rows])) if rows else 0.0
    out = [
        f"### {title}",
        "",
        f"{n} documents; mean pixel miss **{mean:.3f}**; {identical} match LibreOffice to the pixel (< 0.005). "
        "Worst first: fix from the top.",
        "",
        "Pixel miss: ink in one render with no ink within 1 px in the other, over all ink, per page, "
        "averaged over the pages (a missing or extra page counts 1.0). Within lines: the same with each "
        "line band free to slide 40 px, so it shows what differs inside lines, however far an earlier "
        "miss pushed them; the gap between the two is vertical drift.",
        "",
        "| # | Document | Pixel miss |" + (" Change |" if previous is not None else "") + " Within lines | Page 1 | Pages (ours/LO) |",
        "|---:|---|---:|" + ("---:|" if previous is not None else "") + "---:|---:|---|",
    ]
    for i, (name, r) in enumerate(rows[:limit] if limit else rows, 1):
        change = ""
        if previous is not None:
            was = previous.get(name)
            change = " new |" if was is None else f" {r['score'] - was:+.3f} |" if abs(r["score"] - was) >= 0.005 else " = |"
        pages = f"{r['pages_ours']}/{r['pages_lo']}" + ("" if r["pages_ours"] == r["pages_lo"] else " ⚠")
        out.append(f"| {i} | `{name}` | {r['score']:.3f} |{change} {r['band']:.3f} | {r['first_page']:.3f} | {pages} |")
    if limit and n > limit:
        out.append(f"| | … {n - limit} more, all below {rows[limit - 1][1]['score']:.3f} | | | | |")
    return "\n".join(out) + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("out")
    ap.add_argument("--app", action="append", help="app to measure (repeatable); default: every app in OUT")
    ap.add_argument("--tier", default="A")
    ap.add_argument("--tol", type=int, default=64)
    ap.add_argument("--overlay", action="store_true")
    ap.add_argument("--scoreboard", metavar="FILE", help="write the ranked Markdown scoreboard here")
    ap.add_argument("--title", default="Pixel scoreboard")
    ap.add_argument("--previous", metavar="JSON", help="an earlier pixel-scores.json: adds a change column")
    ap.add_argument("--limit", type=int, help="scoreboard rows (default: all)")
    a = ap.parse_args(argv)

    apps = a.app or sorted(d for d in os.listdir(a.out) if os.path.isdir(os.path.join(a.out, d)) and d in ("letters", "tables", "decks"))
    report = {}
    for app in apps:
        part = measure(a.out, app, a.tier, a.tol, a.overlay)
        report.update(part)
        with open(os.path.join(a.out, f"pixel-{app}-{a.tier}.json"), "w") as f:
            json.dump(part, f, indent=1, sort_keys=True)
    # The scores alone, the shape --previous reads back.
    with open(os.path.join(a.out, "pixel-scores.json"), "w") as f:
        json.dump({k: round(v["score"], 4) for k, v in report.items()}, f, indent=1, sort_keys=True)
    if a.scoreboard:
        previous = None
        if a.previous and os.path.exists(a.previous):
            with open(a.previous) as f:
                previous = json.load(f)
        with open(a.scoreboard, "w") as f:
            f.write(scoreboard(report, a.title, previous, a.limit))
    for name, r in sorted(report.items(), key=lambda kv: -kv[1]["score"]):
        pc = "" if r["pages_lo"] == r["pages_ours"] else f"  pages {r['pages_ours']}/{r['pages_lo']}"
        print(f"{name:52} miss {r['score']:.3f}  band {r['band']:.3f}  p1 {r['first_page']:.3f}{pc}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
