// sheet_protection.rs — OOXML `<sheetProtection>` in model terms (TBL-005).
// SPDX-License-Identifier: GPL-3.0-or-later
//
// One table maps every permission of ECMA-376 Part 1 §18.3.1.85
// (CT_SheetProtection) to its `SheetProtection` field. The importer, the
// writer and the loss scan all read it, so a permission cannot be modeled in
// one of them and silently dropped by another. Attribute values mean
// *disallowed* — the inverse of the model's `allow_*` — and each has its own
// schema default: `objects` and `scenarios` are allowed when absent, every
// other operation is disallowed, and both selections are allowed.

use crate::sheet::{SheetModel, SheetProtection};
use std::collections::BTreeMap;

/// One CT_SheetProtection permission.
pub(crate) struct Permission {
    pub attribute: &'static str,
    /// The schema default: true when an absent attribute means disallowed.
    pub disallowed_by_default: bool,
    pub allowed: fn(&SheetProtection) -> bool,
    pub allow: fn(&mut SheetProtection) -> &mut bool,
}

macro_rules! permission {
    ($attribute:literal, $default:literal, $field:ident) => {
        Permission {
            attribute: $attribute,
            disallowed_by_default: $default,
            allowed: |p| p.$field,
            allow: |p| &mut p.$field,
        }
    };
}

/// Every permission attribute of `<sheetProtection>`, besides `sheet` itself
/// and the credential attributes.
pub(crate) const PERMISSIONS: [Permission; 15] = [
    permission!("objects", false, allow_edit_objects),
    permission!("scenarios", false, allow_edit_scenarios),
    permission!("formatCells", true, allow_format_cells),
    permission!("formatColumns", true, allow_format_columns),
    permission!("formatRows", true, allow_format_rows),
    permission!("insertColumns", true, allow_insert_columns),
    permission!("insertRows", true, allow_insert_rows),
    permission!("insertHyperlinks", true, allow_insert_hyperlinks),
    permission!("deleteColumns", true, allow_delete_columns),
    permission!("deleteRows", true, allow_delete_rows),
    permission!("selectLockedCells", false, allow_select_locked),
    permission!("sort", true, allow_sort),
    permission!("autoFilter", true, allow_autofilter),
    permission!("pivotTables", true, allow_pivot_tables),
    permission!("selectUnlockedCells", false, allow_select_unlocked),
];

/// The `<sheetProtection …>` start tags in a worksheet part, prefixed or not.
pub(crate) fn tags(xml: &str) -> impl Iterator<Item = &str> {
    use std::sync::LazyLock;
    static TAG: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"<(?:[A-Za-z_][\w.-]*:)?sheetProtection\b[^>]*>").unwrap()
    });
    TAG.find_iter(xml).map(|m| m.as_str())
}

/// A start tag's attributes by local name, single or double quoted. Local
/// names may hold hyphens, as ODF's do.
pub(crate) fn attributes(tag: &str) -> BTreeMap<&str, &str> {
    use std::sync::LazyLock;
    static ATTRIBUTE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"\s(?:[A-Za-z_][\w.-]*:)?([A-Za-z][A-Za-z0-9-]*)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap()
    });
    ATTRIBUTE
        .captures_iter(tag)
        .map(|a| (a.get(1).unwrap().as_str(), a.get(2).or_else(|| a.get(3)).unwrap().as_str()))
        .collect()
}

/// An xsd:boolean; anything else is not a value and the default applies.
fn boolean(value: &str) -> Option<bool> {
    match value.trim() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

/// The modeled state of one `<sheetProtection>` element's attributes.
pub(crate) fn parse(attrs: &BTreeMap<&str, &str>) -> SheetProtection {
    let mut protection = SheetProtection {
        protected: attrs.get("sheet").and_then(|v| boolean(v)).unwrap_or(false),
        ..SheetProtection::default()
    };
    for permission in &PERMISSIONS {
        let disallowed = attrs
            .get(permission.attribute)
            .and_then(|v| boolean(v))
            .unwrap_or(permission.disallowed_by_default);
        *(permission.allow)(&mut protection) = !disallowed;
    }
    protection
}

/// What re-reading a sheet with no `<sheetProtection>` gives.
fn absent() -> SheetProtection {
    SheetProtection::default()
}

/// The element that reproduces `protection` exactly, or `None` when writing
/// nothing does (an unprotected sheet at the defaults). Attributes appear
/// only where they differ from the schema default.
pub(crate) fn element(protection: &SheetProtection) -> Option<String> {
    if !protection.protected && *protection == absent() {
        return None;
    }
    let mut xml = String::from("<sheetProtection");
    if protection.protected {
        xml.push_str(" sheet=\"1\"");
    }
    for permission in &PERMISSIONS {
        let disallowed = !(permission.allowed)(protection);
        if disallowed != permission.disallowed_by_default {
            xml.push_str(&format!(" {}=\"{}\"", permission.attribute, if disallowed { 1 } else { 0 }));
        }
    }
    xml.push_str("/>");
    Some(xml)
}

/// The writer's own form of a protected sheet's permissions: its options
/// are *allowed* flags, the model's sense.
pub(crate) fn writer_options(p: &SheetProtection) -> rust_xlsxwriter::ProtectionOptions {
    rust_xlsxwriter::ProtectionOptions {
        select_locked_cells: p.allow_select_locked,
        select_unlocked_cells: p.allow_select_unlocked,
        format_cells: p.allow_format_cells,
        format_columns: p.allow_format_columns,
        format_rows: p.allow_format_rows,
        insert_columns: p.allow_insert_columns,
        insert_rows: p.allow_insert_rows,
        insert_links: p.allow_insert_hyperlinks,
        delete_columns: p.allow_delete_columns,
        delete_rows: p.allow_delete_rows,
        sort: p.allow_sort,
        use_autofilter: p.allow_autofilter,
        use_pivot_tables: p.allow_pivot_tables,
        edit_scenarios: p.allow_edit_scenarios,
        edit_objects: p.allow_edit_objects,
        ..Default::default()
    }
}

/// Whether the writer's own element cannot say what `protection` holds, so
/// the saved part is patched with [`element`]: the writer only emits
/// `<sheetProtection>` for a protected sheet.
pub(crate) fn needs_patch(protection: &SheetProtection) -> bool {
    !protection.protected && element(protection).is_some()
}

/// Whether the writer must be asked for an element at all, to be kept or
/// patched: the element's position in the worksheet is the writer's.
pub(crate) fn needs_element(protection: &SheetProtection) -> bool {
    element(protection).is_some()
}

/// Replace the writer's `<sheetProtection>` in each worksheet part whose
/// sheet [`needs_patch`]. Other members are copied byte for byte, still
/// compressed. A part without exactly one element is an error, never a
/// silently unprotected save.
pub(crate) fn patch_package(bytes: Vec<u8>, sheets: &[SheetModel]) -> Result<Vec<u8>, String> {
    use std::io::{Read, Write};
    let patches: BTreeMap<String, String> = sheets
        .iter()
        .enumerate()
        .filter(|(_, s)| needs_patch(&s.protection))
        .filter_map(|(i, s)| Some((format!("xl/worksheets/sheet{}.xml", i + 1), element(&s.protection)?)))
        .collect();
    if patches.is_empty() {
        return Ok(bytes);
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("Protection: {e}"))?;
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut patched = 0;
    for i in 0..archive.len() {
        let name = archive.by_index_raw(i).map_err(|e| format!("Protection: {e}"))?.name().to_string();
        let Some(replacement) = patches.get(&name) else {
            let raw = archive.by_index_raw(i).map_err(|e| format!("Protection: {e}"))?;
            out.raw_copy_file(raw).map_err(|e| format!("Protection: {e}"))?;
            continue;
        };
        let mut xml = String::new();
        archive
            .by_index(i)
            .map_err(|e| format!("Protection: {e}"))?
            .read_to_string(&mut xml)
            .map_err(|e| format!("Protection: {e}"))?;
        let found: Vec<&str> = tags(&xml).collect();
        let [tag] = found.as_slice() else {
            return Err(format!("Protection: {name} has {} sheetProtection elements", found.len()));
        };
        let xml = xml.replacen(tag, replacement, 1);
        out.start_file(name.as_str(), zip::write::SimpleFileOptions::default())
            .map_err(|e| format!("Protection: {e}"))?;
        out.write_all(xml.as_bytes()).map_err(|e| format!("Protection: {e}"))?;
        patched += 1;
    }
    if patched != patches.len() {
        return Err("Protection: a worksheet part to protect is missing".into());
    }
    Ok(out.finish().map_err(|e| format!("Protection: {e}"))?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(tag: &str) -> SheetProtection {
        parse(&attributes(tag))
    }

    #[test]
    fn absent_attributes_take_each_schema_default() {
        let p = parsed("<sheetProtection sheet=\"1\"/>");
        assert!(p.protected);
        assert!(p.allow_select_locked && p.allow_select_unlocked);
        assert!(p.allow_edit_objects && p.allow_edit_scenarios, "objects/scenarios default to unprotected");
        for permission in &PERMISSIONS {
            assert_eq!((permission.allowed)(&p), !permission.disallowed_by_default, "{}", permission.attribute);
        }
        assert_eq!(parsed("<x:sheetProtection/>"), SheetProtection {
            allow_edit_objects: true,
            allow_edit_scenarios: true,
            ..SheetProtection::default()
        });
    }

    #[test]
    fn every_permission_round_trips_through_its_element_alone() {
        for (i, permission) in PERMISSIONS.iter().enumerate() {
            for protected in [true, false] {
                let mut p = SheetProtection { protected, ..SheetProtection::default() };
                let flipped = !(permission.allowed)(&p);
                *(permission.allow)(&mut p) = flipped;
                let xml = element(&p).expect("a nondefault protection is written");
                assert_eq!(parsed(&xml), p, "{i}: {} in {xml}", permission.attribute);
                let attrs = attributes(&xml);
                assert_eq!(attrs.contains_key("sheet"), protected);
            }
        }
        assert_eq!(element(&SheetProtection::default()), None);
    }

    #[test]
    fn values_quotes_and_prefixes_are_read_as_ooxml_booleans() {
        let p = parsed("<s:sheetProtection s:sheet='true' formatCells=\"false\" sort=\"0\" objects=\"yes\" selectLockedCells=\"1\">");
        assert!(p.protected);
        assert!(p.allow_format_cells && p.allow_sort);
        assert!(p.allow_edit_objects, "an invalid boolean keeps the schema default");
        assert!(!p.allow_select_locked);
    }

    #[test]
    fn writer_options_match_the_element_for_a_protected_sheet() {
        let mut workbook = rust_xlsxwriter::Workbook::new();
        for (i, permission) in PERMISSIONS.iter().enumerate() {
            let mut p = SheetProtection { protected: true, ..SheetProtection::default() };
            let flipped = !(permission.allowed)(&p);
            *(permission.allow)(&mut p) = flipped;
            let sheet = workbook.add_worksheet();
            sheet.set_name(format!("S{i}")).unwrap();
            sheet.protect_with_options(&writer_options(&p));
        }
        let bytes = workbook.save_to_buffer().unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        for (i, permission) in PERMISSIONS.iter().enumerate() {
            let mut xml = String::new();
            std::io::Read::read_to_string(&mut archive.by_name(&format!("xl/worksheets/sheet{}.xml", i + 1)).unwrap(), &mut xml).unwrap();
            let tag = tags(&xml).next().unwrap();
            let mut expected = SheetProtection { protected: true, ..SheetProtection::default() };
            let flipped = !(permission.allowed)(&expected);
            *(permission.allow)(&mut expected) = flipped;
            assert_eq!(parsed(tag), expected, "{}: {tag}", permission.attribute);
        }
    }
}
