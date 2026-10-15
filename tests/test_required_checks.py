#!/usr/bin/env python3
"""Mutation tests for the required-check proposal (SH-001).

`conformance/required_checks.py` derives the check names a pull request
reports from the workflows themselves and refuses a proposal that names a
check which cannot reliably report — path-filtered, conditional, advisory,
skipped behind an optional job, or sharing its name with another job. Each
test below breaks one of those properties in a small synthetic workflow set
and asserts the check names it; the real-repository tests assert the
committed proposal passes and that ci-gates.md carries the generated table
rather than a hand-edited copy.

Dependency-free apart from PyYAML: no network, no settings API.
"""

import copy
import json
import sys
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "conformance"))

import required_checks as rc  # noqa: E402

CI_GATES = REPO_ROOT / "docs" / "readiness-2026-09" / "ci-gates.md"


def workflows(**overrides):
    base = {
        ".github/workflows/ci.yml": {
            "on": {"push": None, "pull_request": None},
            "jobs": {
                "test": {"runs-on": "ubuntu-24.04"},
                "lint": {"name": "Lint everything", "runs-on": "ubuntu-24.04"},
                "bundle": {"needs": ["test"], "strategy": {"matrix": {"app": ["a", "b"]}}},
            },
        },
        ".github/workflows/nightly.yml": {
            "on": {"schedule": [{"cron": "0 5 * * *"}]},
            "jobs": {"slow": {"runs-on": "ubuntu-24.04"}},
        },
    }
    base.update(overrides)
    return base


def policy(*rows, app_id=15368):
    default = [
        {"workflow": ".github/workflows/ci.yml", "job": "test", "policy": "required"},
        {"workflow": ".github/workflows/ci.yml", "job": "lint", "policy": "required"},
        {"workflow": ".github/workflows/ci.yml", "job": "bundle", "policy": "required"},
    ]
    return {"app_id": app_id, "jobs": list(rows) or default}


LANES = {"lanes": [{"job": "test", "workflow": ".github/workflows/ci.yml",
                    "runs_on_every_revision": True}]}


def errors_for(flows, pol, lanes=LANES):
    return "\n".join(rc.check_policy(pol, rc.inventory(flows), lanes))


class TheHonestProposalPasses(unittest.TestCase):
    def test_a_complete_reporting_proposal_passes(self):
        self.assertEqual(errors_for(workflows(), policy()), "")

    def test_a_workflow_no_pull_request_starts_is_not_asked_about(self):
        jobs = rc.inventory(workflows())
        self.assertNotIn("slow", [j["job"] for j in jobs])


class EveryPullRequestJobIsClassified(unittest.TestCase):
    def test_an_unclassified_job_is_rejected(self):
        pol = policy()
        pol["jobs"].pop()
        self.assertIn("ci.yml bundle: a pull request starts this job but",
                      errors_for(workflows(), pol))

    def test_a_row_for_a_job_that_no_longer_exists_is_rejected(self):
        pol = policy()
        pol["jobs"].append({"workflow": ".github/workflows/ci.yml", "job": "renamed-away",
                            "policy": "optional", "reason": "r"})
        self.assertIn("renamed-away: classified, but no pull-request job",
                      errors_for(workflows(), pol))

    def test_a_job_classified_twice_is_rejected(self):
        pol = policy()
        pol["jobs"].append(dict(pol["jobs"][0]))
        self.assertIn("classified twice", errors_for(workflows(), pol))

    def test_an_optional_job_needs_a_reason(self):
        pol = policy()
        pol["jobs"][1] = {"workflow": ".github/workflows/ci.yml", "job": "lint",
                          "policy": "optional"}
        self.assertIn("optional without a reason", errors_for(workflows(), pol))

    def test_a_pending_job_names_the_issue_that_blocks_it(self):
        pol = policy()
        pol["jobs"][1] = {"workflow": ".github/workflows/ci.yml", "job": "lint",
                          "policy": "pending", "reason": "fails on main"}
        self.assertIn("pending without the issue", errors_for(workflows(), pol))

    def test_an_unknown_policy_is_rejected(self):
        pol = policy()
        pol["jobs"][1]["policy"] = "mandatory-ish"
        self.assertIn("unknown policy", errors_for(workflows(), pol))

    def test_a_proposal_that_requires_nothing_is_rejected(self):
        pol = policy(
            {"workflow": ".github/workflows/ci.yml", "job": "test", "policy": "optional", "reason": "r"},
            {"workflow": ".github/workflows/ci.yml", "job": "lint", "policy": "optional", "reason": "r"},
            {"workflow": ".github/workflows/ci.yml", "job": "bundle", "policy": "optional", "reason": "r"},
        )
        self.assertIn("nothing is required", errors_for(workflows(), pol, {"lanes": []}))


class ARequiredCheckMustReportOnEveryPullRequest(unittest.TestCase):
    """A required context that does not report blocks forever; one that
    reports *skipped* counts as passing. Either way the rule lies."""

    def test_a_path_filtered_workflow_cannot_be_required(self):
        flows = workflows()
        flows[".github/workflows/ci.yml"]["on"]["pull_request"] = {"paths": ["src/**"]}
        self.assertIn("path-filtered", errors_for(flows, policy()))

    def test_a_conditional_job_cannot_be_required(self):
        flows = workflows()
        flows[".github/workflows/ci.yml"]["jobs"]["lint"]["if"] = "github.event_name == 'push'"
        self.assertIn("ci.yml lint: required, but conditional", errors_for(flows, policy()))

    def test_an_always_running_condition_is_not_a_gap(self):
        flows = workflows()
        flows[".github/workflows/ci.yml"]["jobs"]["lint"]["if"] = "${{ !cancelled() }}"
        self.assertEqual(errors_for(flows, policy()), "")

    def test_activity_types_that_omit_pushes_cannot_be_required(self):
        flows = workflows()
        flows[".github/workflows/ci.yml"]["on"]["pull_request"] = {"types": ["opened"]}
        self.assertIn("does not report on every push", errors_for(flows, policy()))

    def test_an_advisory_job_cannot_be_required(self):
        flows = workflows()
        flows[".github/workflows/ci.yml"]["jobs"]["lint"]["continue-on-error"] = True
        self.assertIn("continue-on-error", errors_for(flows, policy()))

    def test_a_required_job_behind_an_optional_one_is_rejected(self):
        """If `test` fails, `bundle` is skipped, and skipped counts as passing."""
        pol = policy()
        pol["jobs"][0] = {"workflow": ".github/workflows/ci.yml", "job": "test",
                          "policy": "optional", "reason": "r"}
        self.assertIn("it needs 'test', which is optional", errors_for(workflows(), pol, {"lanes": []}))

    def test_a_name_two_jobs_report_cannot_identify_either(self):
        flows = workflows()
        flows[".github/workflows/other.yml"] = {
            "on": ["pull_request"], "jobs": {"x": {"name": "test"}}}
        pol = policy()
        pol["jobs"].append({"workflow": ".github/workflows/other.yml", "job": "x",
                            "policy": "optional", "reason": "r"})
        self.assertIn("check name 'test' is also reported by", errors_for(flows, pol))

    def test_a_name_that_cannot_be_derived_cannot_be_required(self):
        flows = workflows()
        flows[".github/workflows/ci.yml"]["jobs"]["lint"]["name"] = "Lint ${{ inputs.what }}"
        self.assertIn("cannot be resolved statically", errors_for(flows, policy()))


class EveryRevisionLanesAreRequired(unittest.TestCase):
    def test_a_lane_that_runs_on_every_revision_must_be_required(self):
        pol = policy()
        pol["jobs"][0] = {"workflow": ".github/workflows/ci.yml", "job": "test",
                          "policy": "optional", "reason": "r"}
        pol["jobs"][2]["policy"] = "optional"
        pol["jobs"][2]["reason"] = "r"
        self.assertIn("lane 'test' (.github/workflows/ci.yml) is declared to run on every "
                      "revision, but its job is optional", errors_for(workflows(), pol))

    def test_a_scheduled_lane_is_not_asked_to_be_required(self):
        lanes = {"lanes": [{"job": "slow", "workflow": ".github/workflows/nightly.yml",
                            "runs_on_every_revision": False}]}
        self.assertEqual(errors_for(workflows(), policy(), lanes), "")


class CheckNamesFollowGitHub(unittest.TestCase):
    def names(self, job_id, job):
        names, problem = rc.check_names(job_id, job)
        self.assertIsNone(problem)
        return names

    def test_a_job_without_a_name_reports_its_id(self):
        self.assertEqual(self.names("test", {}), ["test"])

    def test_a_matrix_name_substitutes_its_values(self):
        job = {"name": "Render lab (${{ matrix.app }})",
               "strategy": {"matrix": {"app": ["letters", "decks"]}}}
        self.assertEqual(self.names("lab", job), ["Render lab (letters)", "Render lab (decks)"])

    def test_a_matrix_without_a_name_appends_its_values(self):
        job = {"strategy": {"matrix": {"include": [
            {"arch": "x86_64", "runner": "ubuntu-24.04"},
            {"arch": "aarch64", "runner": "ubuntu-24.04-arm"}]}}}
        self.assertEqual(self.names("offline-flatpak", job), [
            "offline-flatpak (x86_64, ubuntu-24.04)",
            "offline-flatpak (aarch64, ubuntu-24.04-arm)"])

    def test_excluded_combinations_report_nothing(self):
        job = {"strategy": {"matrix": {"os": ["a", "b"], "v": [1, 2],
                                       "exclude": [{"os": "b", "v": 2}]}}}
        self.assertEqual(self.names("j", job), ["j (a, 1)", "j (a, 2)", "j (b, 1)"])

    def test_a_run_time_matrix_is_not_guessed(self):
        _, problem = rc.check_names("j", {"strategy": {"matrix": "${{ fromJSON(x) }}"}})
        self.assertIn("computed at run time", problem)

    def test_the_real_names_match_what_github_reported(self):
        """Check-run names observed on PR #1573's head (08c47279), 2026-10-15,
        via `gh api repos/tuna-os/gtk-office-suite/commits/<sha>/check-runs`."""
        observed = {
            "test", "interop-corpus", "Python validators and their tests",
            "RustSec advisory gate", "flatpak (letters)", "flatpak (tables)",
            "flatpak (decks)", "smoke", "Render lab (letters)", "Render lab (tables)",
            "Render lab (decks)", "Tiers A+B vs LibreOffice", "Export lab (letters)",
            "Export lab (decks)", "Export lab (tables)", "Export PDF vs LibreOffice PDF",
            "contract", "source-archive", "aarch64-tests", "Reproducible build (first)",
            "Reproducible build (second)", "Reproducible build (compare)",
            "offline-flatpak (x86_64, ubuntu-24.04)",
            "offline-flatpak (aarch64, ubuntu-24.04-arm)", "corpus (letters)",
            "corpus (tables)", "corpus (decks)", "oracle", "gate",
        }
        derived = {name for job in rc.inventory(rc.load_workflows()) for name in job["names"]}
        self.assertEqual(observed - derived, set())


class ComparingWithTheSettingsApi(unittest.TestCase):
    def setUp(self):
        self.flows = rc.inventory(workflows())
        self.pol = policy()

    def protection(self, *contexts, app_id=15368):
        return {"required_status_checks": {
            "contexts": list(contexts),
            "checks": [{"context": c, "app_id": app_id} for c in contexts]}}

    def test_the_observed_test_only_rule_differs(self):
        differences = rc.compare(self.protection("test"), self.pol, self.flows)
        self.assertIn("proposed but not required: 'Lint everything'", differences)
        self.assertIn("proposed but not required: 'bundle (a)'", differences)

    def test_an_extra_required_context_is_reported(self):
        wanted = rc.required_contexts(self.pol, self.flows)
        differences = rc.compare(self.protection(*wanted, "ghost"), self.pol, self.flows)
        self.assertEqual(differences, ["required but not proposed: 'ghost'"])

    def test_a_context_any_app_can_satisfy_is_reported(self):
        wanted = rc.required_contexts(self.pol, self.flows)
        differences = rc.compare(self.protection(*wanted, app_id=None), self.pol, self.flows)
        self.assertTrue(differences)
        self.assertIn("not GitHub Actions", differences[0])

    def test_the_applied_proposal_matches(self):
        wanted = rc.required_contexts(self.pol, self.flows)
        self.assertEqual(rc.compare(self.protection(*wanted), self.pol, self.flows), [])

    def test_the_payload_pins_every_context_to_github_actions(self):
        body = rc.payload(self.pol, self.flows)
        self.assertEqual({c["app_id"] for c in body["checks"]}, {15368})
        self.assertEqual([c["context"] for c in body["checks"]],
                         rc.required_contexts(self.pol, self.flows))


class TheRealProposal(unittest.TestCase):
    def setUp(self):
        self.policy = json.loads(rc.POLICY.read_text())
        self.lanes = json.loads(rc.LANES.read_text())
        self.jobs = rc.inventory(rc.load_workflows())

    def test_the_committed_proposal_is_valid(self):
        self.assertEqual(rc.check_policy(self.policy, self.jobs, self.lanes), [])

    def test_main_exits_zero(self):
        self.assertEqual(rc.main([]), 0)

    def test_a_mutated_real_proposal_is_rejected(self):
        broken = copy.deepcopy(self.policy)
        row = next(r for r in broken["jobs"] if r["job"] == "oracle")
        row["policy"] = "required"
        errors = "\n".join(rc.check_policy(broken, self.jobs, self.lanes))
        self.assertIn("nightly.yml oracle: required, but path-filtered", errors)

    def test_the_every_revision_lanes_are_required(self):
        required = set(rc.required_contexts(self.policy, self.jobs))
        self.assertTrue({"test", "smoke", "Python validators and their tests"} <= required)

    def test_ci_gates_embeds_the_generated_table(self):
        """The table in ci-gates.md is generated; a hand edit drifts."""
        text = CI_GATES.read_text()
        start, end = text.find(rc.BEGIN_MARK), text.find(rc.END_MARK)
        self.assertGreaterEqual(start, 0, "ci-gates.md has no generated required-check table")
        self.assertGreater(end, start)
        embedded = text[start:end + len(rc.END_MARK)]
        self.assertEqual(embedded, rc.render_markdown(self.policy, self.jobs),
                         "regenerate with: python3 conformance/required_checks.py --markdown")


if __name__ == "__main__":
    unittest.main()
