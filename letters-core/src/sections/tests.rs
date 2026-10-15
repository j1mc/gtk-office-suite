// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::edit::{apply_all, History, Op};
use crate::model::{Document, PageGeometry, TableCell};

fn landscape() -> PageGeometry {
    PageGeometry { width_pt: 841.9, height_pt: 595.3, ..PageGeometry::default() }
}

/// Apply `ops` as one user step of `history`.
fn edit(doc: &mut Document, history: &mut History, ops: &[Op]) {
    let inverse = apply_all(doc, ops).expect("the ops apply");
    history.record(inverse);
}

fn ranges(doc: &Document) -> Vec<(u32, usize, usize)> {
    doc.sections.iter().map(|s| (s.id, s.range.start, s.range.end)).collect()
}

/// A document saved before sections existed reads as one section, and one
/// of one section writes exactly what it did: no `sections` key at all.
#[test]
fn a_document_without_sections_serializes_as_before() {
    let doc = Document::from_plain_text("one\ntwo");
    let json = serde_json::to_string(&doc).unwrap();
    assert!(!json.contains("sections"), "{json}");
    let legacy = json.clone();
    let back: Document = serde_json::from_str(&legacy).unwrap();
    assert!(back.sections.is_empty());
    assert_eq!(back.next_section_id, 0);
    assert_eq!(serde_json::to_string(&back).unwrap(), legacy);
}

/// A recovery snapshot (serde JSON of the document) carries the sections,
/// ranges, ids, pages and headers exactly (SH-003).
#[test]
fn recovery_snapshot_carries_the_section_vector() {
    let mut doc = Document::from_plain_text("portrait\nlandscape\nmore");
    let ops = insert_break(&doc, edit::paragraph_start(&doc, 1), PageGeometry::default()).unwrap();
    apply_all(&mut doc, &ops).unwrap();
    let mut sections = doc.sections.clone();
    sections[1].geometry = landscape();
    sections[1].header = Some("Appendix {page}".into());
    apply_all(&mut doc, &[Op::SetSections { sections }]).unwrap();
    let back: Document = serde_json::from_slice(&serde_json::to_vec(&doc).unwrap()).unwrap();
    assert_eq!(back, doc);
    assert_eq!(ranges(&back), vec![(0, 0, 1), (1, 1, 3)]);
}

/// A break in the middle of a paragraph splits it; the new section takes
/// the page of the one it splits and continues its header.
#[test]
fn a_break_mid_paragraph_splits_it_and_starts_a_section() {
    let mut doc = Document::from_plain_text("alpha beta\ngamma");
    let ops = insert_break(&doc, 6, PageGeometry::default()).unwrap();
    apply_all(&mut doc, &ops).unwrap();
    assert_eq!(doc.to_plain_text(), "alpha \nbeta\ngamma");
    assert_eq!(ranges(&doc), vec![(0, 0, 1), (1, 1, 3)]);
    assert_eq!(doc.sections[1].geometry, PageGeometry::default());
    assert_eq!(doc.page, Some(PageGeometry::default()), "the page mirrors the first section's");
    assert_eq!((doc.sections[1].header.as_deref(), doc.sections[1].footer.as_deref()), (None, None));
    assert!(insert_break(&doc, 0, PageGeometry::default()).is_none(), "nothing to break from at the start");
    let at = edit::paragraph_start(&doc, 1);
    assert!(insert_break(&doc, at, PageGeometry::default()).is_none(), "a section already starts there");
}

/// Deleting a section's break, by command or by deleting the paragraph
/// break between the sections, joins them; one undo puts the sections
/// back verbatim, ranges and ids, and a later break never reuses an id.
#[test]
fn section_delete_restores_ranges_and_ids() {
    let mut doc = Document::from_plain_text("a\nb\nc\nd");
    let mut history = History::default();
    for para in [1, 3] {
        let ops = insert_break(&doc, edit::paragraph_start(&doc, para), PageGeometry::default()).unwrap();
        edit(&mut doc, &mut history, &ops);
    }
    let mut sections = doc.sections.clone();
    sections[1].geometry = landscape();
    edit(&mut doc, &mut history, &[Op::SetSections { sections }]);
    let three = doc.clone();
    assert_eq!(ranges(&three), vec![(0, 0, 1), (1, 1, 3), (2, 3, 4)]);

    // Remove Section Break on the middle section.
    edit(&mut doc, &mut history, &remove_break(&doc, 1).unwrap());
    assert_eq!(ranges(&doc), vec![(0, 0, 3), (2, 3, 4)]);
    assert_eq!(doc.sections[0].geometry, PageGeometry::default(), "the earlier section's page wins");
    history.undo(&mut doc).unwrap();
    assert_eq!(doc, three, "undo restores the ranges, ids and pages");

    // Backspace over the break between "c" and "d".
    let at = edit::paragraph_start(&doc, 3) - 1;
    edit(&mut doc, &mut history, &[Op::Delete { at, len: 1 }]);
    assert_eq!(doc.to_plain_text(), "a\nb\ncd");
    assert_eq!(ranges(&doc), vec![(0, 0, 1), (1, 1, 3)]);
    history.undo(&mut doc).unwrap();
    assert_eq!(doc, three, "undo restores the joined section verbatim");
    history.redo(&mut doc).unwrap();
    assert_eq!(ranges(&doc), vec![(0, 0, 1), (1, 1, 3)]);
    history.undo(&mut doc).unwrap();

    // Ids are never given twice: with section 2 gone, a new break is 3.
    edit(&mut doc, &mut history, &remove_break(&doc, 2).unwrap());
    let ops = insert_break(&doc, edit::paragraph_start(&doc, 2), PageGeometry::default()).unwrap();
    edit(&mut doc, &mut history, &ops);
    assert_eq!(ranges(&doc), vec![(0, 0, 1), (1, 1, 2), (3, 2, 4)]);
}

/// Typing in a section keeps it in that section and moves the ones after.
#[test]
fn typing_moves_the_sections_after_it() {
    let mut doc = Document::from_plain_text("a\nb\nc");
    let ops = insert_break(&doc, edit::paragraph_start(&doc, 2), PageGeometry::default()).unwrap();
    apply_all(&mut doc, &ops).unwrap();
    let mut history = History::default();
    let enter = edit::typing(&doc, 1, "\nnew").unwrap();
    edit(&mut doc, &mut history, &[enter]);
    assert_eq!(doc.to_plain_text(), "a\nnew\nb\nc");
    assert_eq!(ranges(&doc), vec![(0, 0, 3), (1, 3, 4)]);
    history.undo(&mut doc).unwrap();
    assert_eq!(ranges(&doc), vec![(0, 0, 2), (1, 2, 3)]);
}

/// A table never splits across a section break: a break asked for inside
/// one goes before it, a section list breaking one is refused, and an edit
/// that would leave a break inside one moves it to the table's first row.
#[test]
fn table_never_splits_across_section_break() {
    let mut doc = Document::from_plain_text("before\nafter");
    let table = doc.insert_table_at(1, 2, 2);
    assert_eq!(doc.table_paragraph_range(table), Some((1, 4)));
    let in_cell = edit::paragraph_start(&doc, 3);
    let ops = insert_break(&doc, in_cell, PageGeometry::default()).unwrap();
    assert_eq!(ops.len(), 1, "no paragraph splits in a table");
    apply_all(&mut doc, &ops).unwrap();
    assert_eq!(ranges(&doc), vec![(0, 0, 1), (1, 1, 6)]);

    let mut inside = doc.sections.clone();
    inside[0].range.end = 3;
    inside[1].range.start = 3;
    assert_eq!(apply_all(&mut doc.clone(), &[Op::SetSections { sections: inside }]), Err(crate::edit::EditError::Sections));

    // Replacing the paragraph before the table with two keeps the break
    // at the table's first row, not inside it.
    let p = |t: &str| crate::model::Paragraph { style: Default::default(), runs: vec![crate::model::Run::plain(t)] };
    apply_all(&mut doc, &[Op::SetParagraphs { para: 0, remove: 1, insert: vec![p("x"), p("y")] }]).unwrap();
    let first = doc.paragraphs.iter().position(|q| q.style.table_cell.is_some()).unwrap();
    assert_eq!(doc.sections[1].range.start, first);
    assert!(is_valid(&doc, &doc.sections));
    assert_eq!(doc.paragraphs[first].style.table_cell, Some(TableCell { table, row: 0, col: 0 }));
}

/// A section's own header shows on its pages and the ones after that
/// continue it; the first section without one shows the document's.
#[test]
fn headers_continue_until_a_section_has_its_own() {
    let mut doc = Document::from_plain_text("a\nb\nc");
    doc.header = Some("Report".into());
    for para in [1, 2] {
        let ops = insert_break(&doc, edit::paragraph_start(&doc, para), PageGeometry::default()).unwrap();
        apply_all(&mut doc, &ops).unwrap();
    }
    assert_eq!(header(&doc, 0), (Some("Report"), true));
    assert_eq!(header(&doc, 2), (Some("Report"), true));
    let mut sections = doc.sections.clone();
    sections[1].header = Some("Appendix".into());
    sections[2].footer = Some(String::new());
    apply_all(&mut doc, &[Op::SetSections { sections }]).unwrap();
    assert_eq!(header(&doc, 1), (Some("Appendix"), false));
    assert_eq!(header(&doc, 2), (Some("Appendix"), false), "continued from the section before");
    assert_eq!(footer(&doc, 2), (Some(""), false), "an empty footer of its own");
    assert_eq!(footer(&doc, 1), (None, true));
}

/// Page Setup changes the caret's section only; on a document of one
/// section it is the document's page, as before.
#[test]
fn page_setup_sets_the_carets_section() {
    let mut doc = Document::from_plain_text("a\nb");
    assert_eq!(set_page_at(&doc, 1, landscape()), vec![Op::SetPage { page: Some(landscape()) }]);
    let ops = insert_break(&doc, edit::paragraph_start(&doc, 1), PageGeometry::default()).unwrap();
    apply_all(&mut doc, &ops).unwrap();
    let ops = set_page_at(&doc, 1, landscape());
    apply_all(&mut doc, &ops).unwrap();
    assert_eq!(page_at(&doc, 0), Some(PageGeometry::default()));
    assert_eq!(page_at(&doc, 1), Some(landscape()));
    assert!(set_page_at(&doc, 1, landscape()).is_empty());
    assert_eq!(runs(&doc, PageGeometry::default()).iter().map(|r| r.1).collect::<Vec<_>>(), vec![PageGeometry::default(), landscape()]);
}

/// Copying across a section break carries it (`carried`); pasting puts it
/// back (`paste`): the text before it takes the copied section's page and
/// header, the text after keeps the section it was pasted into. One undo
/// takes out the paste and its break together.
#[test]
fn a_copy_carries_its_section_breaks_into_a_paste() {
    let mut source = Document::from_plain_text("portrait\nlandscape");
    let ops = insert_break(&source, edit::paragraph_start(&source, 1), PageGeometry::default()).unwrap();
    apply_all(&mut source, &ops).unwrap();
    let mut sections = source.sections.clone();
    sections[0].header = Some("Part one".into());
    sections[1].geometry = landscape();
    apply_all(&mut source, &[Op::SetSections { sections }]).unwrap();

    let end = edit::doc_len(&source);
    let breaks = carried(&source, 0, end);
    assert_eq!(breaks, vec![CarriedBreak { at: 1, geometry: PageGeometry::default(), header: Some("Part one".into()), footer: None }]);
    assert!(carried(&source, 0, 3).is_empty(), "a copy inside one section carries none");
    let json = serde_json::to_string(&breaks).unwrap();
    assert_eq!(serde_json::from_str::<Vec<CarriedBreak>>(&json).unwrap(), breaks);

    let content = edit::slice(&source, 0, end).unwrap();
    let mut target = Document::from_plain_text("before\nafter");
    target.page = Some(landscape());
    let at = edit::paragraph_start(&target, 1);
    let mut ops = vec![Op::Insert { at, content: content.clone() }];
    ops.push(paste(&target, at, &content, &breaks, PageGeometry::default()).unwrap());
    let original = target.clone();
    let mut history = History::default();
    edit(&mut target, &mut history, &ops);
    assert_eq!(target.to_plain_text(), "before\nportrait\nlandscapeafter");
    assert_eq!(ranges(&target), vec![(1, 0, 2), (0, 2, 3)], "the part before the break is new; the rest keeps its id");
    assert_eq!(target.sections[0].geometry, PageGeometry::default());
    assert_eq!(target.sections[0].header.as_deref(), Some("Part one"));
    assert_eq!(target.sections[1].geometry, landscape(), "the text after the break keeps the target's page");
    assert_eq!(target.page, Some(PageGeometry::default()), "the page mirrors the new first section");
    history.undo(&mut target).unwrap();
    assert_eq!(target, Document { next_section_id: 2, ..original }, "undo takes the paste and its break out together");
    assert!(paste(&target, at, &content, &[], PageGeometry::default()).is_none());
}
