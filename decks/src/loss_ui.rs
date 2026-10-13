//! loss_ui.rs — ask before a save drops content Decks can't keep.
//! SPDX-License-Identifier: GPL-3.0-or-later
//!
//! decks-readiness.md: "Missing media and unsupported animation/comment
//! content is preserved or blocked/warned by #374 before save." What is
//! dropped is `decks_core::loss` (read from the file the deck came from);
//! this is the question, Letters' "Save as …?" for a format that can't hold
//! a document (`letters/src/window/saving.rs`). Cancel writes nothing.

use adw::prelude::*;
use gtk4::gio;
use libadwaita as adw;

/// Run `save` (a save to `target`), first asking when the file the deck
/// came from, `source`, has content the save would leave out. Inspection
/// failure blocks replacement, but permits an explicitly unverified copy
/// to a fresh, distinct destination. A new deck has no source.
/// Once a save has gone ahead the file is Decks' own, so the next save
/// doesn't ask.
pub(crate) fn save_after_asking(window: &adw::ApplicationWindow, source: Option<&str>, target: &str, save: impl FnOnce() + 'static) {
    let report = source.map(decks_core::loss::content_a_save_drops);
    if let Some(report) = report.as_ref().filter(|r| r.has_hard_errors()) {
        let details = report.features.iter()
            .filter(|f| f.disposition == suite_common::interop::FeatureDisposition::HardError)
            .map(|f| f.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let recovery_copy = source.is_some_and(|source| decks_core::loss::can_save_unverified_copy(source, target))
            && report.features.iter().filter(|f| f.disposition == suite_common::interop::FeatureDisposition::HardError)
                .all(|f| f.id == "source-inspection-failed");
        if recovery_copy {
            let body = format!(
                "{}\n\n{}",
                suite_common::i18n("Decks could not check the original file. This unverified copy may be missing content that was not imported. Save Copy writes your current edits to a new file without changing the original. The destination must not already exist."),
                details,
            );
            let dialog = adw::AlertDialog::new(Some(&suite_common::i18n("Save an Unverified Copy?")), Some(&body));
            dialog.add_response("cancel", &suite_common::i18n("_Cancel"));
            dialog.add_response("copy", &suite_common::i18n("_Save Copy"));
            dialog.set_response_appearance("copy", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");
            let (source, target, window) = (source.unwrap_or_default().to_string(), target.to_string(), window.clone());
            dialog.choose(Some(&window.clone()), None::<&gio::Cancellable>, move |response| {
                if response == "copy" {
                    if decks_core::loss::can_save_unverified_copy(&source, &target) {
                        save();
                    } else {
                        show_inspection_failure(&window, &details);
                    }
                }
            });
        } else {
            show_inspection_failure(window, &details);
        }
        return;
    }
    // One line per finding, not one label per feature: the choice needs the
    // affected slide/object/part and the reason (stable ids in
    // `decks_core::loss`), so a repair — a copy, a cancelled save, a fixed
    // source — can target that content. Planning never edits the deck and
    // never records history; it only reads the source file.
    let dropped: Vec<String> = report
        .as_ref()
        .map(|r| {
            let mut lines: Vec<String> = Vec::new();
            for f in r.destructive_features() {
                let line = format!("\u{2022} {}: {} \u{2014} {}", suite_common::i18n(&f.label), f.location, f.detail);
                if !lines.contains(&line) {
                    lines.push(line);
                }
            }
            lines
        })
        .unwrap_or_default();
    if dropped.is_empty() {
        save();
        return;
    }
    // Consent is scoped to this source revision, target and feature set: the
    // source is re-inspected when the user confirms, and anything else —
    // a failed inspection, a changed finding list — blocks the save instead
    // of reusing consent given for a different report. Cancelling writes
    // nothing and keeps dirty edits, source bytes and recovery state.
    let consented: Vec<(String, String, String)> = report
        .as_ref()
        .map(|r| {
            r.destructive_features().iter().map(|f| (f.id.clone(), f.location.clone(), f.detail.clone())).collect()
        })
        .unwrap_or_default();
    let source = source.unwrap_or_default();
    let name = |p: &str| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let outcome = if source == target {
        suite_common::i18n("Saving replaces the file without them. Save a copy instead to keep the original as it is.")
    } else {
        suite_common::i18n("The new file will not have them; %s keeps them.").replace("%s", &name(source))
    };
    let body = format!(
        "{}\n\n{}\n\n{}",
        suite_common::i18n("%s has content Decks can't keep:").replace("%s", &name(source)),
        dropped.join("\n"),
        outcome
    );
    let dialog = adw::AlertDialog::new(Some(&suite_common::i18n("Save Without This Content?")), Some(&body));
    dialog.add_response("cancel", &suite_common::i18n("_Cancel"));
    dialog.add_response("save", &suite_common::i18n("_Save Anyway"));
    dialog.set_response_appearance("save", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let (source_path, window) = (source.to_string(), window.clone());
    let window_for_save = window.clone();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        if response == "save" {
            let fresh = decks_core::loss::content_a_save_drops(&source_path);
            if fresh.has_hard_errors() {
                let details = fresh.features.iter()
                    .filter(|f| f.disposition == suite_common::interop::FeatureDisposition::HardError)
                    .map(|f| f.detail.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                show_inspection_failure(&window_for_save, &details);
            } else {
                let now: Vec<(String, String, String)> = fresh.destructive_features().iter()
                    .map(|f| (f.id.clone(), f.location.clone(), f.detail.clone())).collect();
                if now == consented {
                    save();
                } else {
                    show_source_changed(&window_for_save);
                }
            }
        }
    });
}

fn show_source_changed(window: &adw::ApplicationWindow) {
    let body = suite_common::i18n("The original file changed while the question was open, so the listed content may no longer match. Nothing was saved. Try saving again to review what changed.");
    let dialog = adw::AlertDialog::new(Some(&suite_common::i18n("Source Changed")), Some(&body));
    dialog.add_response("ok", &suite_common::i18n("_OK"));
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(Some(window));
}

fn show_inspection_failure(window: &adw::ApplicationWindow, details: &str) {
    let body = format!(
        "{}\n\n{}",
        suite_common::i18n("Decks could not check the original file for content this save might lose. Nothing was saved. Make the original file available and try again, or use Save As with a new filename to save an unverified copy of your current edits. An unverified copy cannot replace an existing file."),
        details,
    );
    let dialog = adw::AlertDialog::new(Some(&suite_common::i18n("Could Not Save File")), Some(&body));
    dialog.add_response("ok", &suite_common::i18n("_OK"));
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("ok");
    dialog.present(Some(window));
}
