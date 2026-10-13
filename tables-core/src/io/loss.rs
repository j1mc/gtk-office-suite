// loss.rs — what an xlsx save would drop (#1272).
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tables writes one format, xlsx, from its model, and the format gate
// (format.rs) already refuses to put those bytes under another extension.
// What a save can still lose is content: what the model holds and the xlsx
// writer has no form for, and what the source file held that Tables never
// read into the model at all (macros, pictures, Excel tables, slicers…).
// This lists both, as a `CompatibilityReport` of `WarnOnLoss` features, so
// the window can ask before writing — as Letters does for a format that
// can't hold a document, and Decks for content it can't keep (#1264).
//
// The source is read at save time, not remembered from open: the file a
// save replaces (or copies from) is the one that can lose something, and
// once Tables has written it there is nothing left in it to warn about.

use std::collections::BTreeSet;

use suite_common_core::interop::{CompatibilityReport, FeatureDisposition, UnsupportedFeature};
use suite_common_core::zip_guard::{BoundedArchive, ZipBudget};

use crate::sheet::{SheetModel, ValidationRule};

const PIVOTS: (&str, &str) = ("pivot-tables", "Pivot tables (their results stay, as values)");
const VALIDATION: (&str, &str) = ("validation-rules", "Validation rules xlsx can't express");
const MACROS: (&str, &str) = ("macros", "Macros");
const PICTURES: (&str, &str) = ("pictures", "Pictures and drawings");
const THREADED: (&str, &str) = ("threaded-comments", "Threaded comments");
const TABLES: (&str, &str) = ("excel-tables", "Formatted tables (structured ranges)");
const SLICERS: (&str, &str) = ("slicers", "Slicers and timelines");
const LINKS: (&str, &str) = ("external-links", "Links to other workbooks");
const SPARKLINES: (&str, &str) = ("sparklines", "Sparklines");
const CONNECTIONS: (&str, &str) = ("data-connections", "Data connections and queries");
const OLE: (&str, &str) = ("embedded-objects", "Embedded objects");
const PROTECTION_PASSWORD: (&str, &str) = ("sheet-protection-password", "Sheet protection passwords");
const PROTECTION_OPTIONS: (&str, &str) = ("sheet-protection-options", "Sheet protection permissions");

fn record(report: &mut CompatibilityReport, (id, label): (&str, &str), location: &str, detail: &str) {
    report.record(UnsupportedFeature::new(id, label, location, FeatureDisposition::WarnOnLoss, detail));
}

/// Whether the xlsx writer has a form for `rule` (save.rs `xlsx_validation`).
fn writable_rule(rule: &ValidationRule) -> bool {
    match rule {
        ValidationRule::Regex(_) => false,
        ValidationRule::List(items) => !items.is_empty() && items.join(",").chars().count() <= 255,
        ValidationRule::WholeNumber { min, max } => min.is_some() || max.is_some(),
        ValidationRule::Decimal { min, max } => min.is_some() || max.is_some(),
        ValidationRule::TextLength { min, max } => min.is_some() || max.is_some(),
    }
}

/// What saving `sheets` as xlsx over (or from) the file at `source` would
/// lose. `source` is the document's current file, if it has one.
pub fn content_a_save_drops(source: Option<&str>, sheets: &[SheetModel]) -> CompatibilityReport {
    let mut report = CompatibilityReport::new("xlsx");
    for sheet in sheets {
        if !sheet.pivot_tables.is_empty() {
            record(&mut report, PIVOTS, &sheet.name, "the pivot's definition isn't written; its cells are");
        }
        if sheet.validations.iter().flatten().flatten().any(|r| !writable_rule(r)) {
            record(&mut report, VALIDATION, &sheet.name, "a pattern, an empty or over-long list, or an unbounded range");
        }
        if sheet.protection.password_hash.is_some() {
            record(&mut report, PROTECTION_PASSWORD, &sheet.name, "the stored password hash cannot be written; sheet protection is saved without a password");
        }
        let protection = &sheet.protection;
        if !protection.protected && (!protection.allow_select_locked || !protection.allow_select_unlocked
            || protection.allow_format_cells || protection.allow_insert_rows || protection.allow_delete_rows)
        {
            record(&mut report, PROTECTION_OPTIONS, &sheet.name, "nondefault permissions on an unprotected sheet cannot be written; they reset to defaults while cell locks and hidden formulas remain");
        }
    }
    if let Some(path) = source {
        source_drops(path, &mut report);
    }
    report
}

fn source_drops(path: &str, report: &mut CompatibilityReport) {
    let lower = path.to_lowercase();
    let ods = lower.ends_with(".ods");
    if !(ods || lower.ends_with(".xlsx") || lower.ends_with(".xlsm")) {
        return;
    }
    let Ok(file) = std::fs::File::open(path) else { return };
    let Ok(mut zip) = zip::ZipArchive::new(file) else { return };
    let mut budget = ZipBudget::default();
    if budget.check_entry_count(zip.len()).is_err() {
        return;
    }
    let names: BTreeSet<String> = zip.file_names().map(str::to_string).collect();
    if ods {
        let content = zip.optional_part_to_string("content.xml", &mut budget);
        if content.contains("<draw:image") {
            record(report, PICTURES, "content.xml", "pictures on the sheet aren't kept");
        }
        if content.contains("<table:data-pilot-table") {
            record(report, PIVOTS, "content.xml", "Calc's pivot tables aren't read; their cells are");
        }
        if names.iter().any(|n| n.starts_with("Basic/") || n.starts_with("Scripts/")) || content.contains("<office:scripts><office:script") {
            record(report, MACROS, "Basic", "macros aren't kept");
        }
        if content.contains("<draw:object-ole") {
            record(report, OLE, "content.xml", "embedded objects aren't kept");
        }
        if content.contains("<table:table-source") {
            record(report, LINKS, "content.xml", "Calc's hidden copies of linked workbooks aren't kept");
        }
        return;
    }
    let any = |prefix: &str| names.iter().any(|n| n.starts_with(prefix));
    if names.contains("xl/vbaProject.bin") {
        record(report, MACROS, "xl/vbaProject.bin", "macros aren't kept");
    }
    if any("xl/media/") || any("xl/drawings/drawing") && !only_charts(&mut zip, &names, &mut budget) {
        record(report, PICTURES, "xl/drawings", "pictures and shapes on the sheet aren't kept");
    }
    if any("xl/threadedComments/") {
        record(report, THREADED, "xl/threadedComments", "a thread's replies aren't kept; its first note is");
    }
    if any("xl/pivotTables/") {
        record(report, PIVOTS, "xl/pivotTables", "the pivot's definition isn't read; its cells are");
    }
    if any("xl/tables/") {
        record(report, TABLES, "xl/tables", "the table's name, style and filter buttons aren't kept; its cells are");
    }
    if any("xl/slicers/") || any("xl/timelines/") || any("xl/slicerCaches/") {
        record(report, SLICERS, "xl/slicers", "slicers aren't kept");
    }
    if any("xl/externalLinks/") {
        record(report, LINKS, "xl/externalLinks", "formulas keep their last values, not the link");
    }
    if names.contains("xl/connections.xml") || any("customXml/item") && any("xl/queryTables/") {
        record(report, CONNECTIONS, "xl/connections.xml", "queries and connections aren't kept");
    }
    if any("xl/embeddings/oleObject") {
        record(report, OLE, "xl/embeddings", "embedded objects aren't kept");
    }
    let sheets: Vec<String> = names.iter().filter(|n| n.starts_with("xl/worksheets/") && n.ends_with(".xml")).cloned().collect();
    for sheet in sheets {
        let xml = zip.optional_part_to_string(&sheet, &mut budget);
        protection_drops(&xml, &sheet, report);
        if xml.contains("sparklineGroup") {
            record(report, SPARKLINES, &sheet, "sparklines aren't kept");
        }
    }
}

/// The writer accepts plaintext passwords only, not imported legacy hashes or
/// modern salted credentials. Permissions outside SheetProtection also cannot
/// be reproduced from the model. Keep these losses in the existing save gate.
fn protection_drops(xml: &str, location: &str, report: &mut CompatibilityReport) {
    use std::sync::LazyLock;
    static TAG: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"<(?:[A-Za-z_][\w.-]*:)?sheetProtection\b[^>]*>").unwrap()
    });
    static ATTRIBUTE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"\s([A-Za-z][A-Za-z0-9]*)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap()
    });
    for tag in TAG.find_iter(xml) {
        let attrs: std::collections::BTreeMap<&str, &str> = ATTRIBUTE.captures_iter(tag.as_str())
            .map(|a| (a.get(1).unwrap().as_str(), a.get(2).or_else(|| a.get(3)).unwrap().as_str()))
            .collect();
        if attrs.contains_key("password") {
            record(report, PROTECTION_PASSWORD, location, "the imported legacy sheet password hash cannot be written; sheet protection is saved without a password");
        }
        if ["algorithmName", "hashValue", "saltValue", "spinCount"].iter().any(|key| attrs.contains_key(key)) {
            record(report, PROTECTION_PASSWORD, location, "the imported modern salted sheet password credentials cannot be written; sheet protection is saved without a password");
        }
        if !matches!(attrs.get("sheet"), Some(&"1") | Some(&"true")) {
            continue;
        }
        let unsupported: Vec<&str> = [
            ("objects", false), ("scenarios", false),
            ("formatColumns", true), ("formatRows", true), ("insertColumns", true),
            ("deleteColumns", true), ("insertHyperlinks", true), ("sort", true),
            ("autoFilter", true), ("pivotTables", true),
        ].into_iter().filter_map(|(name, default)| {
            let disallowed = attrs.get(name).map_or(default, |value| matches!(*value, "1" | "true"));
            (!disallowed).then_some(name)
        }).collect();
        if !unsupported.is_empty() {
            record(report, PROTECTION_OPTIONS, location, &format!(
                "these unmodeled permissions become disallowed on save: {}", unsupported.join(", ")
            ));
        }
    }
}

/// Whether every drawing in the package holds only charts, which Tables
/// reads (io/charts.rs) and writes back.
fn only_charts<R: std::io::Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, names: &BTreeSet<String>, budget: &mut ZipBudget) -> bool {
    names
        .iter()
        .filter(|n| n.starts_with("xl/drawings/drawing") && n.ends_with(".xml"))
        .all(|n| {
            let xml = zip.optional_part_to_string(n, budget);
            !(xml.contains("<xdr:pic") || xml.contains("<xdr:sp>") || xml.contains("<xdr:sp ") || xml.contains("<xdr:grpSp"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn package(path: &std::path::Path, parts: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in parts {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(data.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    fn ids(r: &CompatibilityReport) -> Vec<&str> {
        let mut v: Vec<&str> = r.features.iter().map(|f| f.id.as_str()).collect();
        v.sort();
        v.dedup();
        v
    }

    fn sheet() -> SheetModel {
        SheetModel::new("Sheet1", 4, 4, 1)
    }

    #[test]
    fn a_workbook_tables_wrote_loses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ours.xlsx");
        let mut s = sheet();
        s.data[0][0] = "1".into();
        crate::io::save_sheets_to_xlsx(path.to_str().unwrap(), &[s.clone()]).unwrap();
        let r = content_a_save_drops(Some(path.to_str().unwrap()), &[s]);
        assert!(r.features.is_empty(), "{:?}", r.features);
        assert!(content_a_save_drops(None, &[sheet()]).features.is_empty());
        assert!(content_a_save_drops(Some(dir.path().join("none.xlsx").to_str().unwrap()), &[sheet()]).features.is_empty());
    }

    #[test]
    fn the_model_reports_pivots_and_rules_xlsx_has_no_form_for() {
        let mut s = sheet();
        s.validations[0][0] = Some(ValidationRule::Regex("^a+$".into()));
        let r = content_a_save_drops(None, &[s.clone()]);
        assert_eq!(ids(&r), ["validation-rules"]);
        s.validations[0][0] = Some(ValidationRule::WholeNumber { min: Some(1), max: None });
        assert!(content_a_save_drops(None, &[s]).features.is_empty(), "a bounded rule is written");
        let mut p = sheet();
        p.pivot_tables.push(crate::sheet::PivotTableSpec {
            name: "Pivot".into(),
            source_range: (0, 0, 3, 1),
            target_cell: (0, 3),
            row_fields: vec![],
            col_fields: vec![],
            data_fields: vec![],
        });
        assert_eq!(ids(&content_a_save_drops(None, &[p])), ["pivot-tables"]);
    }

    #[test]
    fn an_xlsx_lists_what_tables_never_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theirs.xlsx");
        package(&path, &[
            ("xl/workbook.xml", "<workbook/>"),
            ("xl/vbaProject.bin", "x"),
            ("xl/media/image1.png", "png"),
            ("xl/drawings/drawing1.xml", "<xdr:wsDr><xdr:pic/></xdr:wsDr>"),
            ("xl/threadedComments/threadedComment1.xml", "<x/>"),
            ("xl/pivotTables/pivotTable1.xml", "<x/>"),
            ("xl/tables/table1.xml", "<x/>"),
            ("xl/slicers/slicer1.xml", "<x/>"),
            ("xl/externalLinks/externalLink1.xml", "<x/>"),
            ("xl/connections.xml", "<x/>"),
            ("xl/embeddings/oleObject1.bin", "x"),
            ("xl/worksheets/sheet1.xml", "<worksheet><extLst><x14:sparklineGroups><x14:sparklineGroup/></x14:sparklineGroups></extLst></worksheet>"),
        ]);
        let r = content_a_save_drops(Some(path.to_str().unwrap()), &[sheet()]);
        assert_eq!(
            ids(&r),
            ["data-connections", "embedded-objects", "excel-tables", "external-links", "macros", "pictures", "pivot-tables", "slicers", "sparklines", "threaded-comments"]
        );
        assert!(r.requires_confirmation());
    }

    #[test]
    fn a_drawing_with_only_charts_is_not_a_loss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chart.xlsx");
        package(&path, &[("xl/drawings/drawing1.xml", "<xdr:wsDr><xdr:graphicFrame/></xdr:wsDr>"), ("xl/charts/chart1.xml", "<c/>")]);
        assert!(content_a_save_drops(Some(path.to_str().unwrap()), &[sheet()]).features.is_empty());
    }

    #[test]
    fn an_ods_lists_pictures_pivots_macros_and_objects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theirs.ods");
        package(&path, &[
            ("mimetype", "application/vnd.oasis.opendocument.spreadsheet"),
            ("Basic/Standard/Module1.xml", "<x/>"),
            ("content.xml", "<office:document-content><draw:frame><draw:image/></draw:frame><table:data-pilot-table/><draw:object-ole/></office:document-content>"),
        ]);
        let r = content_a_save_drops(Some(path.to_str().unwrap()), &[sheet()]);
        assert_eq!(ids(&r), ["embedded-objects", "macros", "pictures", "pivot-tables"]);
    }

    /// Calc's hidden caches of linked workbooks aren't loaded (load.rs
    /// `is_link_cache`), so a save says it drops them.
    #[test]
    fn an_ods_lists_its_links_to_other_workbooks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("linked.ods");
        package(&path, &[
            ("mimetype", "application/vnd.oasis.opendocument.spreadsheet"),
            ("content.xml", "<office:document-content><table:table table:name=\"'file:///C:/b.xlsm'#S\"><table:table-source xlink:href=\"file:///C:/b.xlsm\"/></table:table></office:document-content>"),
        ]);
        let r = content_a_save_drops(Some(path.to_str().unwrap()), &[sheet()]);
        assert_eq!(ids(&r), ["external-links"]);
    }

    #[test]
    fn csv_and_tsv_have_nothing_beyond_their_cells() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "a,b\n1,2\n").unwrap();
        assert!(content_a_save_drops(Some(path.to_str().unwrap()), &[sheet()]).features.is_empty());
    }

    #[test]
    fn sheet_password_losses_require_confirmation_without_touching_the_source() {
        let dir = tempfile::tempdir().unwrap();
        for (name, attributes, detail) in [
            ("legacy", "password=\"ABCD\"", "legacy"),
            ("modern", "algorithmName='SHA-512' hashValue='digest' saltValue='salt' spinCount='100000'", "modern salted"),
        ] {
            let path = dir.path().join(format!("{name}.xlsx"));
            let xml = format!("<worksheet><sheetProtection sheet=\"1\" objects=\"1\" scenarios=\"1\" {attributes}/></worksheet>");
            package(&path, &[("xl/worksheets/later.xml", &xml)]);
            let before = std::fs::read(&path).unwrap();
            let report = content_a_save_drops(Some(path.to_str().unwrap()), &[sheet()]);
            assert_eq!(ids(&report), ["sheet-protection-password"]);
            assert!(report.requires_confirmation());
            assert!(report.features[0].detail.contains(detail));
            assert_eq!(std::fs::read(&path).unwrap(), before, "preflight must not modify the source");
        }
        let mut s = sheet();
        s.protection.protected = true;
        s.protection.password_hash = Some("ABCD".into());
        assert_eq!(ids(&content_a_save_drops(None, &[s])), ["sheet-protection-password"]);
    }

    #[test]
    fn unmodeled_sheet_permissions_warn_only_when_their_meaning_changes() {
        let mut report = CompatibilityReport::new("xlsx");
        protection_drops(
            r#"<worksheet><sheetProtection sheet="1" objects="1" scenarios="1" formatCells="0" insertRows="0" deleteRows="0" selectLockedCells="1" selectUnlockedCells="1"/></worksheet>"#,
            "Sheet1", &mut report,
        );
        assert!(report.features.is_empty(), "modeled permissions are retained");
        protection_drops(
            r#"<worksheet><sheetProtection sheet="true" objects="true" scenarios="true" formatColumns="false" sort="0" autoFilter="0"/></worksheet>"#,
            "Later", &mut report,
        );
        assert_eq!(ids(&report), ["sheet-protection-options"]);
        assert!(report.requires_confirmation());
        assert!(report.features[0].detail.contains("formatColumns, sort, autoFilter"));
    }

    #[test]
    fn inactive_nondefault_permissions_warn_without_enabling_protection() {
        let mut s = sheet();
        s.protection.allow_insert_rows = true;
        let report = content_a_save_drops(None, &[s]);
        assert_eq!(ids(&report), ["sheet-protection-options"]);
        assert!(report.requires_confirmation());
        assert!(report.features[0].detail.contains("unprotected sheet"));
    }
}
