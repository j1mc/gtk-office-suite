// SPDX-License-Identifier: GPL-3.0-or-later
//! What the collaboration shell decides, with no widget in it: reading an
//! invite, the words of the status button, which head is "theirs", and the
//! two comparisons between a workbook and a file (RFC-0005 2.3 point 1 and
//! 2.5). Separate from `collab_session` so it can be tested without a display.

use std::collections::HashSet;

use suite_collab::{ConnState, HeadInfo};
use tables_core::controller::{diff_ops, Op, WorkbookState};
use tables_core::engine::TablesEngine;
use tables_core::SheetModel;

/// The MIME type of what the sidecar shares and saves.
pub(crate) const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// The document model the shell and the sidecar agree on for a workbook
/// (docs/collab-spec/profiles/loro-1.md).
pub(crate) const DOC_MODEL: &str = "tables/1";

/// The server an invite (`streams:<server url>#<document id>`) names.
pub(crate) fn invite_server(invite: &str) -> Option<String> {
    let (_, rest) = invite.trim().split_once(':')?;
    let (server, doc) = rest.rsplit_once('#')?;
    (!server.is_empty() && !doc.is_empty()).then(|| server.to_owned())
}

/// The keys typed into "People": separated by spaces, commas or lines.
pub(crate) fn parse_people(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A principal as a person can read it: `ed25519:` and the first characters of the key.
pub(crate) fn short(principal: &str) -> String {
    let keep = principal.chars().take(16).collect::<String>();
    if keep.len() < principal.len() {
        format!("{keep}…")
    } else {
        keep
    }
}

/// The icon and the words for a connection: what the button's name and
/// tooltip say, so colour is never the only signal.
pub(crate) fn connection_words(state: &ConnState, people: usize) -> (&'static str, String) {
    let (icon, word) = match state {
        ConnState::Connecting => ("network-transmit-receive-symbolic", suite_common::i18n("connecting")),
        ConnState::Online => ("network-transmit-receive-symbolic", suite_common::i18n("online")),
        ConnState::Offline { .. } => ("network-offline-symbolic", suite_common::i18n("offline, changes are kept")),
        ConnState::AuthRequired => ("dialog-warning-symbolic", suite_common::i18n("not allowed by the server")),
        ConnState::Error { .. } => ("dialog-warning-symbolic", suite_common::i18n("problem")),
    };
    let count = suite_common::ni18n("%d person", "%d people", people as u64).replace("%d", &people.to_string());
    (icon, format!("{}: {word}, {count}", suite_common::i18n("Sharing")))
}

/// The version that is not this session's: the newest head it did not publish
/// itself. `None` when every head is its own, which is no disagreement.
pub(crate) fn their_head<'a>(heads: &'a [HeadInfo], mine: &HashSet<String>) -> Option<&'a HeadInfo> {
    heads.iter().filter(|h| !mine.contains(&h.id)).max_by_key(|h| h.ts_ms)
}

/// Whether `file` (a workbook read back from a version) holds the cells and
/// formulas of `state`. The first check of a join: the genesis rebuilt into a
/// workbook must equal the file it is based on, or the join fails visibly.
pub(crate) fn same_cells(state: &WorkbookState, engine: &TablesEngine, sheets: &[SheetModel]) -> Result<(), String> {
    if sheets.len() != state.sheets.len() {
        return Err(format!("the file has {} sheets, the shared workbook {}", sheets.len(), state.sheets.len()));
    }
    for (index, (mine, theirs)) in state.sheets.iter().zip(sheets).enumerate() {
        let mine = mine.borrow();
        if mine.name != theirs.name {
            return Err(format!("sheet {} is called {:?} in the file and {:?} in the shared workbook", index + 1, theirs.name, mine.name));
        }
        for row in 0..mine.rows.min(theirs.rows) {
            for col in 0..mine.cols.min(theirs.cols) {
                let (a, b) = (state.engine.editable_input_at(index, row, col), engine.editable_input_at(index, row, col));
                if a != b {
                    let at = format!("{}{}", tables_core::sheet::col_label(col), row + 1);
                    return Err(format!("{} on {:?} is {a:?} in the shared workbook and {b:?} in the file", at, mine.name));
                }
            }
        }
    }
    Ok(())
}

/// The ops that make `live` hold what `other` holds, for "Keep theirs": sheet
/// names, the cells' contents, and (when the sheets have the same size) their
/// formats, styles, borders and the sheet-wide properties, by the same diff a
/// local edit is recorded with. It refuses when the two have different sets
/// of sheets: that is not a merge of cells, and "Open both" is the answer.
pub(crate) fn ops_to_match(live: &WorkbookState, engine: &TablesEngine, other: &[SheetModel]) -> Result<Vec<Op>, String> {
    if other.len() != live.sheets.len() {
        return Err(format!("the other version has {} sheets and this one {}", other.len(), live.sheets.len()));
    }
    let mut ops = Vec::new();
    for (index, (mine, theirs)) in live.sheets.iter().zip(other).enumerate() {
        let mine = mine.borrow();
        let sheet = mine.sheet_id;
        if mine.name != theirs.name {
            ops.push(Op::RenameSheet { sheet, name: theirs.name.clone() });
        }
        let mut cells = Vec::new();
        for row in 0..mine.rows.min(theirs.rows) {
            for col in 0..mine.cols.min(theirs.cols) {
                let want = engine.editable_input_at(index, row, col);
                if want != live.engine.editable_input_at(index, row, col) {
                    cells.push((row, col, want));
                }
            }
        }
        if !cells.is_empty() {
            ops.push(Op::SetCells { sheet, cells });
        }
        if (mine.rows, mine.cols) == (theirs.rows, theirs.cols) {
            ops.extend(diff_ops(&mine, theirs));
        }
    }
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use suite_collab::Author;
    use tables_core::controller::apply_all;

    fn head(id: &str, ts_ms: u64) -> HeadInfo {
        HeadInfo {
            id: id.into(),
            author: Author { principal: "ed25519:x".into(), device: "d".into() },
            ts_ms,
            message: None,
            covers: None,
        }
    }

    fn workbook(cells: &[(usize, usize, &str)]) -> WorkbookState {
        let mut state = WorkbookState::new(10, 5).unwrap();
        let sheet = state.sheets[0].borrow().sheet_id;
        let ops: Vec<Op> = cells
            .iter()
            .map(|(row, col, input)| Op::SetCell { sheet, row: *row, col: *col, input: (*input).into() })
            .collect();
        apply_all(&mut state, &ops).unwrap();
        state
    }

    fn models(state: &WorkbookState) -> Vec<SheetModel> {
        state.sheets.iter().map(|s| s.borrow().clone()).collect()
    }

    #[test]
    fn an_invite_names_its_server() {
        assert_eq!(invite_server("streams:http://127.0.0.1:4437/v1/stream/#doc-1").as_deref(), Some("http://127.0.0.1:4437/v1/stream/"));
        assert_eq!(invite_server("  streams:https://s.example/v1/#a#b ").as_deref(), Some("https://s.example/v1/#a"));
        for bad in ["", "streams", "streams:#doc", "streams:http://x/", "streams:http://x/#"] {
            assert_eq!(invite_server(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn people_are_split_on_spaces_commas_and_lines() {
        assert_eq!(parse_people("ed25519:a, ed25519:b\ned25519:c  "), ["ed25519:a", "ed25519:b", "ed25519:c"]);
        assert!(parse_people(" , \n").is_empty());
    }

    #[test]
    fn a_key_is_shortened_on_a_character_boundary() {
        assert_eq!(short("ed25519:short"), "ed25519:short");
        assert_eq!(short("ed25519:AbCdEfGhIjKlMnOp"), "ed25519:AbCdEfGh…");
        assert_eq!(short(&"é".repeat(17)), format!("{}…", "é".repeat(16)));
    }

    #[test]
    fn every_state_says_itself_in_words_and_counts_people() {
        let (icon, words) = connection_words(&ConnState::Online, 2);
        assert_eq!((icon, words.as_str()), ("network-transmit-receive-symbolic", "Sharing: online, 2 people"));
        assert_eq!(connection_words(&ConnState::Online, 1).1, "Sharing: online, 1 person");
        let (icon, words) = connection_words(&ConnState::Offline { since_ms: 1 }, 1);
        assert_eq!(icon, "network-offline-symbolic");
        assert!(words.contains("offline"), "{words}");
        for state in [ConnState::AuthRequired, ConnState::Error { message: "x".into() }] {
            assert_eq!(connection_words(&state, 1).0, "dialog-warning-symbolic");
        }
        assert!(connection_words(&ConnState::Connecting, 1).1.contains("connecting"));
    }

    #[test]
    fn their_head_is_the_newest_head_this_session_did_not_publish() {
        let heads = [head("a", 10), head("b", 30), head("c", 20)];
        let mine: HashSet<String> = ["b".to_owned()].into();
        assert_eq!(their_head(&heads, &mine).map(|h| h.id.as_str()), Some("c"));
        assert_eq!(their_head(&heads, &HashSet::new()).map(|h| h.id.as_str()), Some("b"));
        let all: HashSet<String> = ["a", "b", "c"].map(str::to_owned).into();
        assert!(their_head(&heads, &all).is_none());
    }

    #[test]
    fn a_workbook_equal_to_its_file_passes_and_a_changed_cell_is_named() {
        let state = workbook(&[(0, 0, "item"), (1, 1, "42"), (2, 1, "=B2*2")]);
        let same = workbook(&[(0, 0, "item"), (1, 1, "42"), (2, 1, "=B2*2")]);
        same_cells(&state, &same.engine, &models(&same)).unwrap();
        let other = workbook(&[(0, 0, "item"), (1, 1, "43"), (2, 1, "=B2*2")]);
        let e = same_cells(&state, &other.engine, &models(&other)).unwrap_err();
        assert!(e.contains("B2") && e.contains("42") && e.contains("43"), "{e}");
    }

    #[test]
    fn keeping_theirs_turns_this_workbook_into_the_other_one_and_nothing_else() {
        let mut live = workbook(&[(0, 0, "item"), (1, 1, "42"), (3, 3, "gone")]);
        let theirs = workbook(&[(0, 0, "item"), (1, 1, "43"), (2, 2, "new")]);
        let ops = ops_to_match(&live, &theirs.engine, &models(&theirs)).unwrap();
        assert!(!ops.is_empty());
        apply_all(&mut live, &ops).unwrap();
        same_cells(&live, &theirs.engine, &models(&theirs)).unwrap();
        // Already equal: nothing to do.
        assert!(ops_to_match(&live, &theirs.engine, &models(&theirs)).unwrap().is_empty());
    }

    #[test]
    fn keeping_theirs_refuses_a_different_set_of_sheets() {
        let live = workbook(&[(0, 0, "a")]);
        let mut theirs = workbook(&[(0, 0, "b")]);
        theirs.add_sheet("Second".into(), 10, 5).unwrap();
        let e = ops_to_match(&live, &theirs.engine, &models(&theirs)).unwrap_err();
        assert!(e.contains("2 sheets"), "{e}");
    }
}
