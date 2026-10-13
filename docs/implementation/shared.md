# Shared implementation specification

Status: **pending unless a package explicitly cites observed completion**.
Baseline: `6dacab4d` (PR #1550), inspected 2026-10-13. This is an execution
specification, not a release certificate. App-specific contracts are in
[Letters](letters.md), [Tables](tables.md), and [Decks](decks.md).

## SH-001 — Evidence and required-check boundary

**Problem.** A passing PR is not necessarily a passing product: the branch
protection API inspected on 2026-10-13 requires only the `test` context.
`ci-gates.md` explicitly leaves required-check configuration open. Tests that
were not collected, skipped or run at another revision cannot certify a feature.

**Targets:** `conformance/capabilities.json`, `conformance/lanes.json`,
`conformance/collect_test_inventory.py`, `conformance/collect_skip_reasons.py`,
`conformance/validate_capabilities.py`, `.github/workflows/ci.yml`,
`.github/workflows/gui-tests.yml`, `.github/workflows/release-revision.yml`,
`tests/test_validate_capabilities.py`, `docs/readiness-2026-09/ci-gates.md`.

**Contract and steps:**
1. Preserve existing feature IDs and evidence-layer vocabulary. Add an app
   capability only for a bounded user-visible contract with actual test IDs.
2. Inventory each lane's collected tests and actual outcomes. Assert that every
   admitted release-critical capability is represented in a lane that runs on
   the candidate. Missing results are errors, not successful empty sets.
3. Validate each layer against its own namespace; do not let a core test result
   satisfy a GUI requirement or an older nightly certify a new candidate.
4. Produce an administrator-reviewed required-check list from actual workflow
   job names, including failure semantics for optional versus gating jobs.
   Configure branch rules only with repository-admin authority; do not change
   workflow names to evade rules or use admin merge bypass.
5. Reconcile readiness status and public claims from the same evidence. Preserve
   historical results as dated evidence, not current release proof.

**Acceptance:** existing validator mutation tests and a negative case for each
missing/skipped/stale result fail as intended; valid inventories pass; the
GitHub API reports the agreed required contexts. Record API output and revision.
Configuration authority is an external prerequisite; implementing a validator
cannot check off the repository setting. Separate validator and settings PRs.

## SH-002 — Same-document multi-window save conflicts

**Problem.** `docs/readiness-2026-09/crash-stress.md` leaves same-app multi-window
close/save races unresolved. The applications are single-instance; launching a
second process is not proof that two independent document windows were tested.
An atomic rename alone does not prevent one editor from replacing another's work.

**Targets:** `suite-common-core/src/atomic_save.rs`,
`suite-common-core/src/autosave.rs`, app save/close consumers identified by the
per-app specs, `tests/gui/framework/base.py`, `tests/gui/test_smoke.py`,
`{letters,tables,decks}-core/tests/stateful.rs`.

**Ownership contract:** one session owns its document identity, dirty revision,
savepoint and recovery generation. Successful save alone advances clean state.
A conflict or cancellation preserves edits and the last recoverable checkpoint.
Do not introduce a second document model or use global process termination.

**Implementation steps:**
1. Extend the existing GUI harness to identify two windows of one application
   by document identity, without selecting whichever window happens to be active.
2. Reproduce: open the same existing file in two editable sessions; make distinct
   changes; save first; attempt save/close-save from second. Retain both revisions
   and original bytes in the failure artifact.
3. Define a core destination identity/version precondition for overwrite, using
   resolved file identity and a recorded version rather than path spelling only.
   Detect external replacement, symlink aliases and changed contents. Keep the
   check/commit boundary explicit: a check followed by ordinary replace is not
   an atomic compare-and-swap guarantee. Choose an OS-supported coordination
   mechanism or report the remaining external-writer race honestly.
4. On conflict provide Cancel and Save a Copy; no implicit last-writer-wins.
   Revalidate the destination after dialogs. An unrelated file appearing at a
   recovery-copy destination must not be silently overwritten.
5. Use existing fault barriers to exercise failure before and after commit;
   return a structured outcome so callers distinguish committed-but-sync-failed
   from nothing-written. Preserve recovery and ownership rules.

**Proposed tests:** `SameDocumentConflictSmoke` variants per app (new), core
identity/precondition tests in the owning existing module, and replayable
stateful sequences. Cover Save, Save As, Save All where available, close guard,
renamed/deleted original, two distinct files with equal basenames, and two windows
closing concurrently. Verify reopened files, unsaved flags and checkpoint bytes,
not just dialog presence. One shared-core PR followed by one consumer PR per app.

## SH-003 — Versioned recovery preserves the complete admitted model

**Problem.** Recovery lifecycle mechanisms exist and have passed historical
journeys; new model fields must not disappear merely because recovery serializes
an older or narrower format. This is ongoing acceptance for each model migration,
not a request to rebuild the autosave mechanism.

**Targets:** `suite-common-core/src/autosave.rs`, the existing per-app recovery
serialization consumers, `docs/readiness-2026-09/recovery.md`, existing
`*LegacySnapshotUpgradeSmoke`, `*RecoveryIsItselfProtectedSmoke` and
`*KilledMidSaveSmoke` journeys in `tests/gui/test_smoke.py`.

**Steps:** enumerate fields added by the app package; include them in the existing
checkpoint envelope with explicit schema/version handling; retain stable IDs,
source compatibility context and owned assets; reject an unsupported future
version without destroying it; test older admitted snapshots; migrate once and
write a coherent generation before removing the orphan. Exercise a second crash
immediately after recovery. Do not confuse a checkpoint with a user save or mark
recovered content clean. Credentials and temporary extracted assets need an
explicit retention/lifetime policy.

**Acceptance:** candidate recovery restores edited and untouched admitted fields,
independent windows retain separate generations, fault injection cannot mix bytes
and metadata, corrupt/future snapshots are diagnosed and retained, migration
fixtures remain deterministic. App field-completeness tests belong in app cores;
shared envelope tests belong in shared core. A test of only nonempty recovered
text is insufficient.

## SH-004 — Candidate crash, stress and rendering campaign

**Existing mechanism:** `.github/workflows/release-soak.yml` already builds release
binaries and runs the candidate soak (default 20 repeats); it is called before
publication. `.github/workflows/release-revision.yml` also exists. Older prose
saying no release-soak workflow exists is stale; inspect wiring rather than
creating a duplicate. `tests/gui/stress.py` and the three core `tests/stateful.rs`
suites already record seeds and exercise commands.

**Remaining contract:** bind the actual candidate, binaries, dependencies,
container and every first-attempt outcome to the evidence bundle. A green rerun
does not erase an earlier crash or assertion failure. Nightly health on moving
`main` and candidate soak are separate evidence.

**Steps:**
1. Audit the existing publish dependency graph: failed/missing soak, GUI,
   release-critical or oracle evidence must prevent publication, not fail beside
   it after packages are uploaded. Add workflow regression coverage for a real
   missing dependency edge, not source-wording assertions.
2. Complete SH-002 race scenarios before the final campaign. Retain failing GUI
   ordering seeds only when they reproduce a discovered defect; do not invent
   arbitrary permanent seeds to increase test counts.
3. Run release binaries with baseline repeated journeys and admitted display
   matrix, including width, scale, light/dark and high contrast where specified.
   Record actual settings and observed application geometry.
4. Classify crashes, assertions, timeouts, infrastructure failures and visual
   mismatches separately. Preserve minimized input, trace, first failure, core
   dump/backtrace and video when available. Never turn missing infrastructure
   into a pass. Retain campaign results across reruns.
5. Enforce the stated consecutive-run/nightly targets from crash readiness with
   machine-readable history. Elapsed nights cannot be manufactured by running
   one candidate repeatedly during a single afternoon.

**Acceptance:** downloadable manifest identifies SHA, binary hashes, seed,
environment, every attempt and verdict; candidate campaign meets the stated
threshold; no untriaged P0 crash/data loss remains. Use `release-soak.yml` manual
or reusable entry points, not local heavy builds. Fix one minimized product bug
per PR, then rerun the affected campaign; do not loosen assertions.

## SH-005 — Three rendering instruments and real-document admission

**Targets:** `tools/render-lab/fixtures.py`, `tools/render-lab/edit_journeys.py`,
`tools/render-lab/baseline.json`, `tools/render-lab/vm/`,
`.github/workflows/render-parity.yml`, `docs/RENDER-PARITY-ROADMAP.md`,
`conformance/validate_parity.py`, `docs/PARITY.md`.

**Contract:** model roundtrip, headless render, actual editor surface, exported
PDF and installed Flatpak are different instruments. No one result substitutes
for the others. A shared renderer can consistently draw the wrong document.

**Steps:** every visible app feature names a foreign-writer single-feature fixture
and an edit/save/reopen journey; add representative real files with provenance,
license and admitted semantics; reduce failures without removing the real file;
compare every page/slide/sheet region required by the fixture against LibreOffice;
inspect report images before accepting baselines. Use Tier C on the packaged
candidate. Investigate scale/font/crop errors before considering metric changes.
Record accepted metric artifacts with images and reasons, not arbitrary threshold
relaxation. Keep real-corpus regressions linked to their reduced unit fixtures.

**Acceptance:** no unexplained red admitted fixture; new baseline accepted only
with inspected images; README status derived from the actual report; missing
pages, text, images or formulas cannot pass through averaging. Run per-app render
and export jobs in parallel CI. VM/KVM runner availability is an explicit
prerequisite, not permission to replace Tier C with screenshots of debug binaries.

## SH-006 — Native platform and manual accessibility signoff

**Targets:** `docs/ACCESSIBILITY.md`,
`docs/readiness-2026-09/performance-accessibility.md`, `tests/gui/test_smoke.py`,
`tests/gui/visual_golden.py`, existing installed-app checks in
`tools/render-lab/installed_check.py` and `tools/render-lab/vm/installed.sh`.

**Steps:** consume each app's keyboard/caret/selection contracts; walk with actual
Orca on GNOME/Wayland; record versions, settings, focus, spoken state, deviations
and artifact IDs. Include narrow windows, zoom, scale, dark/high contrast,
clipboard ownership, modal dialogs and error focus restoration. Verify file-manager
MIME activation, recent files, drag/drop and open/save portals from the installed
Flatpak with real sandbox grants. A host path accidentally accessible outside
Flatpak cannot prove portal behavior. Recheck grants after reopen/restart.

**Acceptance:** every admitted task completes without a pointer where specified;
state changes and refusal reasons are announced; focus remains reachable; measured
accessibility geometry follows actual layout. Deterministic AT-SPI assertions
remain gating; VLM review is advisory. Human Orca observations must name observer,
candidate and environment; agents must not fabricate them from AT-SPI role lists.

## SH-007 — Dependency and importer security floor

**Current blocker:** #1549, Typst 0.15.1 → direct `serde_yaml` → `unsafe-libyaml`,
RUSTSEC-2024-0449. A Hayagriva-only upgrade does not remove the direct path.
The advisory is unmaintained-package information, not a demonstrated exploitable
CVE, but the configured policy rejects it. The gate remains enabled.

**Targets:** workspace/crate `Cargo.toml`, `Cargo.lock`, `deny.toml`, existing
`BoundedArchive`/`ZipBudget` readers, core `tests/malformed_inputs.rs`, retained
`tests/crashes/` fixtures, `.github/workflows/ci.yml` and nightly security lanes.

**Steps:** trace the full dependency graph; prefer a compatible maintained
upstream migration; audit parser differences before replacing YAML; prove
`cargo tree -i unsafe-libyaml` has no remaining path, run the advisory gate and
actual PDF exports. If no compatible migration exists, keep the blocker open;
a separately approved, time-bounded risk waiver must state actual exposure,
review date and owner. Do not silently suppress the advisory. Preserve bounded
ZIP entries/decompression and parser limits in all newly admitted formats;
malformed input must produce a recoverable error, not panic, hang or huge allocation.

**Acceptance:** security lane passes for the actual locked candidate; independent
export regressions remain green; retained malformed fixtures replay; no test skip
or unrelated dependency pin is used as a fix. Dependency migration is a separate
PR from feature work and does not certify all importer safety.

## SH-008 — Exact-revision release, architecture and rollout

**Prerequisites:** all admitted app packages, SH-001 through SH-007, reviewed
remaining gap list and actual maintainer release authority. Meta-tracker #1190
ends at v3.0 and Letters submitted to Flathub; a documentation merge is not that
finish line. Explicitly deferred work stays named, not erased.

**Targets:** `.github/workflows/release-gate.yml`, `release-revision.yml`,
`release-soak.yml`, `release-evidence.yml`, `tools/release/source_archive.sh`,
`tools/release/flathub_manifests.py`, `tools/release/evidence.py`,
`flatpak/`, `flathub/`, `scripts/release_gate.py`, `docs/readiness-2026-09/release.md`.

**Steps:** freeze a candidate SHA and dependency lock; run supported architectures
and reproducible/offline source-archive builds; reconcile development and Flathub
manifests; install clean and upgrade the prior released app with real document,
settings and recovery fixtures; execute portals, MIME, conversion, keyboard,
manual accessibility, resource and failure journeys against those packages;
assemble a downloadable machine-readable bundle with collected IDs, results,
skips, independent-reader outputs, render reports and environment/binary hashes.
No evidence may silently refer to another revision. Tag/publish only after checks
and maintainer signoff; submit Letters to Flathub, record submission/review state,
and document the legacy-app identity/cutover and recovery migration.

**Acceptance:** the candidate's evidence validator passes; no unresolved P0 loss
or silently skipped critical test; admitted functionality and gap list agree with
UI/docs; packages install/launch/upgrade on the admitted architectures; published
artifact hashes match tested ones. Submission requires actual external account
permissions; record the URL and response rather than checking a box after writing
a manifest. Rollback retains user documents/checkpoints and identifies the last
known safe package. Subsequent code changes invalidate candidate-specific claims.
