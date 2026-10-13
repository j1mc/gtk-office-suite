//! loss.rs — what a save would drop from the file a deck came from.
//! SPDX-License-Identifier: GPL-3.0-or-later
//!
//! Decks writes its whole package from the model, so anything the readers
//! don't model is gone after a save over (or a Save As from) a file that
//! had it: comments, audio and video, embedded OLE objects, SmartArt,
//! emphasis and motion-path animations, and pictures the file names but
//! does not contain. decks-readiness.md asks that such content be
//! "preserved or blocked/warned by #374 before save"; this finds it, as a
//! `CompatibilityReport` of `WarnOnLoss` features (or `HardError` when
//! inspection fails), so the window can ask
//! before writing (`decks/src/loss_ui.rs`), as Letters does for a format
//! that can't hold what a document has.
//!
//! It reads the file on disk at save time, not a note taken at open: the
//! file a save replaces or copies from is the one that can lose something,
//! and once Decks has written it there is nothing left to warn about.

use std::collections::BTreeSet;
use std::io::Read;

use suite_common_core::interop::{CompatibilityReport, FeatureDisposition, UnsupportedFeature};
use suite_common_core::zip_guard::{BoundedArchive, ZipBudget};

/// One kind of content Decks drops: its id and the label the dialog shows.
const COMMENTS: (&str, &str) = ("comments", "Comments");
const MEDIA: (&str, &str) = ("media", "Audio and video");
const OLE: (&str, &str) = ("embedded-objects", "Embedded objects");
const SMARTART: (&str, &str) = ("smartart", "SmartArt graphics");
const ANIMATION: (&str, &str) = ("animations", "Emphasis and motion-path animations");
const MISSING: (&str, &str) = ("missing-pictures", "Pictures missing from the file");

fn record(report: &mut CompatibilityReport, (id, label): (&str, &str), location: &str, detail: &str) {
    report.record(UnsupportedFeature::new(id, label, location, FeatureDisposition::WarnOnLoss, detail));
}

/// The content of the presentation at `path` that a save of the deck read
/// from it would lose. A source that cannot be inspected blocks ordinary
/// saving: missing is not a new document, whose caller has no source.
/// A separately confirmed recovery copy does not claim to preserve source
/// content. This is a bounded lexical scan, not full package/XML validation.
pub fn content_a_save_drops(path: &str) -> CompatibilityReport {
    content_a_save_drops_with_budget(path, &mut ZipBudget::default())
}

/// Whether an explicitly unverified recovery copy has a fresh destination,
/// distinct from the source. Never replace an existing entry, including a
/// symlink; canonical parent directories also catch aliases of a missing
/// source. Call again after confirmation, before writing.
pub fn can_save_unverified_copy(source: &str, target: &str) -> bool {
    use std::path::Path;
    let source = Path::new(source);
    let target = Path::new(target);
    if !matches!(std::fs::symlink_metadata(target), Err(e) if e.kind() == std::io::ErrorKind::NotFound) {
        return false;
    }
    let location = |path: &Path| {
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let name = path.file_name().ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        std::fs::canonicalize(parent).map(|parent| parent.join(name))
    };
    let Ok(target) = location(target) else { return false };
    // A dangling source symlink could name the proposed destination.
    match std::fs::symlink_metadata(source) {
        Ok(metadata) if metadata.file_type().is_symlink() =>
            std::fs::canonicalize(source).is_ok_and(|source| source != target),
        Ok(_) => location(source).is_ok_and(|source| source != target),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match location(source) {
            Ok(source) => source != target,
            // The source's parent has disappeared; it cannot be the fresh
            // target in the existing parent directory established above.
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        },
        Err(_) => false,
    }
}

fn content_a_save_drops_with_budget(path: &str, budget: &mut ZipBudget) -> CompatibilityReport {
    let odp = path.to_lowercase().ends_with(".odp");
    let mut report = CompatibilityReport::new(if odp { "odp" } else { "pptx" });
    if let Err(detail) = inspect_source(path, odp, budget, &mut report) {
        report.record(UnsupportedFeature::new(
            "source-inspection-failed",
            "Source content could not be checked",
            path,
            FeatureDisposition::HardError,
            detail,
        ));
    }
    report
}

fn inspect_source(path: &str, odp: bool, budget: &mut ZipBudget, report: &mut CompatibilityReport) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open source: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("read source package: {e}"))?;
    budget.check_entry_count(zip.len())?;
    let names: BTreeSet<String> = zip.file_names().map(str::to_string).collect();
    if odp {
        odp_drops(&mut zip, &names, budget, report)
    } else {
        pptx_drops(&mut zip, &names, budget, report)
    }
}

fn scan_part<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str, budget: &mut ZipBudget) -> Result<String, String> {
    zip.part_to_string(name, budget).map_err(|e| format!("inspect {name}: {e}"))
}

fn pptx_drops<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, names: &BTreeSet<String>, budget: &mut ZipBudget, report: &mut CompatibilityReport) -> Result<(), String> {
    // Comment parts themselves: PowerPoint writes an authors list
    // (ppt/authors.xml, ppt/commentAuthors.xml) into decks with no comments.
    if names.iter().any(|n| n.starts_with("ppt/comments/") && n.ends_with(".xml")) {
        record(report, COMMENTS, "ppt/comments", "comments on slides are not kept");
    }
    if names.iter().any(|n| n.starts_with("ppt/diagrams/")) {
        record(report, SMARTART, "ppt/diagrams", "SmartArt is not kept");
    }
    for slide in names.iter().filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml")) {
        let xml = scan_part(zip, slide, budget)?;
        if xml.contains("<a:videoFile") || xml.contains("<a:audioFile") || xml.contains("<p14:media") {
            record(report, MEDIA, slide, "audio and video are not kept");
        }
        if xml.contains("<p:oleObj") {
            record(report, OLE, slide, "embedded objects are not kept");
        }
        if xml.contains("presetClass=\"emph\"") || xml.contains("presetClass=\"path\"") {
            record(report, ANIMATION, slide, "only entrance and exit builds are kept");
        }
        let rels = slide.replacen("ppt/slides/", "ppt/slides/_rels/", 1) + ".rels";
        // A slide need not have relationships. If the part exists, failure
        // to inspect it is not evidence that all its pictures are present.
        if !names.contains(&rels) {
            continue;
        }
        let rels_xml = scan_part(zip, &rels, budget)?;
        for target in internal_targets(&rels_xml, "/image") {
            if !names.contains(&resolve("ppt/slides", &target)) {
                record(report, MISSING, slide, &format!("{target} is not in the file"));
            }
        }
    }
    Ok(())
}

fn odp_drops<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, names: &BTreeSet<String>, budget: &mut ZipBudget, report: &mut CompatibilityReport) -> Result<(), String> {
    let content = scan_part(zip, "content.xml", budget)?;
    if content.contains("<officeooo:annotation") || content.contains("<office:annotation") {
        record(report, COMMENTS, "content.xml", "comments on slides are not kept");
    }
    if content.contains("<draw:plugin") {
        record(report, MEDIA, "content.xml", "audio and video are not kept");
    }
    if content.contains("<draw:object-ole") {
        record(report, OLE, "content.xml", "embedded objects are not kept");
    }
    if content.contains("presentation:preset-class=\"emphasis\"") || content.contains("presentation:preset-class=\"motion-path\"") {
        record(report, ANIMATION, "content.xml", "only entrance and exit builds are kept");
    }
    for href in attr_values(&content, "<draw:image", "xlink:href") {
        let internal = !href.contains(':') && !href.starts_with('/') && !href.starts_with("../");
        if internal && !names.contains(href.trim_start_matches("./")) {
            record(report, MISSING, "content.xml", &format!("{href} is not in the file"));
        }
    }
    Ok(())
}

/// The values of `attr` on every `element` start tag in `xml`.
fn attr_values<'a>(xml: &'a str, element: &str, attr: &str) -> Vec<&'a str> {
    let needle = format!("{attr}=\"");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(element) {
        let tag = &rest[i..];
        let end = tag.find('>').unwrap_or(tag.len());
        let tag = &tag[..end];
        if let Some(j) = tag.find(&needle) {
            let v = &tag[j + needle.len()..];
            if let Some(k) = v.find('"') {
                out.push(&v[..k]);
            }
        }
        rest = &rest[i + element.len()..];
    }
    out
}

/// The targets of the relationships in `rels` whose type ends with
/// `type_suffix`, leaving out external ones (a linked picture is a URL,
/// not a part).
fn internal_targets(rels: &str, type_suffix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = rels;
    while let Some(i) = rest.find("<Relationship ") {
        let tag = &rest[i..];
        let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
        let get = |k: &str| attr_values(tag, "<Relationship", k).first().map(|s| s.to_string());
        let external = get("TargetMode").is_some_and(|m| m == "External");
        if !external && get("Type").is_some_and(|t| t.ends_with(type_suffix)) {
            if let Some(t) = get("Target") {
                out.push(t);
            }
        }
        rest = &rest[i + 1..];
    }
    out
}

/// `target`, relative to the part folder `base`, as a package path.
fn resolve(base: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
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

    fn assert_inspection_blocked(report: &CompatibilityReport, detail: &str) {
        let failure = report.features.iter().find(|f| f.id == "source-inspection-failed")
            .expect("a failed source scan must not claim the save is clean");
        assert_eq!(failure.disposition, FeatureDisposition::HardError);
        assert!(failure.detail.contains(detail), "{}", failure.detail);
        assert!(report.validate_save(false).is_err());
        assert!(report.validate_save(true).is_err(), "Save Anyway must not override a failed inspection");
    }

    #[test]
    fn missing_or_corrupt_sources_block_even_confirmed_saves() {
        let dir = tempfile::tempdir().unwrap();
        for ext in ["pptx", "odp"] {
            let path = dir.path().join(format!("source.{ext}"));
            assert_inspection_blocked(&content_a_save_drops(path.to_str().unwrap()), "open source");
            std::fs::write(&path, b"not a ZIP package").unwrap();
            assert_inspection_blocked(&content_a_save_drops(path.to_str().unwrap()), "read source package");
        }
    }

    #[test]
    fn recovery_copy_requires_a_fresh_distinct_destination() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.pptx");
        let copy = dir.path().join("copy.pptx");
        let allowed = |target: &std::path::Path| can_save_unverified_copy(source.to_str().unwrap(), target.to_str().unwrap());
        assert!(!allowed(&source), "a missing source is not a copy destination");
        assert!(!allowed(&dir.path().join(".").join("source.pptx")));
        assert!(allowed(&copy), "a missing source can be recovered to a fresh name");
        std::fs::write(&source, b"corrupt source").unwrap();
        assert!(allowed(&copy));
        std::fs::write(&copy, b"existing destination").unwrap();
        assert!(!allowed(&copy), "recovery must not overwrite another file");
    }

    #[test]
    fn recovery_copy_handles_relative_sources_and_deleted_parent_directories() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let source = dir.path().join("source.pptx");
        let absolute_source = std::fs::canonicalize(dir.path()).unwrap().join("source.pptx");
        assert!(!can_save_unverified_copy(source.to_str().unwrap(), absolute_source.to_str().unwrap()));
        let missing_parent_source = dir.path().join("deleted-directory").join("source.pptx");
        let copy = dir.path().join("copy.pptx");
        assert!(can_save_unverified_copy(missing_parent_source.to_str().unwrap(), copy.to_str().unwrap()));
        assert!(!can_save_unverified_copy(source.to_str().unwrap(), missing_parent_source.to_str().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn recovery_copy_rejects_symlink_and_hardlink_aliases() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.pptx");
        let alias_dir = dir.path().join("alias");
        symlink(dir.path(), &alias_dir).unwrap();
        assert!(!can_save_unverified_copy(source.to_str().unwrap(), alias_dir.join("source.pptx").to_str().unwrap()));
        let link = dir.path().join("link.pptx");
        symlink(&source, &link).unwrap();
        assert!(!can_save_unverified_copy(source.to_str().unwrap(), link.to_str().unwrap()));
        assert!(!can_save_unverified_copy(link.to_str().unwrap(), source.to_str().unwrap()));
        std::fs::write(&source, b"source").unwrap();
        let hardlink = dir.path().join("hardlink.pptx");
        std::fs::hard_link(&source, &hardlink).unwrap();
        assert!(!can_save_unverified_copy(source.to_str().unwrap(), hardlink.to_str().unwrap()));
    }

    #[test]
    fn missing_required_odp_content_blocks_saving() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing-content.odp");
        package(&path, &[("mimetype", "application/vnd.oasis.opendocument.presentation")]);
        assert_inspection_blocked(&content_a_save_drops(path.to_str().unwrap()), "content.xml: missing");
    }

    #[test]
    fn absent_optional_slide_relationships_are_not_a_scan_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-relationships.pptx");
        package(&path, &[("ppt/slides/slide1.xml", "<p:sld/>")]);
        assert!(content_a_save_drops(path.to_str().unwrap()).validate_save(false).is_ok());
    }

    #[test]
    fn corrupt_relevant_parts_block_saving_instead_of_becoming_empty() {
        let dir = tempfile::tempdir().unwrap();
        for (ext, part) in [
            ("odp", "content.xml"),
            ("pptx", "ppt/slides/slide1.xml"),
            ("pptx", "ppt/slides/_rels/slide1.xml.rels"),
        ] {
            let path = dir.path().join(format!("corrupt.{ext}"));
            for invalid_utf8 in [false, true] {
                let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
                let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
                if part.ends_with(".rels") {
                    writer.start_file("ppt/slides/slide1.xml", options).unwrap();
                    writer.write_all(b"<p:sld/>").unwrap();
                }
                writer.start_file(part, options).unwrap();
                writer.write_all(if invalid_utf8 { b"\xff" } else { b"corrupt-me" }).unwrap();
                writer.finish().unwrap();
                if !invalid_utf8 {
                    let mut bytes = std::fs::read(&path).unwrap();
                    let offset = bytes.windows(b"corrupt-me".len()).position(|s| s == b"corrupt-me").unwrap();
                    bytes[offset] ^= 1; // Leave the stored CRC unchanged.
                    std::fs::write(&path, bytes).unwrap();
                }
                assert_inspection_blocked(&content_a_save_drops(path.to_str().unwrap()), part);
            }
        }
    }

    #[test]
    fn oversized_relevant_parts_block_saving() {
        use suite_common_core::zip_guard::ZipLimits;
        let dir = tempfile::tempdir().unwrap();
        for (ext, part) in [
            ("odp", "content.xml"),
            ("pptx", "ppt/slides/slide1.xml"),
            ("pptx", "ppt/slides/_rels/slide1.xml.rels"),
        ] {
            let path = dir.path().join(format!("oversized.{ext}"));
            let large = " ".repeat(65);
            let mut parts = vec![(part, large.as_str())];
            if part.ends_with(".rels") {
                parts.push(("ppt/slides/slide1.xml", "<p:sld/>"));
            }
            package(&path, &parts);
            let mut budget = ZipBudget::new(ZipLimits { max_entry_bytes: 64, ..ZipLimits::default() });
            let report = content_a_save_drops_with_budget(path.to_str().unwrap(), &mut budget);
            assert_inspection_blocked(&report, part);
            assert_inspection_blocked(&report, "byte limit");
        }
    }

    #[test]
    fn entry_count_and_total_scan_budgets_block_saving() {
        use suite_common_core::zip_guard::ZipLimits;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("over-budget.pptx");
        package(&path, &[
            ("ppt/slides/slide1.xml", "<p:sld/>"),
            ("ppt/slides/slide2.xml", "<p:sld/>"),
        ]);
        let mut count = ZipBudget::new(ZipLimits { max_entries: 1, ..ZipLimits::default() });
        assert_inspection_blocked(&content_a_save_drops_with_budget(path.to_str().unwrap(), &mut count), "members");
        let mut total = ZipBudget::new(ZipLimits { max_total_bytes: 12, ..ZipLimits::default() });
        assert_inspection_blocked(&content_a_save_drops_with_budget(path.to_str().unwrap(), &mut total), "byte total");
    }

    #[test]
    fn scan_failure_does_not_discard_losses_already_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("comments-and-broken-slide.pptx");
        package(&path, &[
            ("ppt/comments/comment1.xml", "<p:cmLst/>"),
            ("ppt/slides/slide1.xml", "<p:sld/>"),
        ]);
        let mut budget = ZipBudget::new(suite_common_core::zip_guard::ZipLimits {
            max_entry_bytes: 1,
            ..Default::default()
        });
        let report = content_a_save_drops_with_budget(path.to_str().unwrap(), &mut budget);
        assert_eq!(ids(&report), ["comments", "source-inspection-failed"]);
        assert_inspection_blocked(&report, "ppt/slides/slide1.xml");
    }

    #[test]
    fn a_deck_decks_wrote_loses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        for ext in ["pptx", "odp"] {
            let p = dir.path().join(format!("ours.{ext}"));
            let mut deck = crate::Deck::new();
            deck.slides[0].objects.push(crate::SlideObject::Shape {
                kind: crate::engine::shape::ShapeKind::Ellipse,
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                rotation: 0.0,
                style: Default::default(),
            });
            crate::write_deck(p.to_str().unwrap(), &deck).unwrap();
            let r = content_a_save_drops(p.to_str().unwrap());
            assert!(r.features.is_empty(), "{ext}: {:?}", r.features);
            assert!(!r.requires_confirmation());
        }
    }

    #[test]
    fn a_pptx_lists_comments_media_ole_smartart_animations_and_missing_pictures() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("theirs.pptx");
        package(&p, &[
            ("ppt/authors.xml", "<p188:authorLst/>"),
            ("ppt/comments/comment1.xml", "<p:cmLst/>"),
            ("ppt/diagrams/data1.xml", "<dgm:dataModel/>"),
            ("ppt/media/image1.png", "png"),
            (
                "ppt/slides/slide1.xml",
                "<p:sld><a:videoFile r:link=\"rId3\"/><p:oleObj/><p:cTn presetClass=\"emph\"/></p:sld>",
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                "<Relationships>\
                 <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/image1.png\"/>\
                 <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/gone.png\"/>\
                 <Relationship Id=\"rId4\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"https://example.org/a.png\" TargetMode=\"External\"/>\
                 </Relationships>",
            ),
        ]);
        let r = content_a_save_drops(p.to_str().unwrap());
        assert_eq!(ids(&r), ["animations", "comments", "embedded-objects", "media", "missing-pictures", "smartart"]);
        let missing: Vec<&str> = r.features.iter().filter(|f| f.id == "missing-pictures").map(|f| f.detail.as_str()).collect();
        assert_eq!(missing, ["../media/gone.png is not in the file"], "the picture that is there and the linked one are fine");
        assert!(r.requires_confirmation());
        assert!(r.validate_save(false).is_err() && r.validate_save(true).is_ok());
    }

    #[test]
    fn an_odp_lists_comments_media_ole_animations_and_missing_pictures() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("theirs.odp");
        package(&p, &[
            ("mimetype", "application/vnd.oasis.opendocument.presentation"),
            ("Pictures/here.png", "png"),
            (
                "content.xml",
                "<office:document-content><draw:page>\
                 <officeooo:annotation/><draw:plugin xlink:href=\"v.mp4\"/><draw:object-ole/>\
                 <anim:par presentation:preset-class=\"motion-path\"/>\
                 <draw:frame><draw:image xlink:href=\"Pictures/here.png\"/></draw:frame>\
                 <draw:frame><draw:image xlink:href=\"Pictures/gone.png\"/></draw:frame>\
                 <draw:frame><draw:image xlink:href=\"https://example.org/a.png\"/></draw:frame>\
                 </draw:page></office:document-content>",
            ),
        ]);
        let r = content_a_save_drops(p.to_str().unwrap());
        assert_eq!(ids(&r), ["animations", "comments", "embedded-objects", "media", "missing-pictures"]);
        assert_eq!(r.features.iter().filter(|f| f.id == "missing-pictures").count(), 1);
    }

    #[test]
    fn entrance_and_exit_builds_and_charts_are_not_losses() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("builds.odp");
        package(&p, &[(
            "content.xml",
            "<office:document-content><anim:par presentation:preset-class=\"entrance\"/>\
             <draw:frame><draw:object xlink:href=\"./Object 1\"/></draw:frame></office:document-content>",
        )]);
        assert!(content_a_save_drops(p.to_str().unwrap()).features.is_empty());
    }

    #[test]
    fn relative_targets_resolve_against_the_part_folder() {
        assert_eq!(resolve("ppt/slides", "../media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve("ppt/slides", "/ppt/media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve("ppt/slides", "./b.png"), "ppt/slides/b.png");
    }
}
