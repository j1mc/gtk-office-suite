//! loss_ui.rs — ask before a save drops content Tables can't keep (#1272).
//! SPDX-License-Identifier: GPL-3.0-or-later
//!
//! What is dropped is `tables_core::io::loss` (read from the workbook's
//! model and from the file it came from); this is the question, as Decks
//! (`decks/src/loss_ui.rs`) and Letters ask theirs. Cancel writes nothing:
//! the file keeps its bytes and the window stays unsaved.
//!
//! TBL-001: every listed loss names the affected sheet or package part and
//! what happens to it, so the choice is an informed one. When the original
//! cannot be inspected at all, there is no informed choice to offer: the
//! save is blocked instead of blessed blind.

use adw::prelude::*;
use gtk4::{self as gtk, gio};
use libadwaita as adw;

use crate::window::AppState;

/// Run `save` (a save to `target`), first asking when the model or the
/// file the workbook came from, `source`, holds content the xlsx save
/// would leave out.
pub(crate) fn save_after_asking(window: &impl IsA<gtk::Widget>, source: Option<&str>, target: &str, state: &AppState, save: impl FnOnce() + 'static) {
    let sheets: Vec<tables_core::sheet::SheetModel> = state.sheets.iter().map(|s| s.borrow().clone()).collect();
    let report = tables_core::io::loss::content_a_save_drops(source, &sheets);
    // Fail closed: an uninspectable original means unknown loss, and an
    // unknown loss is not something a dialog can ask about. Offer no
    // destructive choice at all; the save callback never runs.
    if report.has_hard_errors() {
        let mut blocked: Vec<String> = Vec::new();
        for f in report.features.iter().filter(|f| f.disposition == suite_common::interop::FeatureDisposition::HardError) {
            blocked.push(format!("\u{2022} {} — {}: {}", suite_common::i18n(&f.label), f.location, f.detail));
        }
        let body = format!("{}\n\n{}", suite_common::i18n("Tables could not check what the original holds, so saving might silently drop content."), blocked.join("\n"));
        let dialog = adw::AlertDialog::new(Some(&suite_common::i18n("Cannot Verify the Original")), Some(&body));
        dialog.add_response("ok", &suite_common::i18n("_OK"));
        dialog.set_default_response(Some("ok"));
        dialog.set_close_response("ok");
        dialog.choose(Some(window), None::<&gio::Cancellable>, move |_| {});
        return;
    }
    // One line per loss: what it is, where it is, and what the save does
    // to it. Locations are sheet names for model content and part paths
    // for source-package content.
    let mut dropped: Vec<String> = Vec::new();
    for f in report.destructive_features() {
        let line = format!("\u{2022} {} — {}: {}", suite_common::i18n(&f.label), f.location, f.detail);
        if !dropped.contains(&line) {
            dropped.push(line);
        }
    }
    if dropped.is_empty() {
        save();
        return;
    }
    let name = |p: &str| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let what = source.map(name).unwrap_or_else(|| suite_common::i18n("This workbook"));
    let outcome = match source {
        Some(s) if s == target => suite_common::i18n("Saving replaces the file without them. Save a copy instead to keep the original as it is."),
        Some(s) => suite_common::i18n("‘%t’ will not have them; ‘%s’ keeps them untouched.")
            .replace("%t", &name(target))
            .replace("%s", &name(s)),
        None => suite_common::i18n("The saved file will not have them."),
    };
    let body = format!("{}\n\n{}\n\n{}", suite_common::i18n("%s has content Tables can't keep:").replace("%s", &what), dropped.join("\n"), outcome);
    let dialog = adw::AlertDialog::new(Some(&suite_common::i18n("Save Without This Content?")), Some(&body));
    dialog.add_response("cancel", &suite_common::i18n("_Cancel"));
    dialog.add_response("save", &suite_common::i18n("_Save Anyway"));
    dialog.set_response_appearance("save", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.choose(Some(window), None::<&gio::Cancellable>, move |response| {
        if response == "save" {
            save();
        }
    });
}
