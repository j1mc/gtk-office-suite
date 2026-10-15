#!/usr/bin/env python3
"""DCK-007 complex-script and input-method journey — advisory, not gating.

Kept out of test_smoke.py on purpose. Typing Hebrew, CJK, combining marks
and emoji through xdotool remaps a spare keycode per character, and under
Xvfb GTK can read the keymap before the remap lands: the first CI run typed
"שלום" as "ללםם". That is the harness, not the editor, and a gate that
flakes on it would hide real results. The model evidence that does gate is
`grapheme_selection_maps_gtk_offsets` and the random-session property in
decks-core/tests/rich_text.rs; a real IME and Orca are manual DCK-019
evidence. Run it by hand: tests/gui/run_gui_tests.sh test_decks_ime.py
"""

import os
import time

from framework import BaseGUITestCase
from test_smoke import DecksRichTextJourney


class DecksImeBidiSmoke(DecksRichTextJourney, BaseGUITestCase):
    """DCK-007: complex scripts and input-method composition in the native
    editor. Hebrew with digits, CJK, a combining accent and an emoji typed
    in; a character composed through GTK's own input method (Ctrl+Shift+U
    preedit) is not text until it is committed; two Shift+Left presses
    select two grapheme clusters (three code points) exactly; a preedit
    still open when Apply is pressed is not saved; the text survives PPTX
    and ODP save and reopen byte for byte. A real IME and Orca are manual
    DCK-019 evidence, not this journey."""

    app_name = "decks"
    BASE = "Hi שלום 123 東京 "

    def setUp(self):
        self.isolate_snapshot(prefix="decks-ime-bidi-")
        self.isolate_autosave_state()
        super().setUp()

    def test_complex_scripts_preedit_and_grapheme_selection(self):
        from dogtail import rawinput

        self._new_text_box()
        self._open_editor()
        rawinput.keyCombo("<Control>a")
        rawinput.typeText(self.BASE)
        self.wait_until(self._session, lambda s: s and s["text"] == self.BASE,
                        description="Hebrew, digits and CJK typed in logical order")
        rawinput.keyCombo("<Control><Shift>u")
        rawinput.typeText("6587")
        time.sleep(0.5)  # settling: gives a wrong outcome its chance before the check that it didn't happen
        self.assertEqual(self._session()["text"], self.BASE, "input-method preedit reached the text")
        rawinput.keyCombo("space")
        composed = self.BASE + "文"
        self.wait_until(self._session, lambda s: s["text"] == composed, description="the composed character committed")
        rawinput.typeText("e\u0301😀")
        full = composed + "e\u0301😀"
        self.wait_until(self._session, lambda s: s["text"] == full, description="a combining accent and an emoji")
        rawinput.keyCombo("<Shift>Left")
        rawinput.keyCombo("<Shift>Left")
        start = len(composed.encode("utf-8"))
        self.wait_until(self._session, lambda s: (s["start"], s["end"]) == (start, len(full.encode("utf-8"))),
                        description="two grapheme clusters selected by two Shift+Left")
        rawinput.keyCombo("<Control>b")
        self.wait_until(self._session, lambda s: self._styled(s["runs"], "bold") == "e\u0301😀",
                        description="exactly the selected clusters bold")
        # A preedit still open when Apply is pressed is not committed text.
        rawinput.keyCombo("End")
        rawinput.keyCombo("<Control><Shift>u")
        rawinput.typeText("41")
        time.sleep(0.5)  # settling: the preedit is on screen, not in the text
        self._control("Apply", "push button").do_action(0)
        self.wait_until(self._session, lambda s: s is None, description="Apply ending the session")
        box = self.wait_until(self._box, lambda b: b["text"] == full, description="the committed text, no preedit")
        self.assertEqual(self._styled(box["runs"], "bold"), "e\u0301😀")
        root = self.temp_dir(prefix="decks-ime-bidi-save-")
        for ext in ("odp", "pptx"):
            path = os.path.join(root, f"scripts.{ext}")
            self._save_as(path)
            self.relaunch_app(launch_args=[path])
            box = self.wait_until(self._box, lambda b: b and b["text"] == full, interval=0.3,
                                  description=f"the reopened {ext.upper()}")
            self.assertEqual(self._styled(box["runs"], "bold"), "e\u0301😀", ext)
        self.assertIsNone(self.process.poll(), "decks crashed editing complex scripts")
