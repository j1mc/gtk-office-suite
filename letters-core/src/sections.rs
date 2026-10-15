// sections.rs — documents whose parts have different pages (LET-005S).
// SPDX-License-Identifier: GPL-3.0-or-later
//
// A report with a landscape appendix, a two-column middle or a part with
// its own header is several sections: runs of whole paragraphs, each with
// its own page size, margins, columns, header and footer. Word stores each
// section's properties in a `w:sectPr` at its end; ODF names a master page
// on the paragraph that starts it. Here they are `Document::sections`.
//
// The contract:
//
// - `sections` is empty for a document of one section (every document
//   before LET-005S, and every file without a section break): its page is
//   `Document::page`, its header and footer the document's. Nothing about
//   such a document changes, so a single-section file saves byte for byte
//   as it did.
// - Otherwise the sections' ranges start at paragraph 0, follow one
//   another without gaps and end at the last paragraph; none is empty, no
//   two share an id, and none starts inside a table (a table never splits
//   across a section break). `Document::page` mirrors the first section's
//   geometry, so whatever reads "the page" reads the first page's.
// - A section's `header`/`footer` of `None` continues the one before it
//   (Word's "link to previous"); the first section's falls back to the
//   document's own (`Document::header`, its pictures and alignment).
//   `Some("")` is a section with no header.
// - Ids are stable: an id is never given again once used
//   (`Document::next_section_id`), and an undo puts the vector back
//   verbatim (`edit::apply`).
// - Paragraph edits keep the sections following them (`splice`): text
//   inserted inside a section stays in it; deleting the break between two
//   sections' paragraphs joins them, the earlier one's page winning.

use serde::{Deserialize, Serialize};

use crate::edit::{self, Op};
use crate::model::{Document, PageGeometry};

/// Paragraphs `start..end` of the document (indexes into
/// `Document::paragraphs`, end exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParagraphRange {
    pub start: usize,
    pub end: usize,
}

impl ParagraphRange {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    pub fn contains(&self, para: usize) -> bool {
        (self.start..self.end).contains(&para)
    }
}

/// One section: its paragraphs, its page, and its own header and footer
/// text (`None` continues the section before's).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocSection {
    pub id: u32,
    pub range: ParagraphRange,
    pub geometry: PageGeometry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
}

/// Whether `sections` is a valid section list for `doc`'s paragraphs (see
/// the contract above). An empty list always is.
pub fn is_valid(doc: &Document, sections: &[DocSection]) -> bool {
    if sections.is_empty() {
        return true;
    }
    let mut at = 0;
    let mut ids = std::collections::HashSet::new();
    for s in sections {
        if s.range.start != at || s.range.is_empty() || !ids.insert(s.id) {
            return false;
        }
        if at > 0 && in_table(doc, at) {
            return false;
        }
        at = s.range.end;
    }
    at == doc.paragraphs.len()
}

/// Whether paragraph `para` continues the table the paragraph before it
/// is in: a section cannot start there.
fn in_table(doc: &Document, para: usize) -> bool {
    let table = |i: usize| doc.paragraphs.get(i).and_then(|p| p.style.table_cell).map(|c| c.table);
    para > 0 && table(para).is_some() && table(para) == table(para - 1)
}

/// Where a section that would start at `para` starts: there, or at the
/// first row of the table `para` is inside.
fn outside_table(doc: &Document, mut para: usize) -> usize {
    while in_table(doc, para) {
        para -= 1;
    }
    para
}

/// The index into `doc.sections` of the section holding paragraph `para`
/// (0 for a document of one section).
pub fn section_of(doc: &Document, para: usize) -> usize {
    doc.sections.iter().position(|s| s.range.contains(para)).unwrap_or_else(|| doc.sections.len().saturating_sub(1))
}

/// The page of the section holding paragraph `para`: the section's, or
/// for a document of one section its page (`None`: the application's).
pub fn page_at(doc: &Document, para: usize) -> Option<PageGeometry> {
    match doc.sections.get(section_of(doc, para)) {
        Some(s) => Some(s.geometry),
        None => doc.page,
    }
}

/// The sections as layout runs them: each one's paragraphs and page. A
/// document of one section is one run over every paragraph on its own page
/// or `default_page`.
pub fn runs(doc: &Document, default_page: PageGeometry) -> Vec<(ParagraphRange, PageGeometry)> {
    if doc.sections.is_empty() {
        return vec![(ParagraphRange::new(0, doc.paragraphs.len()), doc.page.unwrap_or(default_page))];
    }
    doc.sections.iter().map(|s| (s.range, s.geometry)).collect()
}

/// The header text on section `k`'s pages, and whether it is the
/// document's own (`Document::header`, which its pictures go with).
pub fn header(doc: &Document, k: usize) -> (Option<&str>, bool) {
    story(doc, k, |s| &s.header, &doc.header)
}

/// The footer text on section `k`'s pages, and whether it is the
/// document's own.
pub fn footer(doc: &Document, k: usize) -> (Option<&str>, bool) {
    story(doc, k, |s| &s.footer, &doc.footer)
}

fn story<'a>(doc: &'a Document, k: usize, own: impl Fn(&'a DocSection) -> &'a Option<String>, default: &'a Option<String>) -> (Option<&'a str>, bool) {
    let upto = k.min(doc.sections.len().saturating_sub(1));
    for s in doc.sections.iter().take(upto + 1).rev() {
        if let Some(text) = own(s) {
            return (Some(text.as_str()), false);
        }
    }
    (default.as_deref(), true)
}

/// Follow a change to the paragraph list: paragraphs `at..at + removed`
/// replaced by `inserted` new ones (`edit::apply` calls it for every op
/// that adds or removes paragraphs). Sections after the change move with
/// it; one that started inside the replaced paragraphs, after the first,
/// joins the section before it (its break was deleted); sections left
/// without paragraphs go, and a break that would fall inside a table moves
/// to the table's first row.
pub(crate) fn splice(doc: &mut Document, at: usize, removed: usize, inserted: usize) {
    if doc.sections.is_empty() {
        return;
    }
    let starts: Vec<(usize, usize)> = doc
        .sections
        .iter()
        .enumerate()
        .filter_map(|(k, s)| {
            let start = s.range.start;
            if k == 0 {
                Some((k, 0))
            } else if start <= at {
                Some((k, start))
            } else if start < at + removed {
                None
            } else {
                Some((k, start - removed + inserted))
            }
        })
        .collect();
    rebuild(doc, starts);
}

/// Make `doc.sections` the sections `starts` names — (index into the
/// current sections, new first paragraph), in order — dropping empty ones
/// and moving breaks out of tables.
fn rebuild(doc: &mut Document, mut starts: Vec<(usize, usize)>) {
    let n = doc.paragraphs.len();
    let mut floor = 0;
    for (_, start) in starts.iter_mut() {
        *start = outside_table(doc, (*start).min(n)).max(floor);
        floor = *start;
    }
    let old = std::mem::take(&mut doc.sections);
    let mut out: Vec<DocSection> = Vec::with_capacity(starts.len());
    for (i, &(k, start)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(n, |next| next.1);
        if end <= start {
            continue;
        }
        let mut s = old[k].clone();
        s.range = ParagraphRange::new(start, end);
        out.push(s);
    }
    if let Some(first) = out.first_mut() {
        first.range.start = 0;
    }
    doc.sections = out;
}

/// The id the next new section takes: never one a section had before.
fn next_id(doc: &Document, sections: &[DocSection]) -> u32 {
    sections.iter().map(|s| s.id + 1).max().unwrap_or(0).max(doc.next_section_id)
}

/// The ops for Insert ▸ Section Break at sequence offset `at`: the
/// paragraph holding `at` splits there (at its start it does not; the
/// break goes before it), and a new section starts with the paragraph
/// after the split, on the page of the section it splits, continuing that
/// section's header and footer. Inside a table the break goes before the
/// table: rows never split across sections. A document of one section
/// becomes two, the first on its page or `default_page` (the application's,
/// for a document with none). `None` where no break can go: the start of
/// the document, or where a section already starts.
pub fn insert_break(doc: &Document, at: usize, default_page: PageGeometry) -> Option<Vec<Op>> {
    let (pi, po) = edit::locate(doc, at)?;
    let (split, start) = match doc.paragraphs[pi].style.table_cell {
        Some(cell) => (false, doc.table_paragraph_range(cell.table)?.0),
        None if po == 0 => (false, pi),
        None => (true, pi + 1),
    };
    if start == 0 {
        return None;
    }
    let mut ops = Vec::new();
    let mut after = doc.clone();
    if split {
        let op = edit::typing(doc, at, "\n")?;
        edit::apply(&mut after, &op).ok()?;
        ops.push(op);
    }
    let mut sections = if after.sections.is_empty() {
        vec![DocSection {
            id: after.next_section_id,
            range: ParagraphRange::new(0, after.paragraphs.len()),
            geometry: after.page.unwrap_or(default_page),
            header: None,
            footer: None,
        }]
    } else {
        after.sections.clone()
    };
    let k = sections.iter().position(|s| s.range.contains(start))?;
    if sections[k].range.start == start {
        return None;
    }
    let new = DocSection {
        id: next_id(&after, &sections),
        range: ParagraphRange::new(start, sections[k].range.end),
        geometry: sections[k].geometry,
        header: None,
        footer: None,
    };
    sections[k].range.end = start;
    sections.insert(k + 1, new);
    ops.push(Op::SetSections { sections });
    Some(ops)
}

/// The ops that take out the break before section `index` (1 or more):
/// its paragraphs join the section before, which keeps its own page,
/// header and footer. No text changes; one undo puts the break back, id
/// and all.
pub fn remove_break(doc: &Document, index: usize) -> Option<Vec<Op>> {
    if index == 0 || index >= doc.sections.len() {
        return None;
    }
    let mut sections = doc.sections.clone();
    let gone = sections.remove(index);
    sections[index - 1].range.end = gone.range.end;
    Some(vec![Op::SetSections { sections }])
}

/// The ops for Page Setup with the caret in paragraph `para`: the page of
/// the section holding it becomes `geometry` (`SetSections`), or for a
/// document of one section the document's page (`SetPage`). None when it
/// already is.
pub fn set_page_at(doc: &Document, para: usize, geometry: PageGeometry) -> Vec<Op> {
    if doc.sections.is_empty() {
        return if doc.page == Some(geometry) { Vec::new() } else { vec![Op::SetPage { page: Some(geometry) }] };
    }
    let k = section_of(doc, para);
    if doc.sections[k].geometry == geometry {
        return Vec::new();
    }
    let mut sections = doc.sections.clone();
    sections[k].geometry = geometry;
    vec![Op::SetSections { sections }]
}

#[cfg(test)]
mod tests;
