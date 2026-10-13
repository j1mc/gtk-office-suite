// SPDX-License-Identifier: GPL-3.0-or-later
//! User authorization, deliberately separate from trusted op replay/undo.

use super::core::WorkbookController;
use super::ops::{Axis, Op, SheetProp};
use super::state::WorkbookState;
use crate::sheet::SheetModel;

fn locked(sheet: &SheetModel, row: usize, col: usize) -> bool {
    sheet.protection.protected
        && sheet.cell_protections.get(row).and_then(|r| r.get(col)).is_some_and(|cell| cell.locked)
}

fn refuses_op(state: &WorkbookState, op: &Op) -> bool {
    // Relinks are normally produced by undo, but public user op groups must
    // not use their restoration payload to write a protected cell elsewhere.
    let relinks = match op {
        Op::Insert { relinks, .. } | Op::AddSheet { relinks, .. } => relinks.as_slice(),
        _ => &[],
    };
    if relinks.iter().any(|(id, row, col, _)| {
        state.sheet_index_for_id(*id).is_some_and(|i| locked(&state.sheets[i].borrow(), *row, *col))
    }) {
        return true;
    }
    let Some(index) = op.sheet().and_then(|id| state.sheet_index_for_id(id)) else { return false };
    let sheet = state.sheets[index].borrow();
    if !sheet.protection.protected {
        return false;
    }
    match op {
        Op::SetCell { row, col, .. }
        | Op::SetValidation { row, col, .. }
        | Op::SetNote { row, col, .. } => locked(&sheet, *row, *col),
        Op::SetCells { cells, .. } => cells.iter().any(|(row, col, _)| locked(&sheet, *row, *col)),
        Op::SetFormat { row, col, .. }
        | Op::SetStyle { row, col, .. }
        | Op::SetBorder { row, col, .. } => !sheet.protection.allow_format_cells && locked(&sheet, *row, *col),
        Op::Insert { axis: Axis::Rows, .. } => !sheet.protection.allow_insert_rows,
        Op::Delete { axis: Axis::Rows, .. } => !sheet.protection.allow_delete_rows,
        // Unprotect is explicit; it does not authorize any other op in this
        // group. Authorization uses the state before the entire command.
        Op::SetProp { prop: SheetProp::Protection(_), .. } => false,
        // Pivot registration accompanies its output SetCells, checked above.
        Op::SetProp { prop: SheetProp::Pivots(_), .. } => false,
        Op::SetProp { prop: SheetProp::Sorted(_), .. } => {
            (sheet.frozen_rows.min(sheet.rows)..sheet.rows)
                .any(|row| (0..sheet.cols).any(|col| locked(&sheet, row, col)))
        }
        // No column/layout permissions are modeled. These edits, and changing
        // locks themselves, conservatively require unprotecting first.
        Op::SetLocked { .. }
        | Op::Insert { .. }
        | Op::Delete { .. }
        | Op::Merge { .. }
        | Op::Unmerge { .. }
        | Op::Resize { .. }
        | Op::Freeze { .. } => true,
        // Sheet protection is not workbook structure protection or a blanket
        // prohibition on view/filter/print settings.
        Op::DeleteSheet { .. }
        | Op::RenameSheet { .. }
        | Op::MoveSheet { .. }
        | Op::SetProp { .. }
        | Op::AddSheet { .. }
        | Op::DefineName { .. } => false,
    }
}

impl WorkbookController {
    /// Preflight the complete group without executing even its first op.
    pub(super) fn authorize_ops(&mut self, ops: &[Op]) -> bool {
        let refused = {
            let state = self.state.borrow();
            ops.iter().any(|op| refuses_op(&state, op))
        };
        if refused {
            self.protection_rejections = self.protection_rejections.wrapping_add(1);
        }
        !refused
    }

    /// Check intended destinations before no-op filtering or formula
    /// translation. A compound selection containing a locked cell is refused
    /// as a whole, even if that cell already contains the requested value.
    pub(super) fn authorize_cells(&mut self, sheet_id: u32, cells: impl IntoIterator<Item = (usize, usize)>) -> bool {
        let refused = {
            let state = self.state.borrow();
            state.sheet_index_for_id(sheet_id).is_some_and(|index| {
                let sheet = state.sheets[index].borrow();
                sheet.protection.protected && cells.into_iter().any(|(row, col)| locked(&sheet, row, col))
            })
        };
        if refused {
            self.protection_rejections = self.protection_rejections.wrapping_add(1);
        }
        !refused
    }

    pub(super) fn authorize_format_cells(&mut self, sheet_id: u32, cells: impl IntoIterator<Item = (usize, usize)>) -> bool {
        let allowed = {
            let state = self.state.borrow();
            state.sheet_index_for_id(sheet_id)
                .is_some_and(|index| state.sheets[index].borrow().protection.allow_format_cells)
        };
        allowed || self.authorize_cells(sheet_id, cells)
    }
}
