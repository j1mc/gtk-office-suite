# [P1] Decks: prove authoring, master fidelity, and presenter/export workflows end to end

Audited on `e7e4df6`. Follow-up verification to closed #115–#117, using `docs/DECKS-STYLING-CHECKLIST.md` and `docs/adr/0004-decks-advanced-workflows.md` as contracts, not test results.

## Architecture
One DecksController owns slide/object/master state, stable identities, selection, history and dirty revision. Canvas and inspector apply the same commands. A drag/resize gesture commits one undoable operation. Rendering, thumbnails, presentation and PDF export consume the same model and geometry. Imported package context travels with the document into the loss-budget save boundary.

## Slices and tests
- [x] Inventory each advertised action and prove it mutates canonical state: select/multi-select, move/resize/rotate, align/arrange/group, duplicate/reorder, style and image fit/crop.
    [DECKS-ACTIONS.md](../DECKS-ACTIONS.md): every `app.*` action and every
    canvas and inspector control, what it changes in the model and the
    test that proves it; `tests/test_decks_action_inventory.py` fails on
    an action with no row or a cited test that isn't there. New journey
    `DecksAdvertisedActionsSmoke` covers what had none: No Fill and Bold
    (one undo step each), Delete on the selected object, Add Image and
    Open. It found two bugs: the canvas could never take keyboard focus,
    so Delete and the arrow keys did nothing, and the odp reader dropped
    a page written as one empty element. Multi-select, grouping and
    multi-object align are not offered, and the inventory says so. Image
    crop is the inspector's Picture group (`DecksPictureCropSmoke`).
- [x] Undo/redo mixed object and slide edits, including deletion and selection repair; no detached state or reentrant RefCell panic.
    `decks-core/tests/controller_property.rs` drives DecksController, the
    API the window calls, with random mixes of object edits (add, delete,
    move, resize, rotate, z-order), slide edits (add, delete, duplicate,
    move up/down), undos and redos. A model of the history predicts every
    undo and redo; each edit that changes the deck is exactly one step and
    clears redo; ids stay one per object; undo-all restores the start.
    After every step `DecksController::repair_selection` keeps the current
    slide and selected object pointing at something that exists, and both
    undo paths in the window (`app.undo`/`app.redo`, the canvas keys) now
    apply it: before, undoing an added slide or object left indices past
- [x] PPTX and ODP journeys preserve supported text runs, images, object geometry/style, master decorations/mapping, slide order and speaker notes.
    `DecksFormatJourneySmoke` opens a hand-built odp with styled runs, a
    filled, outlined and rotated shape, an embedded picture, speaker
    notes, two slides in order and a master with a decoration; adds a
    shape; Save As pptx and reopens; Save As odp from that and reopens.
    Each time every object is what was saved, by the test snapshot's new
    `detail` (the model in full, a picture by a hash of its bytes), and
    so are the notes, slide names and order and the master's decorations.
    The journey found the pptx reader truncating run sizes to the half
    point below (19.84 pt read as 19.5); it rounds now.
- [x] Compare supported shape/style/rotation/crop fields in our writer → Impress rewrite → our reader, not just text extraction.
    Shape kind, fill, outline colour and width, rotation and geometry
    for every preset we draw (rect, rounded rect with its radius,
    ellipse, triangle, diamond) now go through Impress in both formats:
    `shape_kind_fill_outline_and_rotation_survive_impress_in_both_formats`
    in `decks-core/tests/soffice_oracle.rs`. Gradients and theme
- [x] Presenter current/next/notes/timer and external-display disconnect fallback meet the admitted ADR.
    Current, next, notes and the running clock: `DecksPresenterDisplaySmoke`.
    Disconnect fallback, ADR 0004's "return to primary and show a visible
    status": `decks_core::presenter::layout_after_monitor_change` moves a
    window whose monitor went away back to the primary one; the show
    watches GDK's monitor list, puts the audience window back fullscreen
    there, and shows "The external display was disconnected…" over the
    slides and as a banner on the presenter display.
    `DecksPresenterDisplayLostSmoke` lays a show out for two monitors
    (test mode only), reports the one real monitor, and sees the banner,
- [x] PDF/print uses the same slide size/order/master content; export failure is visible and leaves the source document untouched.
    Size: `a_4_3_deck_exports_on_its_own_page` and
    `a_pdf_has_a_page_per_slide_with_its_text` (poppler reads the page
    size). Order: each page's text, in `decks/src/export.rs`. Master
    content: `every_pdf_page_carries_the_masters_decorations` rasterises
    each page and finds the master's band. Same renderer as the editor:
    `a_pdf_page_and_the_png_show_the_same_slide`. Failure:
    `an_export_that_cannot_be_written_is_an_error_and_writes_nothing`,
    and `DecksExportSmoke.test_a_failed_export_is_shown_and_leaves_the_deck_alone`
    exports into `/proc`, sees "Export Failed", and finds the deck and
    its file unchanged.
- [x] Missing media and unsupported animation/comment content is preserved or blocked/warned by #374 before save.
    Warned. `decks_core::loss::content_a_save_drops` reads the file a save
    would replace or copy from and lists what the writers can't carry:
    comments, audio and video, embedded OLE objects, SmartArt, emphasis
    and motion-path animations, and pictures the file names but doesn't
    contain (unit tests for both formats in `decks-core/src/loss.rs`).
    Save and Save As ask first (`decks/src/loss_ui.rs`, "Save Without This
    Content?"); Cancel writes nothing. On the 70 real pptx files of the
    render-lab corpus it flags 9: six with SmartArt, two with OLE objects,
    one with motion paths, and no false comment from PowerPoint's authors
    list. GUI: `DecksUnsupportedContentSmoke` (Cancel keeps the file's
    bytes, Save Anyway drops the comment, the next save doesn't ask).

Exit: deterministic GUI snapshots plus reopened-file semantics for authoring; display fallback test and exported-PDF evidence for presentation. A documentation checklist alone cannot close this issue. Depends on #354, #374 and the shared P0 persistence work.

## Assessment update — 2026-10-13

The checklist above is retained as historical evidence for its admitted subset,
not a declaration of PowerPoint parity. Its Architecture paragraph is a target
contract: selection is still held by the GTK window, and the package-loss boundary
is not an opaque-preservation engine. This update is a source/document/issue
assessment; no applications, builds, tests, render comparisons or REA sessions
were run. Statements about past runs below are attributed to their PRs/issues,
not new verification. All gates below remain open until their own evidence exists.

**Verdict [INFERENCE]: retain the native GTK4/libadwaita shell and Rust engine,
but change the delivery order.** There is a viable foundation for an everyday
presentation editor, not evidence of a general PowerPoint replacement yet.
The bottlenecks are loss-aware format coverage, model expressiveness and actual
canvas authoring, not an inherent GTK or Rust limitation. A useful design tool
needs reliable text, reusable layouts, multi-object composition and a dependable
show before more transition effects, AI generation or collaboration.

### What the current implementation actually supports

| Area | Observed implementation | Boundary that the roadmap must not hide |
|---|---|---|
| Engine and ownership | `decks-core/src/controller.rs` applies inverse `Op`s through shared history; `ops.rs` gives slides/objects identities. Master editing uses a stand-in slide and commits a `SetMaster` operation. | Controller fields expose shared mutable slide/master vectors; `decks/src/window.rs` still owns `current_slide` and a single `selected_object`. The target is one command/selection transaction boundary, not a second engine or widget-owned shadow model. |
| Slide/master/layout model | `engine/model.rs` has slides, notes, masters, layouts, builds, text runs, shapes, pictures, tables and charts. `layouts.rs` applies layouts while retaining unmatched content; `templates.rs` defines five real starter decks. | Geometry is normalized to 960×540; original page size is stored on masters and the first master's size controls PDF/PPTX output. Placeholders have only Title/Subtitle/Body roles and rectangles. There is no group object or first-class theme-token graph in `SlideObject`/`Deck`; resolving import appearance is not preserving editable theme inheritance. |
| Theme fidelity | `engine/parse.rs` reads `ppt/theme/theme1.xml` for its theme; `engine/write.rs::theme_part_xml` writes conventional color slots and one master's font as both major/minor fonts. | Arbitrary multi-theme decks, scheme overrides and changing a theme without flattening direct formatting need explicit model/relationship work. Existing master/layout import is valuable but does not establish full PowerPoint theme round trips. |
| Canvas and text | Cairo/Pango render imported styled text; the inspector changes style and geometry; pointer gestures move/resize/rotate one object, with core snapping guides. | `docs/DECKS-ACTIONS.md` explicitly excludes multi-select/grouping/multi-object align/distribute. The double-click path in `canvas_keys.rs` creates a plain `TextView`, uses fixed 800×450 overlay coordinates and only attaches if the canvas parent is `gtk::Fixed`; `window.rs` installs the canvas in a `ScrolledWindow`. Focus loss only unparents the overlay. `DecksController::change_text` changes `text` but not `runs`, while `canvas.rs::set_styled_text` renders nonempty runs. These source-level mismatches are authoring blockers to reproduce/fix, not an exercised GUI diagnosis. |
| ODP/PPTX | Independent readers/writers exist in `odp.rs` and `engine/{parse,write}.rs`, with styled paragraphs, image crop, supported shapes, notes and master/layout handling. `engine/chart.rs` reads/writes an admitted single-series chart subset. | Both writers rebuild packages; importing a visible approximation does not mean unsupported relationships or editable semantics survive. ODP and PPTX need separate read/edit/write/render coverage, including table spans and chart series; one direction's writer test is not two-way coverage. |
| Rendering/export | `decks/src/canvas.rs`, `text_render.rs` and `export.rs` share slide drawing; Cairo PDF keeps text/vector drawing, and PNG and handouts use the same path. `export.rs::page_pt` uses source page dimensions when present. | Shared rendering prevents editor/export divergence but can share the same import/layout error. The old ADR's fixed 16×9 cm description is historical, not today's page-size implementation. PDF/handout export is not evidence that a printer-portal workflow is implemented. |
| Presenting | `presenter.rs`, `presenter_window.rs`, `builds.rs`, `magic_move.rs` and `transition.rs` implement current/next/notes/timer, rehearsal, selected-display audience fullscreen, builds and supported transitions. GDK monitor changes trigger visible fallback. | `presenter::missing_media` is only defined and unit-tested; there is no caller in `decks/`, and it inspects slide images rather than the full master/layout asset graph. Normal presenter-window placement is left to the compositor (`presenter_window.rs`), so two-monitor privacy/placement is not proved by the state model. |
| Loss/security | `loss.rs` detects selected comment/media/OLE/SmartArt/animation/missing-picture losses; `loss_ui.rs` asks before save. Package readers use `BoundedArchive`/`ZipBudget`. | `loss.rs` rescans the disk source rather than carrying complete imported package context, uses limited lexical feature detection, and returns an empty report on several scan failures. This is not complete fail-closed loss classification or general opaque preservation. |
| Native interaction | Context inspector, notes pane, slide strip, keyboard object traversal and `canvas_area.rs` accessible object children exist. | Presence of AT-SPI objects or a shortcut is not an end-to-end keyboard/screen-reader authoring result. Rich-text caret/range semantics, compound selection and multi-monitor operation remain separate acceptance work. |

Recent work is substantive but narrower than a release claim:

- [PR #1519](https://github.com/tuna-os/gtk-office-suite/pull/1519) preserves
  text-bearing shape frames and applies nested PPTX group placement/fill.
  That repairs import appearance; it does not introduce editable group objects.
- [PR #1530](https://github.com/tuna-os/gtk-office-suite/pull/1530) carries
  master/layout pictures, and
  [PR #1527](https://github.com/tuna-os/gtk-office-suite/pull/1527) adds table
  spans, PPTX round trips and ODP writer markup. Each is a useful vertical
  slice, not proof of the complete master or table feature.
- The 2026-10-13 issue snapshot for
  [#1397](https://github.com/tuna-os/gtk-office-suite/issues/1397) still reports
  the real pay-centre deck amber in tier A and red in tier B (73% versus 0%
  words found). Those reported measurements establish an unresolved real-file
  problem; they do not identify its cause without the report and a reduction.
- [PR #1503](https://github.com/tuna-os/gtk-office-suite/pull/1503) records
  installed Flatpak activation, portals, recent files, drag/drop and sandboxed
  save/reopen checks at `47041899`, run `37740903001`. Preserve that evidence;
  it is not a rerun against a future release candidate.

### Ordered delivery gates

These are dependency-ordered outcomes, not feature-count percentages. Every
implementation slice must include its relevant keyboard, accessibility, format
and rendering assertions; the final gate bundles that evidence rather than
postponing quality work until release.

#### Gate 1 — Safe documents and a measured fidelity envelope

1. Publish the supported PPTX/ODP read/edit/write/render matrix at the granularity
   users can lose: text/paragraph styles, page dimensions, master/layout assets,
   placeholders, shapes/groups, chart series, tables, notes, media, transitions
   and review content. Classify each as editable, safely preserved opaque,
   explicitly lossy, or blocked; reject a damaged/over-budget analysis as an
   error, not a clean bill of health.
2. Carry the source package identity and loss report through Save, Save As and
   recovery. Preserve unknown parts only with valid relationships and a defined
   edit dependency policy; otherwise explain the affected slides/content and
   require an explicit lossy-copy decision. Do not promise comments, SmartArt,
   unknown animation or media survives because a warning detector exists.
3. Keep bounded package/decompression/XML/image handling and no automatic
   execution or network fetching of embedded/linked content. Exercise malformed
   packages, missing assets, interrupted/failed writes, Cancel, recovery and
   external source changes; original bytes must remain safe on cancellation or
   failure. Wire complete asset/font preflight into present/export as well as save.
4. Reduce the real-document failures (starting with #1397 and master/theme,
   background-picture and text cases) to reusable fixtures before expanding
   the feature catalogue. Compare both render and reopened semantics, not just
   text extraction or package validity.

Exit evidence: licensed/owned source files and hashes, expected dispositions,
per-format semantic diffs, rendered slides and save-failure journeys tied to a
revision. No silent loss within the declared supported envelope.

#### Gate 2 — One expressive slide/master/theme model

1. Make document dimensions, asset ownership, stable object/placeholder/layout
   identities and the slide → layout → master → theme relationship explicit.
   Keep source values, inherited values and local overrides distinguishable;
   resolve them in one place used by canvas, thumbnails, presenter and export.
2. Retain theme color/font tokens and color-map overrides instead of only
   resolved RGB/one font. Add typed text/media/object placeholders, consistent
   reset/reapply behavior and master/layout reassignment without losing content.
   Preserve imported group transforms as a coherent editable hierarchy.
3. Extend the existing controller/ops/history rather than establishing parallel
   mutation paths. Slide, master, group, layout, text and asset edits must share
   revision/dirty/selection repair semantics; a gesture is one undo transaction.

Exit evidence: a custom-size deck with multiple masters/themes, nested groups,
inherited placeholder styling and deliberate local overrides survives edits,
undo/redo, PPTX/ODP round trips and rendering. A master/theme edit updates every
inheriting slide, without overwriting its local overrides.

#### Gate 3 — Dependable canvas authoring, first with real text

1. Replace the mismatched text overlay path with a correctly parented native
   editor using actual canvas transforms, caret/selection geometry and a
   synchronized styled-text model. Define multiline Enter, finish/cancel,
   focus loss, IME composition, bidi/CJK, clipboard and text undo behavior.
   Editing imported rich text must preserve unaffected runs/paragraph styles;
   inspector range formatting must not silently mean whole-box formatting.
2. Add click/Shift-click/marquee multi-selection, explicit object/text modes,
   keyboard selection and escape/focus rules. Connect existing core
   align/distribute operations to real multi-object controls. Add group/ungroup,
   layer ordering, locking, duplicate/copy/paste and selection repair.
3. Unify move/resize/rotate/crop and numeric inspector operations; maintain aspect
   ratio when requested, provide guides/snapping and alignment/distribution,
   and keep hit testing correct under zoom, rotation and display scaling.
   Cancelled gestures change neither the document nor history.

Exit evidence: author a multi-slide talk from blank, edit a mixed-style imported
paragraph, compose a grouped diagram, undo back to the start, redo, save/reopen
and continue editing. Repeat by keyboard and at narrow/wide sizes and different
scales. A fixture that only inserts a prefilled text box does not close this gate.

#### Gate 4 — Useful presentation-design workflows

1. Evolve the five starter decks and master-decoration editor into user-authored,
   reusable layouts/themes: create/duplicate/rename/delete layouts; edit tagged
   placeholders; preview/reset/reapply; safely reassign slides when deleting a
   layout. Export/import a reusable template without flattening all inheritance.
2. Provide named object/text styles, palette/font changes, image replacement
   that retains crop/geometry, reusable slide sections/layout variants and
   consistent spacing tools. Master decoration editing alone is not layout
   authoring; a color picker alone is not a theme system.
3. Provide original, licensed example designs for a report, a lecture and a
   visual pitch. Design previews must use the real renderer and remain editable.
   Keep the GNOME context inspector, searchable actions and progressive
   disclosure rather than cloning another application's ribbon or branding.

Exit evidence: a person makes a branded deck, changes its palette/type globally,
creates and reuses a custom image-and-caption layout in another deck, and
exports readable slides/handouts without manually repairing every slide.

#### Gate 5 — Editable charts, tables and media

1. Turn the existing single-series chart model/inspector into a declared
   business-chart subset with editable data, series/categories, axes/labels and
   styles. Until multi-series and newer chart kinds are supported, preserve or
   warn rather than silently treating the first series as the complete chart.
2. Prove table cell editing, merged cells, styles and both format directions;
   importing/drawing a table and inserting a sample 3×3 grid are not table
   authoring. Preserve charts as charts and tables as tables when supported.
3. Establish embedded/linked asset lifetime, missing-file repair, image
   format/vector fallback and crop/replacement behavior across save/recovery.
   Add audio/video only with model semantics, decoder/licensing review,
   sandbox-safe playback, poster/fallback behavior and show controls.
   An initial static-talk release may explicitly exclude playback, never
   silently discard its content on save.

Exit evidence: edit a chart's data and a merged table, replace/crop an image,
move/reopen the file and export/present it. Unsupported media/series must produce
a reproducible safe result. Codec breadth is not a substitute for these journeys.

#### Gate 6 — Trustworthy presenting and output

1. Preserve current/next/notes/timer, rehearsal, supported builds and transition
   preview; add a declared navigation contract for hidden slides, jumps,
   blank/unblank, restart/end, build backtracking and focus restoration.
   These are requirements, not claims that the present model already has them.
2. Verify explicit audience-display selection on real GNOME/Wayland hardware,
   differing scales/aspect ratios, disconnect/reconnect and one-screen fallback.
   The audience must never see notes or editing UI. Account for compositor
   placement constraints rather than trusting monitor indices alone.
3. Use the common renderer for PDF, PNG, handouts and a native print/portal
   workflow. Specify slide ranges, notes-page output, font substitution,
   source dimensions and failure behavior. Do not introduce a second layout
   engine merely to add printing.

Exit evidence: record a complete real two-display rehearsal/show, unplug/replug
the audience display, and publish slide/handout/notes-page PDF and print-path
results. Simulated monitor tests stay useful but do not replace hardware evidence.

#### Gate 7 — Native usability and release evidence

1. Complete keyboard-only creation/edit/save/present and screen-reader journeys:
   meaningful slide/object names, role/selection/focus changes, text caret/ranges,
   reading order, alt text, accessible chart/table information and error actions.
   Test IME, large text, high contrast, reduced motion and responsive inspectors.
   Core operations and AT-SPI snapshots alone do not establish usable interaction.
2. Freeze a licensed real-deck corpus plus authored feature/interaction fixtures.
   Score source import, edited output and reopen separately; cover more than
   title slides. Record fonts, app/reference versions, screenshots/PDFs,
   assertions and skipped/unavailable lanes. Retain the LibreOffice oracle but
   label its disagreements with Office instead of declaring either automatically
   correct.
3. Measure large-deck open/save, text-edit latency, thumbnail memory and show
   frame pacing on named hardware. Run the agreed release journeys on the exact
   installed Flatpak, including portals, MIME activation, recovery, font/media
   access and offline operation; attach revision/run/artifact identities.

Release decision: Gates 1–4 plus the admitted static subset of Gates 5–7 support
a bounded everyday presentation/design release. A broad PowerPoint-alternative
claim additionally requires evidence for imported business decks and their
charts/media/workflows, not completion of a checklist or an upstream percentage.

### Prior art, reuse and evidence policy

- **PowerPoint:** Microsoft's
  [master/layout/theme explanation](https://support.microsoft.com/en-us/office/what-is-a-slide-master-in-powerpoint-b9abb2a0-7aef-4257-a14e-4329c904da54)
  distinguishes global design, related layouts and slide-specific content.
  Use this as a behavioral contract for Gate 2/4, not a mandate to copy the UI.
  Use PresentationML/ECMA-376 and
  [MS-PPTX extension documentation](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-pptx/baa5ff29-d315-43ed-ac03-cf6c10baea59),
  alongside the
  [ODF 1.3 schema](https://docs.oasis-open.org/office/OpenDocument/v1.3/os/part3-schema/OpenDocument-v1.3-os-part3-schema.pdf),
  before reverse-engineering binaries.
- **Keynote:** Apple's
  [layout-authoring guide](https://support.apple.com/guide/keynote/add-and-edit-slide-layouts-tan7a2b69972/mac)
  distinguishes background decorations from editable tagged placeholders and
  describes creation, reassignment and reuse. That is a stronger design target
  than adding more built-in themes. Translate it into GTK/libadwaita interactions.
- **ArtCraft and DeckCraft are not interchangeable.**
  [ArtCraft](https://github.com/storytold/artcraft) describes controlled
  image/video composition and reusable assets; those workflows can inform
  editable visual composition without making AI services a prerequisite.
  Its sibling [DeckCraft](https://github.com/storytold/deckcraft) is directly
  relevant Rust presentation prior art. At inspected upstream revision
  `457406a7a5b62425e915926c8ccb0984e61a198c`, its
  [workspace](https://github.com/storytold/deckcraft/blob/457406a7a5b62425e915926c8ccb0984e61a198c/Cargo.toml)
  separates model, PPTX, engine, render and egui UI; its
  [inheritance resolver](https://github.com/storytold/deckcraft/blob/457406a7a5b62425e915926c8ccb0984e61a198c/crates/model/src/resolve.rs)
  is a concrete candidate for studying placeholder/theme precedence.
  First compare it on the same licensed real-deck corpus and authored fixtures,
  including ODP and export regressions. No source-copy recommendation is made
  before that comparison and provenance review; only then evaluate a narrow
  parser/model/geometry contribution. Do not replace GTK with egui or merge
  two engines.
  Its own [gap list](https://github.com/storytold/deckcraft/blob/457406a7a5b62425e915926c8ccb0984e61a198c/docs/gaps.md)
  says real-deck PPTX corpus coverage, ODP support, vector slide-artwork PDF and
  second-display placement are missing. Upstream feature breadth is therefore
  not evidence it solves Decks' release blockers.
- **Licensing is a gate, not a footnote.** DeckCraft's workspace and
  [MIT license](https://github.com/storytold/deckcraft/blob/457406a7a5b62425e915926c8ccb0984e61a198c/LICENSE-MIT)
  permit a scoped reuse path in this GPL-3.0-or-later project with required
  notices; its [NOTICE](https://github.com/storytold/deckcraft/blob/457406a7a5b62425e915926c8ccb0984e61a198c/NOTICE)
  excludes branding and identifies separately licensed fonts/dependencies.
  Verify per-file origin, dependencies, assets and any codec/patent obligations
  at a pinned revision; the repository's top-level license is not blanket
  permission to redistribute every bundled item.
- **Other OSS:** [LibreOffice Impress](https://www.libreoffice.org/) remains a
  strong external ODP/PPTX/render oracle and source of reduced interoperability
  cases; its [MPL-2.0 and component licensing](https://www.libreoffice.org/licenses/)
  require per-file review for ported code. Do not embed its entire office engine
  merely to avoid fixing the native model.
  [PptxGenJS](https://github.com/gitbrent/PptxGenJS) (MIT) and
  [python-pptx](https://github.com/scanny/python-pptx)
  ([MIT](https://github.com/scanny/python-pptx/blob/master/LICENSE)) are useful
  independently generated fixture/package references. Neither is a replacement
  interactive GTK renderer, and a file they generate still needs external
  rendering and semantic verification.
- **REA is optional investigation tooling, not a converter.**
  [REA](https://github.com/morluto/rea) exposes CLI/MCP analysis and
  evidence/provenance workflows; it supplies neither a PPTX/ODP engine nor
  PowerPoint/Keynote access, licenses, render fidelity or GTK integration.
  If public specs and authored files leave one concrete behavior uncertain,
  bound an authorized experiment to that question, record target/version,
  input hashes, actions, outputs and uncertainty, then make a clean regression
  fixture. Prefer public documentation, maintained OSS, package comparisons
  and permitted behavioral observations over proprietary binary analysis.
  Do not copy decompiled proprietary code or assets, upload private decks,
  install REA into the app, or make it a runtime/release dependency.
  [Issue #1534](https://github.com/tuna-os/gtk-office-suite/issues/1534)
  proposes revision-matched publisher Office PDFs and optional licensed
  reference runners; it explicitly remains a proposal, not an exercised
  PowerPoint oracle. This assessment exercised no proprietary target.
