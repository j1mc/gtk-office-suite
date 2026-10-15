"""The real-document lab runs for every change that reaches an app's rendering.

render-real.yml used to be path-filtered to the corpus and the lab, so
DCK-006 (#1574), which changed only decks-core, merged a deck that went from
amber to red without the lab running. It now starts on every pull request
and each app's job asks tools/render-lab/real_scope.py whether the change
reaches it. Getting that answer wrong is silent in the direction of running
too little — a green check that rendered nothing — so it is pinned here: the
scope comes from the Cargo manifests rather than a list, and the workflow
keeps every app's check reporting on every pull request.
"""

import sys
import tempfile
import unittest
from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "render-real.yml"
sys.path.insert(0, str(REPO_ROOT / "tools" / "render-lab"))

import real_scope  # noqa: E402

# What DCK-006 (#1574) changed, abridged: decks-core and a roadmap row.
DCK_006 = ["ROADMAP.md", "decks-core/src/engine/shape.rs", "decks-core/src/odp.rs",
           "decks-core/tests/master_view.rs"]


def apps_reached(changed):
    return {app for app in real_scope.APPS if real_scope.reaching(app, changed)}


class WhatReachesEachApp(unittest.TestCase):
    def test_the_change_that_regressed_a_real_deck_runs_decks(self):
        self.assertEqual(apps_reached(DCK_006), {"decks"})

    def test_a_shared_core_crate_runs_every_app_that_links_it(self):
        # tables and decks link letters-core through their own core crates.
        self.assertEqual(apps_reached(["letters-core/src/layout.rs"]),
                         {"letters", "tables", "decks"})
        self.assertEqual(apps_reached(["suite-common-core/src/units.rs"]),
                         {"letters", "tables", "decks"})
        self.assertEqual(apps_reached(["suite-common/src/lib.rs"]),
                         {"letters", "tables", "decks"})

    def test_an_app_crate_runs_only_that_app(self):
        self.assertEqual(apps_reached(["letters/src/window.rs"]), {"letters"})
        self.assertEqual(apps_reached(["tables/Cargo.toml"]), {"tables"})
        self.assertEqual(apps_reached(["tables-core/src/model.rs"]), {"tables"})

    def test_suite_export_reaches_the_apps_that_link_it(self):
        self.assertEqual(apps_reached(["suite-export/src/lib.rs"]), {"letters", "tables"})

    def test_shared_inputs_run_every_app(self):
        for path in ("Cargo.lock", "Cargo.toml", "tools/render-lab/compare.py",
                     "tools/render-lab/baseline-real.json", ".github/workflows/render-real.yml"):
            with self.subTest(path=path):
                self.assertEqual(apps_reached([path]), set(real_scope.APPS))

    def test_changes_that_cannot_change_a_render_run_nothing(self):
        for path in ("docs/TESTING.md", "tests/gui/test_smoke.py", "fuzz/Cargo.toml",
                     "flatpak/org.tunaos.letters.json", ".github/workflows/ci.yml",
                     "conformance/required-checks.json"):
            with self.subTest(path=path):
                self.assertEqual(apps_reached([path]), set())

    def test_a_prefix_is_a_directory_not_a_name_prefix(self):
        self.assertEqual(apps_reached(["letters-core-extra/src/lib.rs", "decks.md",
                                       "Cargo.lock.orig"]), set())

    def test_every_app_crate_is_in_its_own_scope(self):
        for app in real_scope.APPS:
            with self.subTest(app=app):
                self.assertIn(f"{app}/", real_scope.scope(app))


class ScopeComesFromTheManifests(unittest.TestCase):
    """A new path dependency widens the scope without anyone editing a list."""

    def workspace(self, crates):
        root = Path(tempfile.mkdtemp())
        for crate, deps in crates.items():
            (root / crate).mkdir()
            lines = ["[package]", f'name = "{crate}"', "[dependencies]"]
            lines += [f'{dep} = {{ path = "../{dep}" }}' for dep in deps]
            (root / crate / "Cargo.toml").write_text("\n".join(lines) + "\n")
        return root

    def test_transitive_path_dependencies_are_followed(self):
        root = self.workspace({"letters": ["letters-core"], "letters-core": ["new-engine"],
                               "new-engine": []})
        self.assertEqual(real_scope.workspace_crates("letters", root),
                         {"letters", "letters-core", "new-engine"})
        self.assertTrue(real_scope.reaching("letters", ["new-engine/src/lib.rs"], root))

    def test_dev_dependencies_do_not_widen_the_scope(self):
        root = self.workspace({"decks": [], "test-helpers": []})
        with open(root / "decks" / "Cargo.toml", "a") as handle:
            handle.write('[dev-dependencies]\ntest-helpers = { path = "../test-helpers" }\n')
        self.assertEqual(real_scope.workspace_crates("decks", root), {"decks"})

    def test_target_specific_dependencies_are_followed(self):
        root = self.workspace({"tables": [], "platform": []})
        with open(root / "tables" / "Cargo.toml", "a") as handle:
            handle.write("[target.'cfg(unix)'.dependencies]\n"
                         'platform = { path = "../platform" }\n')
        self.assertEqual(real_scope.workspace_crates("tables", root), {"tables", "platform"})

    def test_an_unreadable_dependency_raises_rather_than_shrinking(self):
        root = self.workspace({"letters": ["gone"]})
        with self.assertRaises(FileNotFoundError):
            real_scope.workspace_crates("letters", root)

    def test_an_unknown_app_raises(self):
        with self.assertRaises(ValueError):
            real_scope.scope("drawings")


class TheWorkflowReportsEveryAppOnEveryPullRequest(unittest.TestCase):
    def setUp(self):
        with open(WORKFLOW, encoding="utf-8") as handle:
            self.wf = yaml.safe_load(handle)
        self.job = self.wf["jobs"]["lab"]
        self.steps = self.job["steps"]

    def test_every_pull_request_starts_it(self):
        on = self.wf.get("on", self.wf.get(True))
        self.assertIn("pull_request", on)
        self.assertFalse(on["pull_request"],
                         "a paths: filter on render-real.yml stops its checks reporting "
                         "on pull requests outside it; decide per app in the scope step")

    def test_the_job_itself_always_runs(self):
        self.assertNotIn("if", self.job,
                         "a skipped matrix job reports under its unexpanded name, so the "
                         "required Real documents (<app>) check would never arrive")

    def test_the_matrix_is_the_apps_the_scope_knows(self):
        self.assertEqual(set(self.job["strategy"]["matrix"]["app"]), set(real_scope.APPS))

    def test_every_step_after_the_scope_is_gated_by_it(self):
        ids = [step.get("id") for step in self.steps]
        self.assertIn("scope", ids)
        at = ids.index("scope")
        self.assertIn("real_scope.py", self.steps[at]["run"])
        self.assertIn("HEAD^1 HEAD", self.steps[at]["run"])
        for step in self.steps[at + 1:]:
            with self.subTest(step=step.get("name") or step.get("uses")):
                self.assertIn("steps.scope.outputs.run == 'true'", str(step.get("if")))

    def test_the_scope_has_the_merge_commits_parents(self):
        checkout = self.steps[0]
        self.assertTrue(checkout["uses"].startswith("actions/checkout@"))
        self.assertGreaterEqual(int(checkout.get("with", {}).get("fetch-depth", 1)), 2)


if __name__ == "__main__":
    unittest.main()
