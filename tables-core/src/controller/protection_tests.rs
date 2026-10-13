// SPDX-License-Identifier: GPL-3.0-or-later

use super::ops::{apply_all, SheetProp};
use super::{Axis, BorderPreset, Op, WorkbookController, WorkbookImage};
use crate::fragment::{Fragment, GridCell};
use crate::sheet::{BorderStyle, PivotAggFunc, PivotDataField, PivotField, PivotTableSpec, SortDirection};

fn grid(values: &[&[&str]]) -> Fragment {
    Fragment::Grid(values.iter().map(|row| row.iter().map(|value| GridCell {
        value: (*value).into(), formula: None, num_format: None,
    }).collect()).collect())
}

fn select(controller: &WorkbookController, range: (usize, usize, usize, usize)) {
    let state = controller.state.borrow();
    let mut sheet = state.sheet_mut();
    (sheet.selected_row, sheet.selected_col, sheet.sel_end_row, sheet.sel_end_col) = range;
}

fn mixed_sheet(later: bool) -> WorkbookController {
    let mut controller = WorkbookController::new(8, 4).unwrap();
    if later {
        let mut state = controller.state.borrow_mut();
        state.add_sheet("Later".into(), 8, 4).unwrap();
        state.switch_sheet(1).unwrap();
    }
    controller.edit_cell(0, 0, "=10+2");
    controller.edit_cell(1, 0, "2");
    controller.edit_cell(2, 0, "1");
    controller.set_cell_locked(1, 0, false);
    controller.mutate_sheet("Source metadata", |sheet| {
        sheet.styles[0][0].bold = true;
        sheet.notes[2][0] = Some("Keep this note".into());
    });
    controller.set_sheet_protection(true, None);
    // Every rejection below must preserve a nonempty redo stack and a clean
    // dirty flag, not merely avoid adding a new undo description.
    controller.edit_cell(1, 0, "redo sentinel");
    assert!(controller.undo());
    controller.mark_clean();
    controller
}

fn assert_rejected(controller: &mut WorkbookController, operation: impl FnOnce(&mut WorkbookController)) {
    let before = WorkbookImage::of(&controller.state.borrow());
    let model_values: Vec<_> = controller.state.borrow().sheets.iter().map(|s| s.borrow().data.clone()).collect();
    let active = controller.state.borrow().active_sheet;
    let history = (controller.can_undo(), controller.can_redo(),
        controller.undo_description().map(str::to_owned), controller.redo_description().map(str::to_owned));
    let dirty = controller.is_dirty();
    let count = controller.protection_rejection_count();
    operation(controller);
    assert_eq!(controller.protection_rejection_count(), count + 1);
    assert_eq!(WorkbookImage::of(&controller.state.borrow()), before);
    assert_eq!(controller.state.borrow().active_sheet, active);
    for (sheet, values) in controller.state.borrow().sheets.iter().zip(model_values) {
        assert_eq!(sheet.borrow().data, values, "presentation values must agree with the untouched engine");
    }
    assert_eq!((controller.can_undo(), controller.can_redo(),
        controller.undo_description().map(str::to_owned), controller.redo_description().map(str::to_owned)), history);
    assert_eq!(controller.is_dirty(), dirty);
}

#[test]
fn mixed_paste_fill_and_clear_are_atomic_on_first_and_later_sheets() {
    for later in [false, true] {
        let mut controller = mixed_sheet(later);
        assert_rejected(&mut controller, |c| c.paste_fragment(1, 0, &grid(&[&["new"], &["1"]])));
        // The locked destination above already equals its pasted value: it
        // must not disappear from authorization when no-op cells are filtered.
        assert_rejected(&mut controller, |c| c.fill((0, 0, 0, 0), 2, 0));
        select(&controller, (1, 0, 2, 0));
        assert_rejected(&mut controller, |c| c.clear_selection());
        assert!(controller.redo(), "the rejected commands did not consume redo");
        assert_eq!(controller.state.borrow().sheet().cell(1, 0), "redo sentinel");
    }
}

#[test]
fn mixed_format_and_note_operations_are_atomic() {
    let mut controller = mixed_sheet(false);
    select(&controller, (1, 0, 2, 0));
    assert_rejected(&mut controller, |c| c.format_selection("Bold", |style| style.bold = true));
    assert_rejected(&mut controller, |c| c.border_selection(BorderPreset::All, BorderStyle::Solid, (0.0, 0.0, 0.0)));
    assert_rejected(&mut controller, |c| { c.set_note(2, 0, Some("overwrite".into())); });
    assert_rejected(&mut controller, |c| c.mutate_sheet("Compound metadata", |sheet| {
        sheet.styles[1][0].italic = true;
        sheet.notes[2][0] = None;
    }));
}

#[test]
fn protected_sort_structure_and_lock_changes_are_atomic() {
    let mut controller = mixed_sheet(true);
    assert_rejected(&mut controller, |c| c.sort_column(0, SortDirection::Ascending));
    for axis in [Axis::Rows, Axis::Cols] {
        assert_rejected(&mut controller, |c| c.insert_lines(axis, 1, 1));
        assert_rejected(&mut controller, |c| c.delete_lines(axis, 1, 1));
    }
    assert_rejected(&mut controller, |c| c.toggle_merge());
    assert_rejected(&mut controller, |c| c.mutate_sheet("Freeze", |sheet| sheet.frozen_rows = 1));
    assert_rejected(&mut controller, |c| c.set_cell_locked(2, 0, false));
    assert_rejected(&mut controller, |c| { assert!(!c.begin_resize()); });
    let old = controller.state.borrow().sheet().col_widths[0];
    assert_rejected(&mut controller, |c| {
        // Legacy drag previews may already have changed the model by commit.
        c.state.borrow().sheet_mut().col_widths[0] = old + 20.0;
        c.record_resize(Axis::Cols, 0, old);
    });
}

#[test]
fn whole_group_is_checked_before_unprotect_or_later_sheet_write() {
    let mut controller = mixed_sheet(true);
    controller.state.borrow_mut().switch_sheet(0).unwrap();
    let (first, later, unprotected) = {
        let state = controller.state.borrow();
        let later = state.sheets[1].borrow();
        let mut protection = later.protection.clone();
        protection.protected = false;
        let first = state.sheets[0].borrow().sheet_id;
        (first, later.sheet_id, protection)
    };
    assert_rejected(&mut controller, |c| {
        assert!(!c.apply_ops("Compound write", vec![
            Op::SetCell { sheet: first, row: 0, col: 0, input: "must not land".into() },
            Op::SetProp { sheet: later, prop: SheetProp::Protection(unprotected) },
            Op::SetCell { sheet: later, row: 2, col: 0, input: "locked".into() },
        ]));
    });
    assert_rejected(&mut controller, |c| {
        assert!(!c.apply_ops("Raw batch", vec![Op::SetCells {
            sheet: later,
            cells: vec![(1, 0, "unlocked first".into()), (2, 0, "locked last".into())],
        }]));
    });
    assert_rejected(&mut controller, |c| {
        let lines = super::blank_lines(&c.state.borrow(), 0, Axis::Rows, 1);
        assert!(!c.apply_ops("Restoration payload is not a user bypass", vec![Op::Insert {
            sheet: first, axis: Axis::Rows, at: 0,
            lines,
            relinks: vec![(later, 2, 0, "locked relink".into())], merges: None,
        }]));
    });
}

#[test]
fn unlocked_compound_edits_remain_available_and_undoable() {
    let mut controller = WorkbookController::new(4, 2).unwrap();
    controller.edit_cell(0, 0, "3");
    controller.mutate_sheet("Unlock inputs", |sheet| {
        for row in &mut sheet.cell_protections {
            for cell in row { cell.locked = false; }
        }
    });
    controller.set_sheet_protection(true, None);
    let before = WorkbookImage::of(&controller.state.borrow());
    let count = controller.protection_rejection_count();
    controller.paste_fragment(1, 0, &grid(&[&["2"], &["1"]]));
    controller.fill((0, 0, 0, 0), 0, 1);
    select(&controller, (1, 0, 2, 0));
    controller.format_selection("Bold", |style| style.bold = true);
    controller.border_selection(BorderPreset::All, BorderStyle::Solid, (0.0, 0.0, 0.0));
    assert!(controller.set_note(1, 0, Some("Editable".into())));
    controller.sort_column(0, SortDirection::Ascending);
    assert_eq!(controller.state.borrow().sheet().cell(0, 0), "1");
    controller.clear_selection();
    assert_eq!(controller.state.borrow().sheet().cell(1, 0), "");
    assert_eq!(controller.protection_rejection_count(), count);
    for _ in 0..7 { assert!(controller.undo()); }
    assert_eq!(WorkbookImage::of(&controller.state.borrow()), before);
    for _ in 0..7 { assert!(controller.redo()); }
    assert!(controller.sheet_is_protected());
}

#[test]
fn pivot_rejection_keeps_registration_and_output_atomic_on_later_sheet() {
    let mut controller = mixed_sheet(true);
    controller.set_sheet_protection(false, None);
    controller.edit_cell(0, 0, "Region");
    controller.edit_cell(0, 1, "Sales");
    controller.edit_cell(1, 0, "North");
    controller.edit_cell(1, 1, "10");
    for (row, col) in [(4, 0), (4, 1), (5, 0)] { controller.set_cell_locked(row, col, false); }
    controller.set_sheet_protection(true, None);
    controller.mark_clean();
    let pivot = PivotTableSpec {
        name: "Protected summary".into(), source_range: (0, 0, 1, 1), target_cell: (4, 0),
        row_fields: vec![PivotField { col_index: 0, name: "Region".into() }], col_fields: vec![],
        data_fields: vec![PivotDataField { col_index: 1, name: "Sales".into(), func: PivotAggFunc::Sum }],
    };
    assert_rejected(&mut controller, |c| c.add_pivot_table(pivot.clone()));
    controller.set_sheet_protection(false, None);
    controller.set_cell_locked(5, 1, false);
    controller.set_sheet_protection(true, None);
    let before = WorkbookImage::of(&controller.state.borrow());
    controller.add_pivot_table(pivot);
    assert_eq!(controller.state.borrow().sheet().cell(5, 1), "10");
    assert_eq!(controller.state.borrow().sheet().pivot_tables.len(), 1);
    assert!(controller.undo());
    assert_eq!(WorkbookImage::of(&controller.state.borrow()), before);
    assert!(controller.redo());
    assert_eq!(controller.state.borrow().sheet().cell(5, 1), "10");
}

#[test]
fn trusted_replay_and_undo_restore_authorized_locked_cells() {
    let mut controller = WorkbookController::new(2, 2).unwrap();
    let (sheet, protected) = {
        let state = controller.state.borrow();
        let sheet = state.sheet();
        let mut protection = sheet.protection.clone();
        protection.protected = true;
        (sheet.sheet_id, protection)
    };
    // Authorized against the initially unprotected sheet. Its inverse writes
    // the cell before undoing Protection, so an executor-level guard is wrong.
    assert!(controller.apply_ops("Protect and initialize", vec![
        Op::SetProp { sheet, prop: SheetProp::Protection(protected) },
        Op::SetCell { sheet, row: 0, col: 0, input: "authorized".into() },
    ]));
    assert!(controller.sheet_is_protected());
    assert!(controller.undo());
    assert_eq!(controller.state.borrow().sheet().cell(0, 0), "");
    assert!(controller.redo());
    assert_eq!(controller.state.borrow().sheet().cell(0, 0), "authorized");
    let before = WorkbookImage::of(&controller.state.borrow());
    let inverse = apply_all(&mut controller.state.borrow_mut(), &[
        Op::SetCell { sheet, row: 0, col: 0, input: "recovered".into() },
    ]).unwrap();
    assert_eq!(controller.state.borrow().sheet().cell(0, 0), "recovered");
    apply_all(&mut controller.state.borrow_mut(), &inverse).unwrap();
    assert_eq!(WorkbookImage::of(&controller.state.borrow()), before);
    assert_eq!(controller.protection_rejection_count(), 0);
}

#[test]
fn rejection_feedback_does_not_leak_into_later_noops_or_unlocked_edits() {
    let mut controller = mixed_sheet(false);
    assert_rejected(&mut controller, |c| c.edit_cell(2, 0, "refused"));
    let count = controller.protection_rejection_count();
    controller.edit_cell(1, 0, "2"); // unchanged, returns before apply_ops
    controller.fill((1, 0, 1, 0), 1, 0); // no fill direction
    controller.paste_fragment(99, 99, &grid(&[&["outside"]]));
    assert_eq!(controller.protection_rejection_count(), count);
    controller.edit_cell(1, 0, "allowed");
    assert_eq!(controller.protection_rejection_count(), count);
    assert_eq!(controller.state.borrow().sheet().cell(1, 0), "allowed");
}

#[test]
fn modeled_format_permission_allows_formatting_but_not_locked_values() {
    let mut controller = mixed_sheet(false);
    controller.mutate_sheet("Allow formatting", |sheet| sheet.protection.allow_format_cells = true);
    select(&controller, (1, 0, 2, 0));
    let count = controller.protection_rejection_count();
    controller.format_selection("Bold", |style| style.bold = true);
    controller.border_selection(BorderPreset::All, BorderStyle::Solid, (0.0, 0.0, 0.0));
    assert!(controller.state.borrow().sheet().styles[2][0].bold);
    assert!(!controller.state.borrow().sheet().borders[2][0].is_none());
    assert_eq!(controller.protection_rejection_count(), count);
    assert_rejected(&mut controller, |c| c.edit_cell(2, 0, "still locked"));
    assert_rejected(&mut controller, |c| c.clear_selection());
    assert_rejected(&mut controller, |c| c.set_cell_locked(2, 0, false));
}

#[test]
fn modeled_row_permissions_are_independent_and_do_not_allow_columns() {
    let mut controller = mixed_sheet(true);
    controller.mutate_sheet("Allow row insertion", |sheet| sheet.protection.allow_insert_rows = true);
    let count = controller.protection_rejection_count();
    controller.insert_lines(Axis::Rows, 1, 1);
    assert_eq!(controller.state.borrow().sheet().rows, 9);
    assert_eq!(controller.state.borrow().sheet().cell(3, 0), "1");
    assert_eq!(controller.protection_rejection_count(), count);
    assert_rejected(&mut controller, |c| c.delete_lines(Axis::Rows, 1, 1));
    assert_rejected(&mut controller, |c| c.insert_lines(Axis::Cols, 1, 1));
    assert_rejected(&mut controller, |c| c.edit_cell(3, 0, "still locked"));
    controller.mutate_sheet("Allow only row deletion", |sheet| {
        sheet.protection.allow_insert_rows = false;
        sheet.protection.allow_delete_rows = true;
    });
    let before = WorkbookImage::of(&controller.state.borrow());
    let count = controller.protection_rejection_count();
    controller.delete_lines(Axis::Rows, 1, 1);
    assert_eq!(controller.state.borrow().sheet().rows, 8);
    assert_eq!(controller.state.borrow().sheet().cell(2, 0), "1");
    assert_eq!(controller.protection_rejection_count(), count);
    assert_rejected(&mut controller, |c| c.insert_lines(Axis::Rows, 1, 1));
    assert_rejected(&mut controller, |c| c.delete_lines(Axis::Cols, 1, 1));
    // Deletion's inverse inserts rows despite the current insert permission.
    assert!(controller.undo());
    assert_eq!(WorkbookImage::of(&controller.state.borrow()), before);
    assert!(controller.redo());
    assert_eq!(controller.state.borrow().sheet().rows, 8);
}

#[test]
fn sheet_protection_does_not_become_workbook_or_view_protection() {
    let mut controller = mixed_sheet(true);
    let count = controller.protection_rejection_count();
    controller.filter_by_value(0, "2");
    assert!(!controller.state.borrow().sheet().hidden_rows.is_empty());
    controller.clear_filter();
    controller.set_print_area((0, 0, 2, 1));
    assert_eq!(controller.state.borrow().sheet().print_area, Some((0, 0, 2, 1)));
    assert!(controller.rename_sheet(1, "Renamed"));
    assert!(controller.move_sheet(1, 0));
    assert!(controller.sheet_is_protected());
    assert!(controller.delete_sheet(0));
    assert_eq!(controller.state.borrow().sheets.len(), 1);
    assert!(controller.undo());
    assert_eq!(controller.state.borrow().sheets.len(), 2);
    assert!(controller.sheet_is_protected());
    assert_eq!(controller.protection_rejection_count(), count);
}
