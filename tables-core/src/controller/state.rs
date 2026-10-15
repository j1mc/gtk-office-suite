use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;


use crate::engine::TablesEngine;
use crate::sheet::SheetModel;

/// Canonical mutable state for a Tables window.
///
/// GTK views may observe this state, but edits must go through
/// [`crate::controller::WorkbookController`] so the calculation engine, rendered sheet, and undo
/// history cannot drift apart.
pub struct WorkbookState {
    pub sheets: Vec<Rc<RefCell<SheetModel>>>,
    pub active_sheet: usize,
    pub engine: TablesEngine,
}

impl WorkbookState {
    pub fn new(rows: usize, cols: usize) -> Result<Self, String> {
        let engine = TablesEngine::new(rows, cols)?;
        let sheet_id = engine.sheet_id_at(0).unwrap_or(0);
        let sheet = SheetModel::new("Sheet1", rows, cols, sheet_id);
        Ok(Self {
            sheets: vec![Rc::new(RefCell::new(sheet))],
            active_sheet: 0,
            engine,
        })
    }

    /// Resolve a stable sheet identity to its current position. Sheets are
    /// never deleted out from under a live GUI reference without also being
    /// dropped from `sheets`, so this stays in lockstep with the engine.
    pub fn sheet_index_for_id(&self, sheet_id: u32) -> Option<usize> {
        self.sheets
            .iter()
            .position(|sheet| sheet.borrow().sheet_id == sheet_id)
    }

    pub fn sheet(&self) -> Ref<'_, SheetModel> {
        self.sheets[self.active_sheet].borrow()
    }

    pub fn sheet_mut(&self) -> RefMut<'_, SheetModel> {
        self.sheets[self.active_sheet].borrow_mut()
    }

    /// The editable input for a cell on the active sheet: its formula with
    /// the leading `=`, else the text it was typed as (`""` when empty).
    /// Setting this back recreates the cell. This is what the formula bar,
    /// copy, undo and save use — never the displayed value below, which
    /// would flatten a formula into its current result.
    pub fn cell_input(&self, row: usize, col: usize) -> String {
        self.cell_input_at(self.active_sheet, row, col)
    }

    /// [`Self::cell_input`] on `sheet` instead of the active one.
    pub fn cell_input_at(&self, sheet: usize, row: usize, col: usize) -> String {
        self.engine.editable_input_at(sheet, row, col)
    }

    /// The cell's displayed value on the active sheet: read-only
    /// projection text for render and stats. Not editable, not committable.
    pub fn displayed_value(&self, row: usize, col: usize) -> String {
        self.displayed_value_at(self.active_sheet, row, col)
    }

    /// [`Self::displayed_value`] on `sheet` instead of the active one.
    pub fn displayed_value_at(&self, sheet: usize, row: usize, col: usize) -> String {
        self.engine.cell_at(sheet, row, col)
    }

    /// Write one cell through the engine and refresh every sheet's
    /// projection in the same step: a recalculation can change dependents
    /// anywhere, and their projections must follow before any consumer
    /// reads them — not when their sheet is next visited. An upstream
    /// rejection is an error, never a silent success.
    pub(super) fn set_cell_input(&mut self, row: usize, col: usize, input: &str) -> Result<(), String> {
        self.engine.set_cell_text(row, col, input)?;
        self.sync_all_sheets();
        Ok(())
    }

    /// [`Self::set_cell_input`] for many cells with a single calculation
    /// boundary: one `evaluate` for the batch, then one refresh of every
    /// affected projection. All or nothing — see
    /// `TablesEngine::put_cells_on_sheet`.
    pub(super) fn set_cell_inputs<'a>(&mut self, inputs: impl IntoIterator<Item = (usize, usize, &'a str)>) -> Result<(), String> {
        let sheet = self.active_sheet;
        self.engine.put_cells_on_sheet(sheet, inputs)?;
        self.sync_all_sheets();
        Ok(())
    }

    /// [`Self::set_cell_input`] on `sheet`, restoring the previously
    /// active sheet afterwards. A bad `sheet` is an error, not a panic.
    pub(super) fn set_cell_input_on_sheet(&mut self, sheet: usize, row: usize, col: usize, input: &str) -> Result<(), String> {
        let previous = self.active_sheet;
        self.engine
            .set_active_sheet(sheet)
            .map_err(|e| format!("no sheet {sheet}: {e}"))?;
        self.active_sheet = sheet;
        let result = self.set_cell_input(row, col, input);
        // The write rolled itself back on error; still restore the active
        // sheet and refresh, so a failure changes nothing observable.
        self.engine
            .set_active_sheet(previous)
            .map_err(|e| format!("no sheet {previous}: {e}"))?;
        self.active_sheet = previous;
        result?;
        Ok(())
    }

    /// [`Self::set_cell_inputs`] on `sheet`, restoring the previously
    /// active sheet afterwards. A bad `sheet` is an error, not a panic.
    pub(super) fn set_cell_inputs_on_sheet<'a>(
        &mut self,
        sheet: usize,
        inputs: impl IntoIterator<Item = (usize, usize, &'a str)>,
    ) -> Result<(), String> {
        let previous = self.active_sheet;
        self.engine
            .set_active_sheet(sheet)
            .map_err(|e| format!("no sheet {sheet}: {e}"))?;
        self.active_sheet = sheet;
        let result = self.set_cell_inputs(inputs);
        self.engine
            .set_active_sheet(previous)
            .map_err(|e| format!("no sheet {previous}: {e}"))?;
        self.active_sheet = previous;
        result?;
        Ok(())
    }

    pub fn add_sheet(&mut self, name: String, rows: usize, cols: usize) -> Result<usize, String> {
        let index = self.engine.add_sheet(&name)?;
        let sheet_id = self.engine.sheet_id_at(index).unwrap_or(index as u32);
        self.sheets.push(Rc::new(RefCell::new(SheetModel::new(
            &name, rows, cols, sheet_id,
        ))));
        Ok(index)
    }

    pub fn switch_sheet(&mut self, index: usize) -> Result<(), String> {
        if index >= self.sheets.len() {
            return Err(format!("Sheet index {index} does not exist"));
        }
        self.engine.set_active_sheet(index)?;
        self.active_sheet = index;
        self.sync_active_sheet();
        Ok(())
    }

    /// Rename a sheet in both the engine and its live presentation model.
    pub fn rename_sheet(&mut self, index: usize, name: &str) -> Result<(), String> {
        if index >= self.sheets.len() {
            return Err(format!("Sheet index {index} does not exist"));
        }
        self.engine.rename_sheet(index, name)?;
        self.sheets[index].borrow_mut().name = name.to_string();
        Ok(())
    }

    /// Delete a sheet. Fails if it is the only sheet. Undo history entries
    /// that targeted the deleted sheet become inert no-ops rather than
    /// corrupting a different sheet (they resolve by sheet_id, and the id
    /// no longer exists).
    pub fn delete_sheet(&mut self, index: usize) -> Result<(), String> {
        if index >= self.sheets.len() {
            return Err(format!("Sheet index {index} does not exist"));
        }
        self.engine.delete_sheet(index)?;
        self.sheets.remove(index);
        self.active_sheet = self.engine.active_sheet();
        self.sync_active_sheet();
        Ok(())
    }

    /// Reorder sheets. `new_order` must be a permutation of current indices.
    pub fn reorder_sheets(&mut self, new_order: &[usize]) -> Result<(), String> {
        self.engine.reorder_sheets(new_order)?;
        self.sheets = new_order.iter().map(|&i| self.sheets[i].clone()).collect();
        self.active_sheet = self.engine.active_sheet();
        Ok(())
    }

    /// The engine reads every sheet through one grid size: keep it at
    /// least as big as the biggest sheet, or a grown sheet reads blanks past
    /// the old edge.
    pub fn grow_engine(&mut self) {
        for s in &self.sheets {
            let s = s.borrow();
            self.engine.rows = self.engine.rows.max(s.rows);
            self.engine.cols = self.engine.cols.max(s.cols);
        }
    }

    pub(super) fn sync_active_sheet(&mut self) {
        let active = self.active_sheet;
        self.sync_sheet(active);
    }

    /// Re-read one sheet's projection from the engine without disturbing
    /// the active sheet: the read is sheet-indexed, so no switching is
    /// needed and none happens.
    pub(super) fn sync_sheet(&mut self, sheet: usize) {
        if let Some(model) = self.sheets.get(sheet) {
            model.borrow_mut().sync_from_engine_at(&self.engine, sheet);
        }
    }

    /// Re-read every sheet's projection from the engine. Every engine
    /// write in this file ends here, in the same step: dependents on
    /// unvisited sheets follow their precedents before any consumer —
    /// render, copy, save, collab diff — can read a stale value. Grows
    /// the engine first so a grown sheet never reads blanks past the
    /// old edge.
    pub(super) fn sync_all_sheets(&mut self) {
        self.grow_engine();
        for sheet in 0..self.sheets.len() {
            self.sync_sheet(sheet);
        }
    }
}
