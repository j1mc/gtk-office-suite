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
- **XLSX Persistence:** Extended DrawingML chart XML generation in `tables-core/src/io.rs` for multi-series, axis titles, legends, and chart types.

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
  - `password_hash: Option<String>` (SHA-256 or legacy OOXML hash).
  - `allow_select_locked: bool`, `allow_select_unlocked: bool`, `allow_format_cells: bool`, `allow_insert_rows: bool`, `allow_delete_rows: bool`, etc.
- **Enforcement Policy:** When `sheet.protection.protected` is `true`, `WorkbookController` enforces edit checks: attempts to mutate locked cells return `Err("Sheet is protected")`.
- **Loss Policy & Warnings:** Full round-trip preservation into XLSX `<sheetProtection>` elements. Unsupported protection features emit explicit user warnings rather than silent data loss.

### 5. Recalculation & Large-Workbook Budget

- Large workbook benchmark targets (< 100ms for 10,000 cells recalculation) are enforced in `tables-core/benches` and integration test gates.

## Consequences

- Clean architectural split preserved: calculation stays inside `ironcalc_base`, presentation in `tables` GTK shell.
- Current implementation has scoped but incomplete interoperability: pivot output is written as ordinary cells and does not round-trip as an OOXML pivot definition; chart and protection coverage remains limited to implemented formats and tested cases.
