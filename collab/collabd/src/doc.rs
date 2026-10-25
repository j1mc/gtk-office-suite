//! One task per open document.
//!
//! The spike's `DocumentStore` has no push (RFC-0005 2.1), so each document has
//! its own loop: publish what is queued, read the store, then wait for news
//! (`DocumentStore::wait_for_change`: a long poll on Durable Streams) or for the
//! shell's next message. Reading turns the store's version list into `heads`
//! events (`dcp_core::view::build`) and its update log into `genesis`, `update`,
//! `dropped` and `gap` events (`dcp_core::updates::CrdtSession`, whose base-version
//! rule is `docs/collab-spec/profiles/loro-1.md` section 3).
//!
//! Nothing the shell sends is lost while the server is away: updates and versions
//! wait in a `dcp_core::outbox::Outbox` and go out in order once it is back.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dcp_core::graph::Options;
use dcp_core::outbox::{Outbox, Step};
use dcp_core::updates::{
    inline_payload, CrdtSession, Dropped, Files, Skip, Target, MAX_INLINE_BYTES,
};
use dcp_core::{
    integrity, publish, view, DocumentStore, FileRef, NewFile, Payload, Role, Stamped, StoreError,
    Update, Version,
};
use suite_collab::{
    Author, ConnState, DropKind, ErrorCode, Event, HeadInfo, PublishKind,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::connector::code_of;
use crate::output::Output;
use crate::{log, now_ms, ALGORITHM, DOC_MODELS};

/// What the daemon forwards to a document's task.
pub enum DocMsg {
    PublishUpdate {
        req: Option<u64>,
        bytes: Vec<u8>,
    },
    PublishVersion {
        req: Option<u64>,
        bytes: Vec<u8>,
        message: Option<String>,
        resolve: Vec<String>,
    },
    FetchVersion {
        req: Option<u64>,
        id: String,
    },
    Applied {
        id: String,
    },
    Rejected {
        id: String,
        why: String,
    },
    Resync {
        req: Option<u64>,
    },
    Leave {
        req: Option<u64>,
    },
}

pub struct DocHandle {
    pub tx: mpsc::UnboundedSender<DocMsg>,
    pub task: JoinHandle<()>,
}

/// Everything a document's task starts from.
pub struct Start {
    pub doc: String,
    pub store: Arc<dyn DocumentStore>,
    pub out: Output,
    pub me: Author,
    pub role: Role,
    /// A sharer holds its session from the start; a joiner finds the genesis in the log.
    pub session: Option<CrdtSession>,
    pub genesis_id: Option<String>,
    /// The version a save is a child of. `None`: the newest head once it is known.
    pub opened: Option<String>,
    /// The document's MIME type (what a saved version's file is).
    pub mime: String,
}

pub fn spawn(start: Start) -> DocHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let doc = Doc::new(start);
    let task = tokio::spawn(doc.run(rx));
    DocHandle { tx, task }
}

/// What is waiting to go to the store, in the order the shell asked.
enum Outgoing {
    Update {
        req: Option<u64>,
        seq: u64,
        bytes: Vec<u8>,
        /// The payload file once it is uploaded, so a retry of the publish does
        /// not upload it again.
        file: Mutex<Option<FileRef>>,
    },
    Version {
        req: Option<u64>,
        bytes: Vec<u8>,
        message: Option<String>,
        resolve: Vec<String>,
    },
}

impl Outgoing {
    fn req(&self) -> Option<u64> {
        match self {
            Outgoing::Update { req, .. } | Outgoing::Version { req, .. } => *req,
        }
    }
}

/// The receiver's view of this sidecar's own algorithm. The dry run of DCP spec
/// 7 rule 3 is the shell's (RFC-0005 2.2): the sidecar has no Loro, so here it is a
/// structural pass-through and `apply` hands the bytes on.
#[derive(Default)]
struct Collect {
    applied: Vec<Vec<u8>>,
}

impl Target for Collect {
    fn supports(&self, algorithm: &str, doc_model: &str) -> bool {
        algorithm == ALGORITHM && DOC_MODELS.contains(&doc_model)
    }
    fn dry_run(&self, _: &str, _: &str, _: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn apply(&mut self, _: &str, _: &str, payload: &[u8]) {
        self.applied.push(payload.to_vec());
    }
}

/// The genesis of a session (loro/1 profile section 4): update `seq 0`, a `file`
/// payload, written by the device that wrote the root version it is based on.
/// The first such update in log order.
pub fn find_genesis<'a>(
    updates: &'a [Stamped<Update>],
    versions: &[Stamped<Version>],
) -> Option<&'a Stamped<Update>> {
    updates.iter().find(|u| {
        u.body.algorithm == ALGORITHM
            && u.body.seq == 0
            && matches!(u.body.payload, Payload::File { .. })
            && versions.iter().any(|v| {
                v.id == u.body.base_version
                    && v.body.parents.is_empty()
                    && v.body.created_by.principal == u.sender
                    && u.device.as_deref() == Some(v.body.created_by.device.as_str())
            })
    })
}

struct Doc {
    doc: String,
    store: Arc<dyn DocumentStore>,
    out: Output,
    me: Author,
    role: Role,
    session: Option<CrdtSession>,
    genesis_id: Option<String>,
    opened: Option<String>,
    mime: String,

    versions: Vec<Stamped<Version>>,
    version_ids: Option<Vec<String>>,
    head_ids: Option<Vec<String>>,
    file_cache: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    reported_versions: HashSet<String>,
    reported_records: HashSet<(String, String, String)>,
    resolved: Files,
    awaiting: BTreeSet<String>,

    outbox: Outbox<Arc<Outgoing>>,
    online: Option<bool>,
    failures: u32,
    told_auth: bool,
}

/// Streams has no rate limit; the outbox's pacing is one event per millisecond.
const OUTBOX_EVENTS_PER_SECOND: u64 = 1000;
const NEWS_WAIT: Duration = Duration::from_secs(20);

impl Doc {
    fn new(s: Start) -> Self {
        Doc {
            doc: s.doc,
            store: s.store,
            out: s.out,
            me: s.me,
            role: s.role,
            session: s.session,
            genesis_id: s.genesis_id,
            opened: s.opened,
            mime: s.mime,
            versions: Vec::new(),
            version_ids: None,
            head_ids: None,
            file_cache: Arc::new(Mutex::new(HashMap::new())),
            reported_versions: HashSet::new(),
            reported_records: HashSet::new(),
            resolved: Files::new(),
            awaiting: BTreeSet::new(),
            outbox: Outbox::new(OUTBOX_EVENTS_PER_SECOND),
            online: None,
            failures: 0,
            told_auth: false,
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<DocMsg>) {
        let mut need_poll = true;
        let mut retry_at: Option<Instant> = None;
        loop {
            let outbox_wake = self.flush().await;
            if need_poll && retry_at.is_none_or(|t| Instant::now() >= t) {
                match self.poll().await {
                    Ok(()) => {
                        self.set_online();
                        need_poll = false;
                        retry_at = None;
                        self.failures = 0;
                        self.told_auth = false;
                    }
                    Err(e) => retry_at = Some(Instant::now() + self.poll_failed(&e)),
                }
            }
            let wake = [outbox_wake, retry_at.filter(|_| need_poll)]
                .into_iter()
                .flatten()
                .min();
            // Owned clones, so the futures below do not borrow `self`.
            let (store, doc) = (self.store.clone(), self.doc.clone());
            let wait_for_news = !need_poll;
            tokio::select! {
                m = rx.recv() => match m {
                    None => break,
                    Some(DocMsg::Leave { req }) => {
                        self.out.send(req, Event::Done { doc: Some(self.doc.clone()) });
                        break;
                    }
                    Some(m) => {
                        if self.on_msg(m).await {
                            need_poll = true;
                            retry_at = None;
                        }
                    }
                },
                r = store.wait_for_change(&doc, NEWS_WAIT), if wait_for_news => {
                    need_poll = true;
                    if let Err(e) = r {
                        retry_at = Some(Instant::now() + self.poll_failed(&e));
                    }
                }
                _ = async {
                    match wake {
                        Some(t) => tokio::time::sleep_until(tokio::time::Instant::from_std(t)).await,
                        None => std::future::pending().await,
                    }
                } => {}
            }
        }
    }

    // ----------------------------------------------------------- state events

    fn event(&self, e: Event) {
        self.out.send(None, e);
    }

    fn set_online(&mut self) {
        if self.online != Some(true) {
            self.online = Some(true);
            self.event(Event::State {
                doc: Some(self.doc.clone()),
                state: ConnState::Online,
            });
        }
    }

    fn set_offline(&mut self) {
        if self.online != Some(false) {
            self.online = Some(false);
            self.event(Event::State {
                doc: Some(self.doc.clone()),
                state: ConnState::Offline { since_ms: now_ms() },
            });
        }
    }

    /// Say what a failed read means to the shell and how long to wait before the next try.
    fn poll_failed(&mut self, e: &StoreError) -> Duration {
        self.failures += 1;
        match e {
            StoreError::Transport(_) | StoreError::RateLimited { .. } => {
                log!("{}: {e}", self.doc);
                self.set_offline();
                Duration::from_millis(250u64 << (self.failures - 1).min(5)).min(Duration::from_secs(5))
            }
            StoreError::Forbidden(m) => {
                if !self.told_auth {
                    self.told_auth = true;
                    self.online = Some(false);
                    self.event(Event::State {
                        doc: Some(self.doc.clone()),
                        state: ConnState::AuthRequired,
                    });
                    self.event(Event::error(ErrorCode::Forbidden, m.clone()));
                }
                Duration::from_secs(10)
            }
            other => {
                log!("{}: {other}", self.doc);
                self.online = Some(false);
                self.event(Event::State {
                    doc: Some(self.doc.clone()),
                    state: ConnState::Error {
                        message: other.to_string(),
                    },
                });
                Duration::from_secs(10)
            }
        }
    }

    // ----------------------------------------------------------- shell messages

    /// Returns whether the store should be read again at once.
    async fn on_msg(&mut self, m: DocMsg) -> bool {
        match m {
            DocMsg::PublishUpdate { req, bytes } => {
                if self.role < Role::Editor {
                    self.out.send(
                        req,
                        Event::error(
                            ErrorCode::Forbidden,
                            format!("your role on this document is {:?}: it cannot be edited", self.role),
                        ),
                    );
                    return false;
                }
                let Some(session) = self.session.as_mut() else {
                    self.out.send(
                        req,
                        Event::error(
                            ErrorCode::Invalid,
                            "this document has no live session yet (its genesis has not been read)",
                        ),
                    );
                    return false;
                };
                let seq = session.reserve();
                self.outbox.push(Arc::new(Outgoing::Update {
                    req,
                    seq,
                    bytes,
                    file: Mutex::new(None),
                }));
                false
            }
            DocMsg::PublishVersion {
                req,
                bytes,
                message,
                resolve,
            } => {
                if self.role < Role::Editor {
                    self.out.send(
                        req,
                        Event::error(ErrorCode::Forbidden, "your role on this document cannot save a version"),
                    );
                    return false;
                }
                self.outbox.push(Arc::new(Outgoing::Version {
                    req,
                    bytes,
                    message,
                    resolve,
                }));
                false
            }
            DocMsg::FetchVersion { req, id } => {
                self.fetch_version(req, &id).await;
                false
            }
            DocMsg::Applied { id } => {
                if !self.awaiting.remove(&id) {
                    log!("{}: applied {id}, which was not waiting", self.doc);
                }
                false
            }
            DocMsg::Rejected { id, why } => {
                if !self.awaiting.remove(&id) {
                    log!("{}: rejected {id}, which was not waiting", self.doc);
                }
                // The shell's dry run failed: from the sidecar's side that is a drop.
                self.event(Event::Dropped {
                    doc: self.doc.clone(),
                    kind: DropKind::Update,
                    id,
                    reason: format!("rejected by the application: {why}"),
                });
                false
            }
            DocMsg::Resync { req } => {
                if let Some(s) = self.session.as_mut() {
                    s.resync();
                }
                self.awaiting.clear();
                self.out.send(req, Event::Done { doc: Some(self.doc.clone()) });
                true
            }
            DocMsg::Leave { .. } => unreachable!("handled by the loop"),
        }
    }

    async fn fetch_version(&mut self, req: Option<u64>, id: &str) {
        let Some(v) = self.versions.iter().find(|v| v.id == id).cloned() else {
            self.out.send(
                req,
                Event::error(ErrorCode::NotFound, format!("no version {id} in this document")),
            );
            return;
        };
        let cached = self.file_cache.lock().expect("cache").get(id).cloned();
        let bytes = match cached {
            Some(b) => b,
            None => match self.store.fetch_file(&self.doc, &v).await {
                Ok(b) => b,
                Err(e) => {
                    self.out.send(req, Event::error(code_of(&e), e.to_string()));
                    return;
                }
            },
        };
        if let Err(e) = integrity::verify_version(&v, &bytes) {
            self.out.send(req, Event::error(ErrorCode::Invalid, e.to_string()));
            return;
        }
        self.out.send(
            req,
            Event::VersionFile {
                doc: self.doc.clone(),
                id: id.to_owned(),
                mime: v.body.file.mime.clone(),
                bytes,
            },
        );
    }

    // ----------------------------------------------------------- publishing

    /// Send what is queued, in order. Returns when the outbox wants to be called again.
    async fn flush(&mut self) -> Option<Instant> {
        loop {
            let now = now_ms();
            let saved: Mutex<Option<String>> = Mutex::new(None);
            let step = {
                let (store, doc, out, mime) = (&self.store, &self.doc, &self.out, &self.mime);
                let session = self.session.as_ref();
                let opened = self.opened.as_deref();
                let saved = &saved;
                self.outbox
                    .step(now, |item: Arc<Outgoing>| async move {
                        send_one(store, doc, session, mime, out, opened, &item, saved).await
                    })
                    .await
            };
            match step {
                Step::Idle => return None,
                Step::Sent => {
                    if let Some(id) = saved.into_inner().expect("saved") {
                        self.opened = Some(id);
                    }
                    self.set_online();
                }
                Step::Wait { until_ms } => return Some(after(until_ms, now)),
                Step::BackedOff { until_ms } => {
                    self.set_offline();
                    return Some(after(until_ms, now));
                }
                Step::Refused(e) => {
                    let head = self.outbox.discard_head();
                    log!("{}: refused: {e}", self.doc);
                    self.out.send(
                        head.and_then(|h| h.req()),
                        Event::error(code_of(&e), e.to_string()),
                    );
                }
            }
        }
    }

    // ----------------------------------------------------------- reading

    async fn poll(&mut self) -> Result<(), StoreError> {
        self.role = self.store.role(&self.doc).await?;
        let versions = self.store.versions(&self.doc).await?;
        self.on_versions(versions).await?;
        let updates = self.store.updates(&self.doc).await?;
        self.on_updates(updates).await?;
        self.report_records().await;
        Ok(())
    }

    async fn on_versions(&mut self, versions: Vec<Stamped<Version>>) -> Result<(), StoreError> {
        let mut ids: Vec<String> = versions.iter().map(|v| v.id.clone()).collect();
        ids.sort();
        if self.version_ids.as_ref() == Some(&ids) {
            return Ok(());
        }
        let (cache, store, doc) = (self.file_cache.clone(), self.store.clone(), self.doc.clone());
        let v = view::build(versions.clone(), Options::default(), move |v| {
            let (cache, store, doc) = (cache.clone(), store.clone(), doc.clone());
            async move {
                if let Some(b) = cache.lock().expect("cache").get(&v.id).cloned() {
                    return Ok(b);
                }
                let b = store.fetch_file(&doc, &v).await?;
                cache.lock().expect("cache").insert(v.id.clone(), b.clone());
                Ok(b)
            }
        })
        .await?;
        self.versions = versions;
        self.version_ids = Some(ids);

        for r in &v.rejected {
            if self.reported_versions.insert(r.id.clone()) {
                self.event(Event::Dropped {
                    doc: self.doc.clone(),
                    kind: DropKind::Version,
                    id: r.id.clone(),
                    reason: format!("{:?}", r.reason),
                });
            }
        }
        let head_ids: Vec<String> = v.heads.iter().map(|h| h.version.id.clone()).collect();
        if self.opened.is_none() {
            self.opened = head_ids.first().cloned();
        }
        if self.head_ids.as_ref() != Some(&head_ids) {
            self.head_ids = Some(head_ids);
            let heads = v
                .heads
                .iter()
                .map(|h| HeadInfo {
                    id: h.version.id.clone(),
                    author: Author {
                        principal: h.version.body.created_by.principal.clone(),
                        device: h.version.body.created_by.device.clone(),
                    },
                    ts_ms: h.version.ts_ms,
                    message: h.version.body.message.clone(),
                    covers: h.version.body.covers.as_ref().map(|c| c.through.clone()),
                })
                .collect();
            self.event(Event::Heads {
                doc: self.doc.clone(),
                heads,
                conflict: v.is_conflict(),
            });
        }
        Ok(())
    }

    async fn on_updates(&mut self, updates: Vec<Stamped<Update>>) -> Result<(), StoreError> {
        if self.session.is_none() {
            let Some(g) = find_genesis(&updates, &self.versions) else {
                return Ok(());
            };
            self.genesis_id = Some(g.id.clone());
            self.session = Some(CrdtSession::new(
                ALGORITHM,
                g.body.doc_model.clone(),
                g.body.base_version.clone(),
            ));
        }
        let base = self.session.as_ref().expect("session").base().to_owned();

        // File payloads of updates this session would apply, fetched and checked here
        // because the receiver is synchronous. A transport failure fails the whole
        // read (it is tried again); anything else makes that one update a drop.
        for u in &updates {
            let Payload::File { file } = &u.body.payload else {
                continue;
            };
            if self.resolved.contains_key(&u.id)
                || u.body.algorithm != ALGORITHM
                || !DOC_MODELS.contains(&u.body.doc_model.as_str())
                || u.body.base_version != base
            {
                continue;
            }
            match self.store.fetch_update_file(&self.doc, file).await {
                Ok(bytes) => {
                    let checked = integrity::verify(file, &bytes)
                        .map(|()| bytes)
                        .map_err(|e| e.to_string());
                    self.resolved.insert(u.id.clone(), checked);
                }
                Err(e @ (StoreError::Transport(_) | StoreError::RateLimited { .. })) => {
                    return Err(e)
                }
                Err(e) => {
                    self.resolved.insert(u.id.clone(), Err(e.to_string()));
                }
            }
        }

        let mut target = Collect::default();
        let report = self
            .session
            .as_mut()
            .expect("session")
            .ingest_resolved(&updates, &self.resolved, &mut target);

        let by_id: HashMap<&str, &Stamped<Update>> =
            updates.iter().map(|u| (u.id.as_str(), u)).collect();
        let mut accepted: Vec<(String, Vec<u8>)> =
            report.applied.iter().cloned().zip(target.applied).collect();
        // The genesis first: a joiner cannot apply anything before it has a replica.
        let genesis = self.genesis_id.clone();
        accepted.sort_by_key(|(id, _)| Some(id) != genesis.as_ref());
        for (id, bytes) in accepted {
            let u = by_id[id.as_str()];
            let device = u.device.clone().unwrap_or_default();
            if u.sender == self.me.principal && device == self.me.device {
                continue; // this process wrote it; the replica has it
            }
            let author = Author {
                principal: u.sender.clone(),
                device,
            };
            if genesis.as_deref() == Some(id.as_str()) {
                self.event(Event::Genesis {
                    doc: self.doc.clone(),
                    id: id.clone(),
                    base_version: base.clone(),
                    author,
                    bytes,
                });
            } else {
                self.event(Event::Update {
                    doc: self.doc.clone(),
                    id: id.clone(),
                    author,
                    ts_ms: u.ts_ms,
                    bytes,
                });
            }
            self.awaiting.insert(id);
        }
        for (id, why) in &report.skipped {
            let why = match why {
                Skip::Unsupported => "an algorithm or document model this sidecar does not carry",
                Skip::OtherBase => "based on another version than this session's",
                Skip::DuplicateSeq => "a second update with a seq the stream already used",
            };
            log!("{}: skipped update {id}: {why}", self.doc);
        }
        for (id, d) in &report.dropped {
            let reason = match d {
                Dropped::BadPayload(e) => e.to_string(),
                Dropped::Invalid(s) => s.clone(),
            };
            self.event(Event::Dropped {
                doc: self.doc.clone(),
                kind: DropKind::Update,
                id: id.clone(),
                reason,
            });
        }
        for g in &report.gaps {
            self.event(Event::Gap {
                doc: self.doc.clone(),
                author: Author {
                    principal: g.stream.principal.clone(),
                    device: g.stream.device.clone(),
                },
                expected: g.expected,
                found: g.found,
            });
        }
        Ok(())
    }

    async fn report_records(&mut self) {
        for r in self.store.dropped_records(&self.doc).await {
            let key = (r.stream.clone(), r.id.clone(), r.reason.clone());
            if self.reported_records.insert(key) {
                self.event(Event::Dropped {
                    doc: self.doc.clone(),
                    kind: DropKind::Record,
                    id: r.id,
                    reason: format!("{}: {}", r.stream, r.reason),
                });
            }
        }
    }
}

/// `until_ms` on the wall clock as an `Instant`.
fn after(until_ms: u64, now: u64) -> Instant {
    Instant::now() + Duration::from_millis(until_ms.saturating_sub(now))
}

async fn send_one(
    store: &Arc<dyn DocumentStore>,
    doc: &String,
    session: Option<&CrdtSession>,
    mime: &str,
    out: &Output,
    opened: Option<&str>,
    item: &Outgoing,
    saved: &Mutex<Option<String>>,
) -> Result<(), StoreError> {
    match item {
        Outgoing::Update {
            req,
            seq,
            bytes,
            file,
        } => {
            let session =
                session.ok_or_else(|| StoreError::Invalid("no live session".into()))?;
            // Spec 7 rule 5: over 34,000 bytes the payload is a file.
            let payload = if bytes.len() as u64 <= MAX_INLINE_BYTES {
                inline_payload(bytes).map_err(|e| StoreError::Invalid(e.to_string()))?
            } else {
                let known = file.lock().expect("file").clone();
                let file_ref = match known {
                    Some(f) => f,
                    None => {
                        let f = store
                            .put_update_file(
                                doc,
                                NewFile {
                                    mime: "application/octet-stream".into(),
                                    bytes: bytes.clone(),
                                },
                            )
                            .await?;
                        *file.lock().expect("file") = Some(f.clone());
                        f
                    }
                };
                Payload::File { file: file_ref }
            };
            let stamped = store
                .publish_update(doc, session.update_at(*seq, payload))
                .await?;
            out.send(
                *req,
                Event::Published {
                    doc: doc.clone(),
                    kind: PublishKind::Update,
                    id: stamped.id,
                    seq: Some(*seq),
                    parents: Vec::new(),
                },
            );
            Ok(())
        }
        Outgoing::Version {
            req,
            bytes,
            message,
            resolve,
        } => {
            // Spec 4.2 rule 4: a save is a child of the version the editor started
            // from, however the head moved since. A merge names every head the
            // user merged (spec 6.3).
            let parents = if resolve.is_empty() {
                publish::parents_for_save(opened.ok_or_else(|| {
                    StoreError::Invalid("there is no version to save from yet".into())
                })?)
            } else {
                resolve.clone()
            };
            let mime = mime.to_owned();
            let v = publish::publish_checked(
                store.as_ref(),
                doc,
                parents,
                NewFile {
                    mime,
                    bytes: bytes.clone(),
                },
                message.clone(),
            )
            .await?;
            *saved.lock().expect("saved") = Some(v.id.clone());
            out.send(
                *req,
                Event::Published {
                    doc: doc.clone(),
                    kind: PublishKind::Version,
                    id: v.id,
                    seq: None,
                    parents: v.body.parents,
                },
            );
            Ok(())
        }
    }
}
