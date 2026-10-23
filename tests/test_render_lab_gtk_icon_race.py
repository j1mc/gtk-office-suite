"""A capture that GTK's icon cache race killed before its first frame
(#1648) runs once more; any other empty capture still counts
(tools/render-lab/capture.py, retry_gtk_icon_race)."""

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "tools", "render-lab"))

try:
    import capture
except ImportError:  # pragma: no cover - the render lab's own image has its imports
    capture = None

RACE = "(decks:6600): GLib-GObject-CRITICAL **: 06:15:04.701: object_ref: assertion '!object_already_finalized' failed\n"


@unittest.skipIf(capture is None, "the render lab's imports")
class GtkIconRaceTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.log = os.path.join(self.dir, "A.log")

    def runs(self, logs, pages):
        """A fake capture that writes each run's log and returns its pages."""
        calls = []

        def run():
            with open(self.log, "w") as f:
                f.write(logs[len(calls)])
            calls.append(1)
            return pages[len(calls) - 1]

        return run, calls

    def test_a_raced_capture_runs_once_more(self):
        run, calls = self.runs([RACE, ""], [0, 2])
        self.assertEqual(capture.retry_gtk_icon_race(run, self.log), 2)
        self.assertEqual(len(calls), 2)
        with open(self.log + ".race") as f:
            self.assertIn("object_already_finalized", f.read(), "the raced run's log is kept")

    def test_racing_twice_still_fails(self):
        run, calls = self.runs([RACE, RACE], [0, 0])
        self.assertEqual(capture.retry_gtk_icon_race(run, self.log), 0)
        self.assertEqual(len(calls), 2)

    def test_any_other_empty_capture_is_not_run_again(self):
        run, calls = self.runs(["thread 'main' panicked at src/canvas.rs\n"], [0])
        self.assertEqual(capture.retry_gtk_icon_race(run, self.log), 0)
        self.assertEqual(len(calls), 1)

    def test_a_capture_that_drew_is_not_run_again(self):
        run, calls = self.runs([RACE], [1])
        self.assertEqual(capture.retry_gtk_icon_race(run, self.log), 1, "a critical with pages drawn is not a lost capture")
        self.assertEqual(len(calls), 1)

    def test_an_export_with_no_pdf_counts_like_no_pages(self):
        run, calls = self.runs([RACE, ""], [None, 1])
        self.assertEqual(capture.retry_gtk_icon_race(run, self.log), 1)


if __name__ == "__main__":
    unittest.main()
