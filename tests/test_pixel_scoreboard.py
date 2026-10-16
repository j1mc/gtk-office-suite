"""The pixel scoreboard (tools/render-lab/pixel_diff.py, pixel_board.py).

The render lab ranks every document by how far our pixels are from
LibreOffice's, worst first, so work goes to the worst diff first. These
pin what the ranking means: identical pages score 0, a missing or extra
page counts as a full miss, a whole-page shift is drift (the page score
sees it, the within-lines score forgives it), the board lists the worst
document first, and the scores a board publishes read back for the next
run's change column.
"""

import json
import os
import sys
import tempfile
import unittest

from PIL import Image, ImageDraw

LAB = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "tools", "render-lab")
sys.path.insert(0, LAB)

import pixel_board  # noqa: E402
import pixel_diff  # noqa: E402


def page(path, dy=0, text=True):
    """A white page with three black bars (stand-ins for lines), moved down
    by `dy` pixels."""
    img = Image.new("L", (200, 160), 255)
    d = ImageDraw.Draw(img)
    if text:
        for i in range(3):
            y = 20 + 30 * i + dy
            d.rectangle([20, y, 160, y + 8], fill=0)
    img.save(path)


class Scoring(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.out = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def doc(self, name, lo, ours):
        """A document: LibreOffice's pages and ours, each a list of dy
        (None: a blank page)."""
        d = os.path.join(self.out, "letters", name)
        os.makedirs(d)
        for n, dy in enumerate(lo, 1):
            page(os.path.join(d, f"lo-{n}.png"), dy or 0, dy is not None)
        for n, dy in enumerate(ours, 1):
            page(os.path.join(d, f"A-{n}.png"), dy or 0, dy is not None)

    def test_identical_pages_score_zero(self):
        self.doc("same", [0, 0], [0, 0])
        r = pixel_diff.measure(self.out, "letters", "A", 64, False)["letters/same"]
        self.assertEqual(r["score"], 0.0)
        self.assertEqual(r["band"], 0.0)

    def test_a_shift_is_drift_the_page_sees_and_lines_forgive(self):
        self.doc("shifted", [0], [12])
        r = pixel_diff.measure(self.out, "letters", "A", 64, False)["letters/shifted"]
        self.assertGreater(r["score"], 0.5)
        self.assertLess(r["band"], 0.01)

    def test_a_missing_or_extra_page_is_a_full_miss(self):
        self.doc("short", [0, 0], [0])
        self.doc("long", [0], [0, 0])
        report = pixel_diff.measure(self.out, "letters", "A", 64, False)
        self.assertAlmostEqual(report["letters/short"]["score"], 0.5)
        self.assertAlmostEqual(report["letters/long"]["score"], 0.5)

    def test_the_board_lists_the_worst_first_with_its_change(self):
        self.doc("good", [0], [0])
        self.doc("bad", [0], [12])
        report = pixel_diff.measure(self.out, "letters", "A", 64, False)
        board = pixel_diff.scoreboard(report, "Board", previous={"letters/bad": 0.9})
        rows = [line for line in board.splitlines() if line.startswith("| 1 ") or line.startswith("| 2 ")]
        self.assertIn("letters/bad", rows[0])
        self.assertIn("letters/good", rows[1])
        self.assertIn(" new |", rows[1])
        self.assertIn(" +0.100 |", rows[0])

    def test_main_writes_the_scores_previous_reads(self):
        self.doc("good", [0], [0])
        board = os.path.join(self.out, "board.md")
        pixel_diff.main([self.out, "--scoreboard", board])
        with open(os.path.join(self.out, "pixel-scores.json")) as f:
            self.assertEqual(json.load(f), {"letters/good": 0.0})
        with open(board) as f:
            self.assertIn("`letters/good`", f.read())


class Board(unittest.TestCase):
    def test_a_published_body_carries_its_marker_and_scores(self):
        text = pixel_board.body("| table |", {"letters/x": 0.25}, "fixtures", "https://run")
        self.assertIn(pixel_board.MARK.format("fixtures"), text)
        self.assertEqual(json.loads(pixel_board.SCORES.search(text).group(1)), {"letters/x": 0.25})


if __name__ == "__main__":
    unittest.main()
