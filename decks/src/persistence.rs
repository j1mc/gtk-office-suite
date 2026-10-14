// persistence.rs — where a Decks window's crash-recovery snapshots live.
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Split out of window.rs (#168, #285), which had grown past the 1,800-line
// ceiling the release contract enforces. The same split as
// `tables/src/persistence.rs`: the per-window identity deciding where an
// autosave snapshot lands, and the format it is written in.

// ── Crash-recovery snapshots ─────────────────────────────────────────────
// Unique across launches, not only within one (see new_doc_id).
pub(crate) fn next_doc_id() -> String {
    suite_common::autosave::new_doc_id()
}

pub(crate) fn autosave_state_dir() -> std::path::PathBuf {
    // XDG state dir, never a fixed name in /tmp (#829): see the shared helper.
    suite_common::autosave::state_dir("decks")
}

pub(crate) fn autosave_format_hint(path: &Option<String>) -> String {
    match path {
        Some(p) if p.to_lowercase().ends_with(".odp") => "odp".to_string(),
        _ => "pptx".to_string(),
    }
}

// ── Autosave: one crash-recovery snapshot attempt ──────────────────────────
// Shared by the manual `autosave-now` action and the unattended timer so
// the two cannot drift apart. Serialization AND slot-write failures both
// reach the notifier: a snapshot that silently never lands is work the
// user believes is protected. The prior valid generation stays — the
// slot's single atomic write only ever replaces it on success (DCK-003).
pub(crate) fn autosave_once(
    controller: &decks_core::DecksController,
    dirty: &std::cell::Cell<bool>,
    slot: &suite_common::autosave::AutosaveSlot,
    notices: &suite_common::autosave_notice::AutosaveNotifier,
    path: Option<String>,
) {
    if !dirty.get() {
        return;
    }
    // Consistent-revision capture: serialization runs here on the GTK
    // thread, so the revision cannot advance mid-clone; the second read
    // guards a future background move, which must never misattribute a
    // newer revision to these bytes.
    let checkpoint = controller.checkpoint();
    let deck = controller.deck();
    if checkpoint != controller.checkpoint() {
        notices.record(Err(
            "the deck changed while its snapshot was being written; retrying next tick".to_string(),
        ));
        return;
    }
    let kind = autosave_format_hint(&path);
    match decks_core::write_deck_bytes(&kind, &deck) {
        Ok(bytes) => {
            let meta = suite_common::autosave::SnapshotMeta {
                original_path: path.map(std::path::PathBuf::from),
                kind,
            };
            notices.record(slot.write(&bytes, &meta));
        }
        Err(reason) => notices.record(Err(format!("could not snapshot the deck: {reason}"))),
    }
}
