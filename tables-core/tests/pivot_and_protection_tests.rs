// pivot_and_protection_tests.rs — Unit and fixture tests for pivot tables and sheet protection (#114).
// SPDX-License-Identifier: GPL-3.0-or-later

use tables_core::controller::WorkbookController;
use tables_core::sheet::{PivotAggFunc, PivotDataField, PivotField, PivotTableSpec};

#[test]
fn test_pivot_table_evaluation_and_placement() {
    let mut controller = WorkbookController::new(20, 10).unwrap();

    // Populate source data (Region, Category, Sales)
    // Row 0: Headers
    controller.edit_cell(0, 0, "Region");
    controller.edit_cell(0, 1, "Category");
    controller.edit_cell(0, 2, "Sales");

    // Data rows
    controller.edit_cell(1, 0, "North"); controller.edit_cell(1, 1, "Electronics"); controller.edit_cell(1, 2, "100");
    controller.edit_cell(2, 0, "North"); controller.edit_cell(2, 1, "Apparel");     controller.edit_cell(2, 2, "50");
    controller.edit_cell(3, 0, "South"); controller.edit_cell(3, 1, "Electronics"); controller.edit_cell(3, 2, "200");
    controller.edit_cell(4, 0, "South"); controller.edit_cell(4, 1, "Apparel");     controller.edit_cell(4, 2, "150");

    let pivot = PivotTableSpec {
        name: "SalesSummary".to_string(),
        source_range: (0, 0, 4, 2),
        target_cell: (6, 0),
        row_fields: vec![PivotField { col_index: 0, name: "Region".to_string() }],
        col_fields: vec![],
        data_fields: vec![
            PivotDataField { col_index: 2, name: "Sales".to_string(), func: PivotAggFunc::Sum },
            PivotDataField { col_index: 2, name: "Sales".to_string(), func: PivotAggFunc::Count },
        ],
    };

    controller.add_pivot_table(pivot);

    let state = controller.state.borrow();
    let sheet = state.sheet();

    // Target cell (6, 0): Header row [Region, Sum of Sales, Count of Sales]
    assert_eq!(sheet.data[6][0], "Region");
    assert_eq!(sheet.data[6][1], "Sum of Sales");
    assert_eq!(sheet.data[6][2], "Count of Sales");

    // Group rows: North (100 + 50 = 150, count 2)
    assert_eq!(sheet.data[7][0], "North");
    assert_eq!(sheet.data[7][1], "150");
    assert_eq!(sheet.data[7][2], "2");

    // Group rows: South (200 + 150 = 350, count 2)
    assert_eq!(sheet.data[8][0], "South");
    assert_eq!(sheet.data[8][1], "350");
    assert_eq!(sheet.data[8][2], "2");
}

#[test]
fn test_pivot_column_fields_create_cross_tab_intersections() {
    let mut controller = WorkbookController::new(20, 10).unwrap();
    for (row, values) in [
        ["Region", "Category", "Sales"],
        ["North", "Electronics", "100"],
        ["North", "Apparel", "50"],
        ["South", "Electronics", "200"],
    ]
    .iter()
    .enumerate()
    {
        for (column, value) in values.iter().enumerate() {
            controller.edit_cell(row, column, *value);
        }
    }

    let pivot = PivotTableSpec {
        name: "CrossTab".into(),
        source_range: (0, 0, 3, 2),
        target_cell: (6, 0),
        row_fields: vec![PivotField {
            col_index: 0,
            name: "Region".into(),
        }],
        col_fields: vec![PivotField {
            col_index: 1,
            name: "Category".into(),
        }],
        data_fields: vec![PivotDataField {
            col_index: 2,
            name: "Sales".into(),
            func: PivotAggFunc::Sum,
        }],
    };
    let result = controller.evaluate_pivot_table(&pivot);
    controller.add_pivot_table(pivot);
    {
        let state = controller.state.borrow();
        let sheet = state.sheet();
        assert_eq!(sheet.data[6][0], "Region");
        assert_eq!(sheet.data[7][1], "50");
        assert_eq!(sheet.data[8][2], "200");
    }

    assert_eq!(
        result,
        vec![
            vec![
                "Region".to_string(),
                "Apparel - Sum of Sales".to_string(),
                "Electronics - Sum of Sales".to_string(),
            ],
            vec!["North".into(), "50".into(), "100".into()],
            vec!["South".into(), "0".into(), "200".into()],
        ]
    );
}

#[test]
fn test_sheet_protection_enforcement() {
    let mut controller = WorkbookController::new(10, 10).unwrap();

    // Set value in A1 before protection
    controller.edit_cell(0, 0, "Original");

    // Lock cell A1, unlock cell A2
    controller.set_cell_locked(0, 0, true);
    controller.set_cell_locked(1, 0, false);

    // Enable sheet protection
    controller.set_sheet_protection(true, Some("hash123".into()));

    // Try editing locked cell A1 -> should be rejected (remains "Original")
    controller.edit_cell(0, 0, "Modified");
    {
        let state = controller.state.borrow();
        assert_eq!(state.sheet().data[0][0], "Original");
    }

    // Try editing unlocked cell A2 -> should succeed
    controller.edit_cell(1, 0, "Allowed");
    {
        let state = controller.state.borrow();
        assert_eq!(state.sheet().data[1][0], "Allowed");
    }

    // Disable sheet protection
    controller.set_sheet_protection(false, None);

    // Editing cell A1 should now succeed
    controller.edit_cell(0, 0, "ModifiedNow");
    {
        let state = controller.state.borrow();
        assert_eq!(state.sheet().data[0][0], "ModifiedNow");
    }
}
