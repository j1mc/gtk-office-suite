// SPDX-License-Identifier: GPL-3.0-or-later
//! The two-heads banner (RFC-0005 5.1): a lasting state of the document, so
//! a banner and not a toast (docs/GNOME-GUIDELINES.md §7).

use gtk4::prelude::*;
use libadwaita as adw;

/// A banner that says more than one version of the shared workbook exists
/// and whose button runs `app.collab-resolve`. Hidden until [`show`].
pub(crate) fn build() -> adw::Banner {
    adw::Banner::builder()
        .title(suite_common::i18n("Two versions of this workbook exist"))
        .button_label(suite_common::i18n("_Resolve…"))
        .action_name("app.collab-resolve")
        .revealed(false)
        .build()
}

/// Reveal or hide the banner: it is up exactly while the document has two
/// heads (`Heads { conflict: true }`), and goes when a merge makes one.
pub(crate) fn show(banner: &adw::Banner, conflict: bool) {
    banner.set_revealed(conflict);
}
