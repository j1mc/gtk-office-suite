// session.rs — canonical per-tab document identity for Letters.
// SPDX-License-Identifier: GPL-3.0-or-later
//
// First slice of issue #103 for Letters. Letters' actual document
// *content* model is a GtkTextBuffer (rich text + formatting tags), which
// has no GTK-free representation today — that's a larger design question
// (mirror buffer state into a GTK-free AST kept in sync, or accept
// GtkTextBuffer itself as the content boundary) than a single slice
// should resolve. What *is* cleanly GTK-free, and was living directly in
// `letters/src/window.rs` despite having no GTK dependency in any of its
// field types, is per-tab session identity: which file a tab is backed
// by, whether it's mid-close-after-save, and its autosave slot. Moved
// here so it's canonical, testable, and outside the app crate.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use suite_common_core::autosave::AutosaveSlot;
use suite_common_core::carry::Carried;
use suite_common_core::interop::CompatibilityReport;

/// What the tab's file held at open that the model does not, pinned to the
/// exact bytes it was read from (LET-001). Two halves:
///
/// * the safe subset a save carries back (`Carried`: custom XML, thumbnail,
///   ODF settings);
/// * the scan of what no save can keep (`loss::content_a_save_drops`:
///   macros, embedded objects, charts/SmartArt, nested tables).
///
/// Both used to be re-read from the live source path at save/dialog time,
/// so a file deleted or replaced while open either warned about the wrong
/// bytes or carried bytes the user never opened. The session owns this
/// context; `LiveModel` owns content. After a successful `save_to` it is
/// re-pinned to the bytes just written, which are the model's own — a file
/// Letters wrote has nothing left to warn about.
#[derive(Clone, Debug, Default)]
pub struct ImportProvenance {
    source: PathBuf,
    len: u64,
    hash: u64,
    /// False when the source could not even be read at open; the context
/// then carries nothing and behaves as no context.
    has_bytes: bool,
    carried: Carried,
    source_loss: CompatibilityReport,
}

/// The live source path measured against the imported revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceState {
    /// Untitled documents, or a context that captured nothing: callers fall
/// back to the old live read.
    NoContext,
    /// On-disk bytes still match the imported revision.
    Intact,
    /// The source cannot be read: deleted, renamed away, or unreadable.
/// Carried parts are unavailable; the dialog must say the original is
/// unrecoverable rather than warn about bytes that are not there.
    Missing,
    /// The source reads back with different bytes. Never carry from it:
/// those are not the bytes the user opened.
    Replaced,
}

/// FNV-1a over the whole file: no new dependency for an open-vs-save
/// comparison made in the same process. Length is compared too, so equal
/// hashes of different lengths cannot pass.
fn fingerprint(bytes: &[u8]) -> (u64, u64) {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    (bytes.len() as u64, hash)
}

 pub struct DocumentSession {
     pub file: Option<PathBuf>,
     pub closing_after_save: bool,
     pub autosave_slot: Rc<AutosaveSlot>,
     /// Ownership of the snapshot slot, held for as long as the tab is
     /// open so that another launch does not offer this document — which
     /// is open right here — back as a crash recovery. `None` only if the
     /// claim could not be taken, which `claim()` treats as "carry on".
     _autosave_owner: Option<suite_common_core::autosave::SnapshotOwner>,
    /// The imported revision's carried parts + loss findings (LET-001).
/// `None` for untitled tabs and recovered snapshots with no original path.
    import_context: Option<ImportProvenance>,
}

/// A committed user save may still need a recovery-cleanup warning. Cleanup
/// failure does not undo a successful file write or change its destination.
pub struct SaveCommit {
    pub recovery_warning: Option<String>,
}

impl DocumentSession {
    pub fn new(autosave_slot: Rc<AutosaveSlot>) -> Self {
        let _autosave_owner = autosave_slot.claim();
        Self {
            file: None,
            closing_after_save: false,
            autosave_slot,
            _autosave_owner,
            import_context: None,
        }
    }

    pub fn file_name(&self) -> Option<String> {
        self.file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
    }

    /// Pin the carried parts + loss findings to the revision at `source`
/// right now. Window open paths call this once the document has loaded
/// and store the result with [`set_import_context`]; the save path never
/// re-reads the live file for either.
    pub fn capture_import_context(source: &Path) -> ImportProvenance {
        let (len, hash, has_bytes) = match std::fs::read(source) {
            Ok(bytes) => {
                let (len, hash) = fingerprint(&bytes);
                (len, hash, true)
            }
            Err(_) => (0, 0, false),
        };
        ImportProvenance {
            source: source.to_path_buf(),
            len,
            hash,
            has_bytes,
            carried: suite_common_core::carry::capture(source),
            source_loss: crate::loss::content_a_save_drops(source),
        }
    }

    /// Store the import context beside the tab's identity. Content stays
/// with `LiveModel`; this is provenance only.
    pub fn set_import_context(&mut self, context: ImportProvenance) {
        self.import_context = Some(context);
    }

    pub fn import_context(&self) -> Option<&ImportProvenance> {
        self.import_context.as_ref()
    }

    /// Measure the live source path against the imported revision. Never
/// carry bytes from a path that no longer resolves to it.
    pub fn source_state(&self) -> SourceState {
        let Some(context) = self.import_context.as_ref() else {
            return SourceState::NoContext;
        };
        if !context.has_bytes {
            return SourceState::NoContext;
        }
        match std::fs::read(&context.source) {
            Ok(bytes) => {
                let (len, hash) = fingerprint(&bytes);
                if len == context.len && hash == context.hash {
                    SourceState::Intact
                } else {
                    SourceState::Replaced
                }
            }
            Err(_) => SourceState::Missing,
        }
    }

    /// The carried parts a save may write: the imported revision's, and
/// only while its source still resolves to that revision. Missing or
/// replaced sources carry nothing — writing the old bytes over a changed
/// file, or resurrecting a deleted file's parts unasked, would be a
/// silent carry. `Carried::apply` already drops parts whose family does
/// not match the target (a docx saved as odt), so Save As needs no
/// further gating here.
    pub fn carried_for_save(&self) -> Carried {
        match self.source_state() {
            SourceState::Intact => self
                .import_context
                .as_ref()
                .map(|c| c.carried.clone())
                .unwrap_or_default(),
            _ => Carried::default(),
        }
    }

    /// The loss the imported revision's source was scanned with at open.
/// The save dialog shows this instead of re-scanning the live path.
    pub fn stored_source_loss(&self) -> Option<&CompatibilityReport> {
        self.import_context.as_ref().map(|c| &c.source_loss)
    }

    /// Whether `target` is the file the context was captured from: only
/// then does a Save-a-Copy offer make sense (a Save As elsewhere already
/// is one).
    pub fn provenance_source_matches(&self, target: &Path) -> bool {
        self.import_context.as_ref().is_some_and(|c| c.source == target)
    }

    /// File name of the provenance source, for dialog copy.
    pub fn provenance_source_name(&self) -> Option<String> {
        self.import_context.as_ref().and_then(|c| {
            c.source.file_name().map(|n| n.to_string_lossy().into_owned())
        })
    }

    /// Names of the parts the imported revision carried, whatever the live
/// source looks like now — for naming what's at stake, never for writing.
    pub fn imported_carried_names(&self) -> Vec<String> {
        self.import_context
            .as_ref()
            .map(|c| c.carried.names().map(|n| n.to_string()).collect())
            .unwrap_or_default()
    }

    /// Whether the imported revision held anything a save cannot rebuild
/// from the model: carried parts or scanned loss. Only then does a
/// missing or replaced source change what the dialog must say; an
/// ordinary package saves on without asking, as before.
    pub fn has_unrecoverable_content(&self) -> bool {
        self.import_context.as_ref().is_some_and(|c| {
            !c.carried.is_empty() || !c.source_loss.destructive_features().is_empty()
        })
    }

    /// Commit identity and retire the recovery checkpoint only after the
/// supplied writer succeeds. The GTK adapter marks its buffer clean only
/// after this returns Ok; on Err all session/recovery state stays intact.
    pub fn save_to(
        &mut self,
        path: PathBuf,
        write: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<SaveCommit, String> {
        write(&path)?;
        self.file = Some(path.clone());
        // The bytes on disk are now the model's own write (plus the parts
        // just carried into it): re-pin provenance to them, so the next save
        // neither warns about the old source's content nor carries the old
        // revision's parts. A `commit_save` (remote upload) failure arrives as
        // the writer's Err above, so it never reaches this re-pinning: identity,
        // checkpoint and provenance all stay as they were.
        self.import_context = Some(Self::capture_import_context(&path));
        Ok(SaveCommit {
            recovery_warning: self.autosave_slot.clear().err(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn slot() -> Rc<AutosaveSlot> {
        Rc::new(AutosaveSlot::new(
            std::env::temp_dir(),
            "test-doc".to_string(),
        ))
    }

    #[test]
    fn new_session_has_no_file() {
        let s = DocumentSession::new(slot());
        assert!(s.file.is_none());
        assert!(!s.closing_after_save);
        assert_eq!(s.file_name(), None);
    }

    #[test]
    fn file_name_extracts_basename() {
        let mut s = DocumentSession::new(slot());
        s.file = Some(Path::new("/tmp/some/dir/report.docx").to_path_buf());
        assert_eq!(s.file_name(), Some("report.docx".to_string()));
    }

    #[test]
    fn failed_save_keeps_original_identity_and_recovery_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let slot = Rc::new(AutosaveSlot::new(dir.path(), "document"));
        let meta = suite_common_core::autosave::SnapshotMeta {
            original_path: Some(PathBuf::from("original.odt")),
            kind: "json".into(),
        };
        slot.write(b"unsaved styled document", &meta).unwrap();
        let mut session = DocumentSession::new(slot.clone());
        session.file = meta.original_path.clone();
        let result = session.save_to(PathBuf::from("new.docx"), |path| {
            assert_eq!(path, Path::new("new.docx"));
            Err("disk full".into())
        });
        assert!(result.is_err());
        assert_eq!(session.file, meta.original_path);
        assert_eq!(
            slot.read(),
            Some((b"unsaved styled document".to_vec(), meta))
        );
    }

    #[test]
    fn successful_save_commits_identity_and_retires_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let slot = Rc::new(AutosaveSlot::new(dir.path(), "document"));
        slot.write(
            b"draft",
            &suite_common_core::autosave::SnapshotMeta {
                original_path: None,
                kind: "json".into(),
            },
        )
        .unwrap();
        let mut session = DocumentSession::new(slot.clone());
        let destination = dir.path().join("saved.odt");
        let result = session
            .save_to(destination.clone(), |path| {
                std::fs::write(path, b"saved content").map_err(|e| e.to_string())
            })
            .unwrap();
        assert!(result.recovery_warning.is_none());
        assert_eq!(session.file, Some(destination.clone()));
        assert_eq!(std::fs::read(destination).unwrap(), b"saved content");
        assert!(slot.read().is_none());
    }

    #[test]
    fn recovery_cleanup_failure_is_reported_after_successful_save() {
        let dir = tempfile::tempdir().unwrap();
        let slot = Rc::new(AutosaveSlot::new(dir.path(), "document"));
        // A nonempty directory in the snapshot slot deterministically makes
        // cleanup fail, including when tests run as root.
        std::fs::create_dir(dir.path().join("document.snapshot")).unwrap();
        let mut session = DocumentSession::new(slot);
        let result = session
            .save_to(PathBuf::from("saved.odt"), |_| Ok(()))
            .unwrap();
        assert!(result.recovery_warning.is_some());
        assert_eq!(session.file, Some(PathBuf::from("saved.odt")));
    }

    /// A docx holding what Letters never reads (a macro project: loss) and
/// what it carries byte-identical (a custom XML part: carried).
    fn docx_with_unmodelled_parts(path: &Path) {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            ("[Content_Types].xml", "types"),
            (
                "word/document.xml",
                "<w:document xmlns:w=\"w\"><w:body><w:p/></w:body></w:document>",
            ),
            ("customXml/item1.xml", "<props/>"),
            ("word/vbaProject.bin", "macro-bytes"),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn session_with_import(source: &Path) -> DocumentSession {
        let mut session = DocumentSession::new(slot());
        session.file = Some(source.to_path_buf());
        session.set_import_context(DocumentSession::capture_import_context(source));
        session
    }

    fn loss_ids(session: &DocumentSession) -> Vec<String> {
        session
            .stored_source_loss()
            .map(|r| r.destructive_features().iter().map(|f| f.id.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn import_context_binds_carried_parts_and_loss_to_the_imported_revision() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("report.docx");
        docx_with_unmodelled_parts(&source);
        let session = session_with_import(&source);
        assert_eq!(session.source_state(), SourceState::Intact);
        let carried: Vec<String> = session.carried_for_save().names().map(|n| n.to_string()).collect();
        assert_eq!(carried, ["customXml/item1.xml"]);
        assert_eq!(loss_ids(&session), ["macros"]);
        assert!(session.has_unrecoverable_content());
        assert!(session.provenance_source_matches(&source));
        assert!(!session.provenance_source_matches(&dir.path().join("copy.docx")));
    }

    #[test]
    fn source_replaced_between_open_and_save_refuses_silent_carry() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("report.docx");
        docx_with_unmodelled_parts(&source);
        let session = session_with_import(&source);
        // Replaced by bytes that are not even a package, then by a valid
        // package with none of the imported revision's parts: either way the
        // old bytes must not be carried, while the dialog still names what the
        // imported revision held.
        std::fs::write(&source, b"entirely different bytes").unwrap();
        assert_eq!(session.source_state(), SourceState::Replaced);
        assert!(session.carried_for_save().is_empty(), "replaced bytes must never be carried");
        assert_eq!(loss_ids(&session), ["macros"]);
        docx_with_unmodelled_parts(&dir.path().join("other.docx"));
        std::fs::rename(dir.path().join("other.docx"), &source).unwrap();
        // Same length is not the same revision; only identical bytes pass.
        assert!(session.carried_for_save().is_empty(), "a different package must never be carried");
        assert_eq!(loss_ids(&session), ["macros"]);
    }

    #[test]
    fn missing_source_is_reported_unrecoverable_and_carries_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("report.docx");
        docx_with_unmodelled_parts(&source);
        let session = session_with_import(&source);
        std::fs::remove_file(&source).unwrap();
        assert_eq!(session.source_state(), SourceState::Missing);
        assert!(session.carried_for_save().is_empty(), "a missing source has no parts to carry");
        assert!(session.has_unrecoverable_content());
        assert_eq!(loss_ids(&session), ["macros"], "the dialog still names the imported revision's loss");
    }

    #[test]
    fn ordinary_package_without_unmodelled_content_saves_on_without_asking() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("plain.docx");
        let doc = crate::model::Document::default();
        crate::save::write(&doc, &source).unwrap();
        let session = session_with_import(&source);
        assert_eq!(session.source_state(), SourceState::Intact);
        assert!(!session.has_unrecoverable_content());
        assert!(loss_ids(&session).is_empty());
    }

    #[test]
    fn remote_commit_failure_keeps_identity_and_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("report.docx");
        docx_with_unmodelled_parts(&original);
        let slot = Rc::new(AutosaveSlot::new(dir.path(), "document"));
        let meta = suite_common_core::autosave::SnapshotMeta {
            original_path: Some(original.clone()),
            kind: "json".into(),
        };
        slot.write(b"unsaved styled document", &meta).unwrap();
        let mut session = DocumentSession::new(slot.clone());
        session.file = Some(original.clone());
        session.set_import_context(DocumentSession::capture_import_context(&original));
        let staged = dir.path().join("staged.docx");
        // The writer (model bytes plus carried parts) succeeds; the remote
        // upload (`locations::commit_save`) then refuses. `save_to` sees one
        // closure, so the test stages a real write before the commit error.
        let result = session.save_to(staged.clone(), |path| {
            std::fs::write(path, b"staged model bytes").map_err(|e| e.to_string())?;
            Err("Can't save to https://example.invalid/docs/report.docx: changed elsewhere".to_string())
        });
        assert!(result.is_err());
        assert_eq!(session.file, Some(original.clone()), "identity must not advance on commit failure");
        assert_eq!(
            slot.read(),
            Some((b"unsaved styled document".to_vec(), meta)),
            "the recovery checkpoint must survive a commit failure"
        );
        assert_eq!(session.source_state(), SourceState::Intact, "provenance stays pinned to the original");
        assert!(session.provenance_source_matches(&original));
    }

    #[test]
    fn save_as_across_formats_recomputes_both_reports() {
        use crate::model::{Paragraph, ParaStyle, Run, RunStyle};
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("report.docx");
        docx_with_unmodelled_parts(&source);
        let session = session_with_import(&source);
        let mut doc = crate::model::Document::from_plain_text("");
        doc.paragraphs = vec![Paragraph {
            style: ParaStyle::default(),
            runs: vec![Run { text: "bold".into(), style: RunStyle { bold: true, ..Default::default() } }],
        }];
        // The model report is per target format: bold survives docx, dies
        // in plain text.
        let docx_report = crate::save::compatibility_report(&doc, crate::save::SaveFormat::Docx);
        assert!(docx_report.destructive_features().is_empty());
        let txt_report = crate::save::compatibility_report(&doc, crate::save::SaveFormat::PlainText);
        assert!(txt_report.destructive_features().iter().any(|f| f.id == "character-formatting"));
        // The source report is the imported revision's, whatever the target:
        // a Save As leaves the macro behind in both cases.
        assert_eq!(loss_ids(&session), ["macros"]);
        assert!(session.has_unrecoverable_content());
    }
}
