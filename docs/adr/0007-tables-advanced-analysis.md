# ADR 0007 — Tables Advanced Analysis: IronCalc Integration, Dynamic Arrays, Pivot Tables, and Sheet Protection Strategy

> Renumbered from 0004 to 0007 on 2026-10-01 (#1205): 0004 was also taken by another ADR. The decision is unchanged.


Date: 2026-08-11 · Status: accepted

## Context

Issue #114 requires scoping advanced spreadsheet capabilities for Tables:
1. **Dynamic Arrays and Advanced Functions:** Strategy for integrating with IronCalc engine capability checks without reimplementing mature calculation machinery locally.
2. **Rich Charts:** Multi-series editing, axis labels, titles, legends, custom positioning, and OOXML/xlsx persistence.
3. **Pivot Tables:** Scope, data model, and interoperability verification against fixtures before GUI expansion.
4. **Sheet & Cell Protection:** Explicit protection model (locked/unlocked flags, passwords, permission flags) and loss policy during import/export.
5. **Recalculation and Performance Budgets:** Maintaining performance gates on large workbooks.

## Decision

### 1. IronCalc Engine Strategy & Dynamic Arrays

- **Engine Boundary:** All formula evaluation, dependency tracking, and calculation logic belong strictly to the underlying calculation engine (`ironcalc_base::Model` via `TablesEngine`). `tables-core` and the GTK GUI shell MUST NOT re-implement expression evaluation or array spilling locally.
- **Engine behavior:** The pinned IronCalc engine supports dynamic array formulas, including `SEQUENCE`.
- **Observed array behavior:** `TablesEngine` obtains calculated values for spill cells through its normal grid projection, limited to its configured row and column bounds. A non-empty destination blocks the spill and yields `#SPILL!`. Function-by-function compatibility is not implied; confirm engine behavior with a regression test before documenting a function as supported.
- **Capability accounting:** Track supported formula behavior through executable engine tests and the function catalog; do not claim a runtime capability query unless the engine exposes one.

### 2. Rich Charts Model & Persistence

- **Extended `ChartSpec` Model:**
  - `kind`: `Bar`, `Line`, `Pie`, `Scatter`, `Area`.
  - `title`: Optional custom title string.
  - `x_axis_title` / `y_axis_title`: Custom axis title strings.
  - `legend_position`: `None`, `Top`, `Bottom`, `Left`, `Right`.
  - `series`: Vector of `ChartSeries` (`name`, `cat_range`, `val_range`, `color`).
  - `anchor` & dimensions: Position `(row, col)` and sizing `(width_px, height_px)` for flexible canvas placement.
- **XLSX Persistence target:** preserve multi-series, axes, legends, worksheet ownership and chart placement through load/edit/save. The writer in `tables-core/src/io/save.rs` supports more fields than the reader in `tables-core/src/io/charts.rs`; this is not yet a complete round trip.

### 3. Pivot Table Model & Fixture Verification

- **Data Model (`PivotTableSpec`):**
  - `name`: Pivot table identifier.
  - `source_range`: `(top, left, bottom, right)` data source.
  - `target_cell`: `(row, col)` location for output.
  - `row_fields`: Vector of column indices for row grouping.
  - `col_fields`: Vector of column indices for column grouping.
  - `data_fields`: Vector of `(col_index, AggregationFunc)` (Sum, Count, Average, Min, Max).
  - `filter_fields`: Vector of column indices for filtering.
- **Fixtures & Interoperability:** `tables-core/tests/pivot_and_protection_tests.rs` verifies row aggregation, column-field cross-tab output, and placement. This does not establish GUI availability or XLSX pivot-definition round-tripping.

### 4. Sheet and Cell Protection Model

- **Cell Protection (`CellProtection`):**
  - `locked: bool` (defaults to `true`, matching Excel/Calc defaults).
  - `hidden_formula: bool` (hides formula string when protected).
- **Sheet Protection (`SheetProtection`):**
  - `protected: bool`.
  - `password_hash: Option<String>`: an opaque imported value, not yet a typed or verifiable credential (TBL-005 part 2).
  - One `allow_*` per OOXML `<sheetProtection>` permission (selection, format cells/columns/rows, insert/delete columns/rows, hyperlinks, sort, filters, pivot tables, objects, scenarios); `tables-core/src/io/sheet_protection.rs` maps each to its attribute and ECMA-376 default.
- **Current enforcement (TBL-005 part 1):** the controller authorizes every user operation group before it runs. Locked cells refuse edits, and each modeled permission governs its own operations; delete and sort permissions never bypass locked cells. Selection permissions apply to mouse, keyboard and name-box selection, and a cell that cannot be selected cannot be edited. A protected hidden formula shows its value, but the formula bar, clipboard and fill sources withhold its text. Changing protection is one undo step through the Sheet Protection dialog.
- **Persistence:** cell locks and hidden flags, plus every permission (also on unprotected sheets), round-trip through XLSX; ODS import reads Calc's sheet and cell protection. Credentials, Allow Edit Ranges and workbook structure protection warn before a save drops them. Sheet protection is not encryption and is not a security boundary.

### 5. Recalculation & Large-Workbook Budget

- The earlier <100ms/10,000-cell statement is not the implemented gate: there is no `tables-core/benches` directory. `tables-core/tests/performance_budgets.rs` enforces a two-second timing budget, with a recalculation fixture containing 256 input cells and 256 formulas. Extend these existing gates with reproducible dense/sparse whole-workbook workloads, identified hardware and memory measurements; measure loading, recalculation, editing and saving separately from viewport rendering.

## Consequences

- Clean architectural split preserved: calculation stays inside `ironcalc_base`, presentation in `tables` GTK shell.
- Current implementation has scoped but incomplete interoperability: pivot output is written as ordinary cells and does not round-trip as an OOXML pivot definition; chart and protection coverage remains limited to implemented formats and tested cases.

## Delivery priority

The [Tables readiness roadmap](../readiness-2026-09/tables-readiness.md#direction-and-delivery-order)
orders implementation: safe/fidelity-preserving edits and saves, one calculation
authority, coherent native interaction, persisted analysis features, then
exact-revision packaged daily-driver evidence. Reuse IronCalc APIs before
building parallel logic; assess its XLSX adapter against the existing corpus
before replacing the current file-I/O stack. REA and Excel/Numbers observations
are optional research inputs, not implementation dependencies or parity proof.
