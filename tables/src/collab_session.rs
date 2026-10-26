// SPDX-License-Identifier: GPL-3.0-or-later
//! One window's collaboration session (RFC-0005 2.1, 2.3, 2.5).
//!
//! The shell never runs tokio and never links a network stack. `collabd` does
//! (collab/), started with [`gio::Subprocess`]; this talks to it in
//! `suite_collab` lines over its pipes. Commands go out through a
//! [`suite_collab::SessionClient`] (a [`ProcessClient`] over the child's
//! stdin) and events come in with `DataInputStream::read_line_async` on the
//! main loop, so no thread and no channel crosses into GTK, and the
//! [`Replica`] stays on the main thread beside the workbook it mirrors.
//!
//! A session has a few phases. A sharer sends `connect` and `share` (the saved
//! workbook as version zero, its Loro snapshot as the genesis) and is live at
//! once. A joiner sends `connect` and `join`, receives the sharer's genesis,
//! rebuilds a workbook from it and checks it against version zero's file (a
//! join that does not match fails visibly), then replaces the window's
//! workbook with it and applies the updates that arrived meanwhile. From then
//! on every edit, undo and redo of this user is recorded into the replica and
//! published as a delta, and every update of another person is dry-run into
//! the replica (`Replica::import_updates`), acknowledged, and shown by
//! replacing the workbook with the one the document now holds. The undo
//! history is cleared by that (RFC-0005 2.4, the first slice) and the user is
//! told.
//!
//! No borrow of `live` is held across a call into the controller or a
//! widget, and no command that can end the session is run inside a
//! controller call: those are deferred to the main loop.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use adw::prelude::{AdwDialogExt as _, AlertDialogExt as _};
use gio::prelude::*;
use gtk4::prelude::*;
use gtk4::{self as gtk, gio, glib};
use libadwaita as adw;
use suite_collab::{
    decode_message, Author, Binding, Command, ConnState, ConnectConfig, ErrorCode, Event, HeadInfo, Message,
    Person, ProcessClient, PublishKind, Role, SessionClient, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
use tables_core::controller::{Op, Replica, WorkbookController, WorkbookState};

use crate::collab_logic::{
    connection_words, invite_server, ops_to_match, same_cells, short, their_head, DOC_MODEL, XLSX_MIME,
};
use crate::collab_status::{Status, StatusModel};

type Reply = Box<dyn FnOnce(&Rc<Collab>, Event)>;
/// What a step that talks to the sidecar says when it ends.
pub(crate) type Done<T> = Box<dyn FnOnce(&Rc<Collab>, Result<T, String>)>;

/// What the session drives in its window.
pub(crate) struct View {
    pub window: glib::WeakRef<adw::ApplicationWindow>,
    pub controller: Rc<RefCell<WorkbookController>>,
    pub state: Rc<RefCell<WorkbookState>>,
    pub grid: gtk::DrawingArea,
    pub stack: gtk::Stack,
    pub fx_entry: gtk::Entry,
    pub sheet_model: gtk::StringList,
    pub sheet_switcher: gtk::DropDown,
    pub refresh_sel: Rc<dyn Fn()>,
    pub toasts: adw::ToastOverlay,
}

/// The actions whose sensitivity follows the session.
pub(crate) struct Actions {
    pub share: gio::SimpleAction,
    pub join: gio::SimpleAction,
    pub leave: gio::SimpleAction,
    pub resolve: gio::SimpleAction,
    pub copy_code: gio::SimpleAction,
}

/// What the Share dialog collected.
pub(crate) struct ShareForm {
    pub server: String,
    pub ca_file: Option<String>,
    pub people: Vec<String>,
}

/// What the Join dialog collected.
pub(crate) struct JoinForm {
    pub server: Option<String>,
    pub ca_file: Option<String>,
    pub code: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The sharer has asked for the document and has no answer yet.
    Sharing,
    /// The joiner has joined and has not seen the genesis.
    AwaitingGenesis,
    /// The genesis is rebuilt and is being checked against version zero's file.
    Verifying,
    Live,
}

struct Queued {
    id: String,
    author: Author,
    bytes: Vec<u8>,
}

struct Doc {
    id: String,
    title: String,
    invite: String,
    role: Role,
    phase: Phase,
    me: String,
    replica: Option<Replica>,
    /// What the other peers are known to have: the replica's version at the
    /// last publish or import. A delta is exported from it.
    last_sent: Vec<u8>,
    /// The workbook rebuilt from the genesis, until it is checked and shown.
    incoming: Option<WorkbookState>,
    /// Updates that arrived before the workbook was shown.
    queued: Vec<Queued>,
    people: BTreeSet<String>,
    last_update: Option<String>,
    problem: Option<String>,
    heads: Vec<HeadInfo>,
    conflict: bool,
    /// The versions this session published: the ones that are not "theirs".
    my_versions: HashSet<String>,
}

impl Doc {
    fn new(id: String, title: String, invite: String, role: Role, phase: Phase, me: String) -> Doc {
        let mut people = BTreeSet::new();
        people.insert(me.clone());
        Doc {
            id,
            title,
            invite,
            role,
            phase,
            me,
            replica: None,
            last_sent: Vec::new(),
            incoming: None,
            queued: Vec::new(),
            people,
            last_update: None,
            problem: None,
            heads: Vec::new(),
            conflict: false,
            my_versions: HashSet::new(),
        }
    }
}

struct Connected {
    principal: String,
    server: String,
    ca_file: Option<String>,
}

type Client = ProcessClient<std::io::Empty, gio::OutputStreamWrite<gio::OutputStream>>;

struct Live {
    generation: u64,
    process: gio::Subprocess,
    stdin: gio::OutputStream,
    client: Client,
    pending: HashMap<u64, Reply>,
    connected: Option<Connected>,
    conn: ConnState,
    doc: Option<Doc>,
}

pub(crate) struct Collab {
    view: View,
    status: Status,
    banner: adw::Banner,
    actions: Actions,
    live: RefCell<Option<Live>>,
    generation: Cell<u64>,
}

thread_local! {
    /// The session of each workbook window, by the address of its workbook
    /// state, so the save path (which has only the state) can find it.
    static SESSIONS: RefCell<Vec<(usize, Weak<Collab>)>> = const { RefCell::new(Vec::new()) };
}

/// Called by the save path after a save has been committed: publish the file
/// just written as a version, when the workbook is in a live session.
/// A failed publish never fails the save (RFC-0005 2.3 point 4).
pub(crate) fn after_save(state: &Rc<RefCell<WorkbookState>>, path: &Path) {
    let key = Rc::as_ptr(state) as usize;
    let session = SESSIONS.with(|s| s.borrow().iter().find(|(k, _)| *k == key).and_then(|(_, c)| c.upgrade()));
    if let Some(collab) = session {
        collab.publish_saved(path);
    }
}

/// A peer id this document has never seen: line ids are `peer.counter`, so a
/// peer is never reused.
fn new_peer() -> u64 {
    let peer = (u64::from(glib::random_int()) << 32) | u64::from(glib::random_int());
    peer.clamp(1, u64::MAX - 1)
}

/// Where `collabd` is: `COLLABD_BIN` (development and tests), else the
/// libexec directory a package installs it in.
fn sidecar_path() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("COLLABD_BIN").filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!("{}: {}", p.display(), suite_common::i18n("COLLABD_BIN is not a file")))
        };
    }
    for dir in ["/app/libexec", "/usr/libexec", "/usr/local/libexec"] {
        let p = Path::new(dir).join("tuna-collabd");
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(suite_common::i18n("The collaboration helper, tuna-collabd, is not installed."))
}

/// A file in the cache directory holding `bytes`, for something that can
/// only read a path (the workbook reader, a second window).
fn stage(bytes: &[u8], tag: &str) -> Result<PathBuf, String> {
    let dir = glib::user_cache_dir().join("tuna-collab");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{tag}-{:08x}.xlsx", glib::random_int()));
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Read the workbook in `bytes` with the reader every file open uses.
fn read_workbook(bytes: &[u8], tag: &str) -> Result<(tables_core::TablesEngine, Vec<tables_core::SheetModel>), String> {
    let path = stage(bytes, tag)?;
    let loaded = match path.to_str() {
        Some(p) => tables_core::io::load_workbook(p),
        None => Err("the cache path is not UTF-8".to_owned()),
    };
    let _ = std::fs::remove_file(&path);
    loaded
}

fn clock() -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|t| t.format("%H:%M:%S").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

impl View {
    fn toast(&self, text: &str) {
        let toast = adw::Toast::new(text);
        toast.set_timeout(3);
        self.toasts.add_toast(toast);
    }

    /// The calls the undo action makes after a step: the sheet switcher, the
    /// accessible cells, the grid's description and the drawing.
    fn refreshed(&self) {
        crate::sheet_bar::follow(&self.sheet_model, &self.sheet_switcher, &self.state);
        (self.refresh_sel)();
        crate::grid_area::refresh_grid_a11y(&self.grid, &self.state);
        self.grid.queue_draw();
    }

    /// Show the workbook another person's update produced.
    fn replace_remote(&self, incoming: WorkbookState, from: &str) {
        let cleared = self.controller.borrow_mut().replace_from_remote(incoming);
        self.refreshed();
        if cleared {
            self.toast(&suite_common::i18n("Undo history was cleared by a change from %s").replace("%s", from));
        }
    }

    /// Show the workbook a join produced, in place of this window's: it has
    /// no file of its own yet, so saving asks where to put it.
    fn install_joined(&self, incoming: WorkbookState, title: &str) {
        self.controller.borrow_mut().replace_from_remote(incoming);
        *self.controller.borrow().file_path.borrow_mut() = None;
        self.stack.set_visible_child_name("editor");
        self.fx_entry.set_text("");
        if let Some(window) = self.window.upgrade() {
            window.set_title(Some(&format!("{title} {} — Tables", suite_common::i18n("(Shared)"))));
        }
        self.refreshed();
    }
}

impl Collab {
    pub(crate) fn new(view: View, status: Status, banner: adw::Banner, actions: Actions) -> Rc<Collab> {
        let key = Rc::as_ptr(&view.state) as usize;
        let collab = Rc::new(Collab {
            view,
            status,
            banner,
            actions,
            live: RefCell::new(None),
            generation: Cell::new(0),
        });
        SESSIONS.with(|s| {
            let mut s = s.borrow_mut();
            s.retain(|(_, c)| c.upgrade().is_some());
            s.push((key, Rc::downgrade(&collab)));
        });
        if let Some(window) = collab.view.window.upgrade() {
            let this = collab.clone();
            window.connect_destroy(move |_| this.shutdown());
        }
        collab.refresh_ui();
        collab
    }

    pub(crate) fn window(&self) -> Option<adw::ApplicationWindow> {
        self.view.window.upgrade()
    }

    /// Whether the window holds a workbook nobody has typed in (a join takes
    /// it without asking) or one a join would replace.
    pub(crate) fn is_pristine(&self) -> bool {
        let c = self.view.controller.borrow();
        let untouched = c.file_path.borrow().is_none() && !c.is_dirty();
        untouched
    }

    pub(crate) fn invite(&self) -> Option<String> {
        self.live.borrow().as_ref().and_then(|l| l.doc.as_ref()).map(|d| d.invite.clone()).filter(|i| !i.is_empty())
    }

    pub(crate) fn has_session(&self) -> bool {
        self.live.borrow().as_ref().is_some_and(|l| l.doc.is_some())
    }

    pub(crate) fn toast(&self, text: &str) {
        self.view.toast(text);
    }

    pub(crate) fn copy_to_clipboard(&self, text: &str) {
        self.view.grid.clipboard().set_text(text);
    }

    fn alert(&self, heading: &str, body: &str) {
        suite_common::show_error_dialog(self.window().as_ref(), heading, body);
    }

    // ------------------------------------------------------------------ the process

    fn ensure_process(self: &Rc<Self>) -> Result<(), String> {
        if self.live.borrow().is_some() {
            return Ok(());
        }
        let bin = sidecar_path()?;
        let process = gio::Subprocess::newv(
            &[bin.as_os_str()],
            gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE,
        )
        .map_err(|e| format!("{}: {e}", bin.display()))?;
        let (Some(stdin), Some(stdout)) = (process.stdin_pipe(), process.stdout_pipe()) else {
            process.force_exit();
            return Err(suite_common::i18n("The collaboration helper has no pipes."));
        };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let mut client: Client = ProcessClient::new(std::io::empty(), stdin.clone().into_write());
        let hello = client
            .send(&Command::Hello {
                protocol: PROTOCOL_VERSION,
                min_protocol: Some(MIN_PROTOCOL_VERSION),
                client: format!("tables {}", env!("CARGO_PKG_VERSION")),
            })
            .map_err(|e| e.to_string())?;
        let mut pending: HashMap<u64, Reply> = HashMap::new();
        pending.insert(
            hello,
            Box::new(|this, event| match event {
                Event::Hello { bindings, .. } if !bindings.contains(&Binding::Streams) => {
                    this.fail_sidecar(&suite_common::i18n("The collaboration helper cannot speak to Durable Streams."));
                }
                Event::Error { message, .. } => this.fail_sidecar(&message),
                _ => {}
            }),
        );
        *self.live.borrow_mut() = Some(Live {
            generation,
            process,
            stdin,
            client,
            pending,
            connected: None,
            conn: ConnState::Connecting,
            doc: None,
        });
        Self::read_loop(self, generation, gio::DataInputStream::new(&stdout));
        Ok(())
    }

    fn read_loop(this: &Rc<Self>, generation: u64, input: gio::DataInputStream) {
        let weak = Rc::downgrade(this);
        let again = input.clone();
        input.read_line_async(glib::Priority::DEFAULT, None::<&gio::Cancellable>, move |line| {
            let Some(this) = weak.upgrade() else { return };
            match line {
                Ok(Some(line)) => {
                    this.on_line(generation, &line);
                    Self::read_loop(&this, generation, again);
                }
                Ok(None) => this.sidecar_ended(generation, None),
                Err(e) => this.sidecar_ended(generation, Some(e.to_string())),
            }
        });
    }

    fn on_line(self: &Rc<Self>, generation: u64, line: &[u8]) {
        if self.live.borrow().as_ref().map(|l| l.generation) != Some(generation) {
            return;
        }
        let message = std::str::from_utf8(line)
            .map_err(|e| e.to_string())
            .and_then(|text| decode_message(text).map_err(|e| e.to_string()));
        match message {
            Ok(message) => self.on_message(message),
            // A line this shell cannot read is the sidecar's to explain: it is logged, and the next line is read.
            Err(e) => eprintln!("tables: the collaboration helper sent a line that cannot be read: {e}"),
        }
    }

    fn on_message(self: &Rc<Self>, message: Message) {
        let Message { req, event } = message;
        if let Some(req) = req {
            let reply = self.live.borrow_mut().as_mut().and_then(|l| l.pending.remove(&req));
            if let Some(reply) = reply {
                reply(self, event);
                return;
            }
        }
        self.on_event(event);
    }

    /// Write `command`; `reply` runs with the event that answers it. When the
    /// pipe is gone the reply is called at once with a transport error.
    fn send(self: &Rc<Self>, command: Command, reply: Option<Reply>) -> bool {
        let failure = {
            let mut guard = self.live.borrow_mut();
            let Some(live) = guard.as_mut() else {
                drop(guard);
                if let Some(reply) = reply {
                    reply(self, Event::error(ErrorCode::NotConnected, suite_common::i18n("There is no session.")));
                }
                return false;
            };
            match live.client.send(&command) {
                Ok(req) => {
                    if let Some(reply) = reply {
                        live.pending.insert(req, reply);
                    }
                    return true;
                }
                Err(e) => (reply, e.to_string()),
            }
        };
        let (reply, why) = failure;
        if let Some(reply) = reply {
            reply(self, Event::error(ErrorCode::Transport, why.clone()));
        }
        self.fail_sidecar(&why);
        false
    }

    /// The sidecar cannot be written to or said something that ends the
    /// session. Deferred: this may run inside a controller call.
    fn fail_sidecar(self: &Rc<Self>, why: &str) {
        let (this, why) = (self.clone(), why.to_owned());
        glib::idle_add_local_once(move || {
            let had_session = this.live.borrow().is_some();
            this.end_session();
            if had_session {
                this.alert(&suite_common::i18n("Sharing Stopped"), &why);
            }
        });
    }

    fn sidecar_ended(self: &Rc<Self>, generation: u64, why: Option<String>) {
        if self.live.borrow().as_ref().map(|l| l.generation) != Some(generation) {
            return;
        }
        let why = why.unwrap_or_else(|| suite_common::i18n("The collaboration helper stopped."));
        self.fail_sidecar(&why);
    }

    /// Close the pipe, which ends `collabd`; a helper that has not left in
    /// five seconds is killed.
    fn stop_process(&self) {
        let Some(live) = self.live.borrow_mut().take() else { return };
        let _ = live.stdin.close(gio::Cancellable::NONE);
        let process = live.process;
        drop(live.client);
        glib::timeout_add_seconds_local_once(5, move || process.force_exit());
    }

    /// End the session here: the workbook stays as it is, editable and
    /// ordinary, and the sidecar goes.
    fn end_session(self: &Rc<Self>) {
        {
            let mut controller = self.view.controller.borrow_mut();
            controller.observe_applied(None);
            controller.set_read_only(false);
        }
        self.stop_process();
        self.refresh_ui();
    }

    fn shutdown(&self) {
        if let Ok(mut controller) = self.view.controller.try_borrow_mut() {
            controller.observe_applied(None);
        }
        self.stop_process();
    }

    /// A connection nobody has used (the Join dialog showed a key and was
    /// closed) ends with the dialog.
    pub(crate) fn drop_idle_connection(self: &Rc<Self>) {
        if !self.has_session() {
            self.stop_process();
            self.refresh_ui();
        }
    }

    // ------------------------------------------------------------------ connecting

    /// Be connected to `server`, starting the sidecar if need be; `then` gets
    /// this person's key (the principal other people put on a roster).
    pub(crate) fn ensure_connected(self: &Rc<Self>, server: String, ca_file: Option<String>, then: Done<String>) {
        if let Err(e) = self.ensure_process() {
            return then(self, Err(e));
        }
        let already = self.live.borrow().as_ref().and_then(|l| l.connected.as_ref()).and_then(|c| {
            (c.server == server && c.ca_file == ca_file).then(|| c.principal.clone())
        });
        if let Some(principal) = already {
            return then(self, Ok(principal));
        }
        let (s, ca) = (server.clone(), ca_file.clone());
        self.send(
            Command::Connect { binding: Binding::Streams, config: ConnectConfig { server, ca_file } },
            Some(Box::new(move |this, event| match event {
                Event::Connected { principal, .. } => {
                    if let Some(live) = this.live.borrow_mut().as_mut() {
                        live.connected = Some(Connected { principal: principal.clone(), server: s, ca_file: ca });
                    }
                    then(this, Ok(principal));
                }
                Event::Error { message, .. } => then(this, Err(message)),
                other => then(this, Err(format!("unexpected answer: {other:?}"))),
            })),
        );
    }

    // ------------------------------------------------------------------ sharing

    pub(crate) fn share(self: &Rc<Self>, form: ShareForm, done: Done<String>) {
        if self.has_session() {
            return done(self, Err(suite_common::i18n("This workbook is already shared.")));
        }
        let ShareForm { server, ca_file, people } = form;
        self.ensure_connected(
            server,
            ca_file,
            Box::new(move |this, connected| match connected {
                Err(e) => {
                    this.drop_idle_connection();
                    done(this, Err(e));
                }
                Ok(principal) => this.send_share(principal, people, done),
            }),
        );
    }

    fn document_title(&self) -> String {
        let controller = self.view.controller.borrow();
        let path = controller.file_path.borrow();
        path.as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| suite_common::i18n("Untitled Workbook"))
    }

    fn send_share(self: &Rc<Self>, principal: String, people: Vec<String>, done: Done<String>) {
        // The file and the genesis come from one workbook at one moment.
        let made = {
            let state = self.view.state.borrow();
            crate::persistence::autosave_bytes(&state).map(|xlsx| (xlsx, Replica::new(new_peer(), &state)))
        };
        let (xlsx, replica) = match made {
            Ok(m) => m,
            Err(e) => {
                self.drop_idle_connection();
                return done(self, Err(e));
            }
        };
        let genesis = replica.export_snapshot();
        let title = self.document_title();
        let mut doc = Doc::new(String::new(), title.clone(), String::new(), Role::Owner, Phase::Sharing, principal);
        doc.last_sent = replica.version_bytes();
        doc.replica = Some(replica);
        if let Some(live) = self.live.borrow_mut().as_mut() {
            live.doc = Some(doc);
        }
        // From now on edits are recorded; they are published once the document exists.
        self.install_observer();
        self.send(
            Command::Share {
                title,
                mime: XLSX_MIME.to_owned(),
                first_version: xlsx,
                genesis: Some(genesis),
                doc_model: DOC_MODEL.to_owned(),
                people: people.into_iter().map(|principal| Person { principal, role: Role::Editor }).collect(),
            },
            Some(Box::new(move |this, event| match event {
                Event::Shared { doc, invite, .. } => {
                    this.with_doc(|d| {
                        d.id = doc;
                        d.invite = invite.clone();
                        d.phase = Phase::Live;
                    });
                    // What was edited while the document was being made.
                    this.flush_updates();
                    this.refresh_ui();
                    done(this, Ok(invite));
                }
                Event::Error { message, .. } => {
                    this.end_session();
                    done(this, Err(message));
                }
                other => {
                    this.end_session();
                    done(this, Err(format!("unexpected answer: {other:?}")));
                }
            })),
        );
        self.refresh_ui();
    }

    // ------------------------------------------------------------------ joining

    pub(crate) fn join(self: &Rc<Self>, form: JoinForm, done: Done<String>) {
        if self.has_session() {
            return done(self, Err(suite_common::i18n("This workbook is already shared.")));
        }
        let code = form.code.trim().to_owned();
        let Some(server) = form.server.filter(|s| !s.trim().is_empty()).or_else(|| invite_server(&code)) else {
            return done(self, Err(suite_common::i18n("That is not a join code.")));
        };
        self.ensure_connected(
            server,
            form.ca_file,
            Box::new(move |this, connected| match connected {
                Err(e) => {
                    this.drop_idle_connection();
                    done(this, Err(e));
                }
                Ok(principal) => {
                    let invite = code.clone();
                    this.send(
                        Command::Join { invite: code },
                        Some(Box::new(move |this, event| match event {
                            Event::Joined { doc, title, role, .. } => {
                                if let Some(live) = this.live.borrow_mut().as_mut() {
                                    live.doc = Some(Doc::new(doc, title.clone(), invite, role, Phase::AwaitingGenesis, principal));
                                }
                                this.refresh_ui();
                                done(this, Ok(title));
                            }
                            Event::Error { message, .. } => {
                                this.drop_idle_connection();
                                done(this, Err(message));
                            }
                            other => {
                                this.drop_idle_connection();
                                done(this, Err(format!("unexpected answer: {other:?}")));
                            }
                        })),
                    );
                }
            }),
        );
    }

    /// The join cannot go on: say why, and leave.
    fn abort_join(self: &Rc<Self>, why: &str) {
        let doc = self.live.borrow().as_ref().and_then(|l| l.doc.as_ref()).map(|d| d.id.clone());
        if let Some(doc) = doc {
            self.send(Command::Leave { doc }, None);
        }
        self.end_session();
        self.alert(&suite_common::i18n("Could Not Join"), why);
    }

    fn on_genesis(self: &Rc<Self>, doc_id: &str, id: String, base_version: String, author: Author, bytes: Vec<u8>) {
        let outcome = self.with_doc(|doc| {
            if doc.id != doc_id {
                return None;
            }
            doc.people.insert(author.principal.clone());
            if doc.replica.is_some() {
                return Some(Ok(false));
            }
            Some(match Replica::from_snapshot(new_peer(), &bytes) {
                Ok((replica, state)) => {
                    doc.last_sent = replica.version_bytes();
                    doc.replica = Some(replica);
                    doc.incoming = Some(state);
                    doc.phase = Phase::Verifying;
                    Ok(true)
                }
                Err(e) => Err(e.to_string()),
            })
        });
        match outcome.flatten() {
            None => {}
            Some(Ok(first)) => {
                self.send(Command::Applied { doc: doc_id.to_owned(), id }, None);
                if first {
                    // Version zero's file is the record the genesis is checked against.
                    self.send(
                        Command::FetchVersion { doc: doc_id.to_owned(), id: base_version },
                        Some(Box::new(|this, event| match event {
                            Event::VersionFile { bytes, .. } => this.verify_and_show(&bytes),
                            Event::Error { message, .. } => this.abort_join(&message),
                            _ => {}
                        })),
                    );
                }
            }
            Some(Err(e)) => {
                self.send(Command::Rejected { doc: doc_id.to_owned(), id, why: e.clone() }, None);
                self.abort_join(&format!("{}: {e}", suite_common::i18n("The shared workbook cannot be read")));
            }
        }
    }

    /// The first rebuild is checked against the file it is based on: the
    /// cells and formulas must be equal, or the join fails visibly.
    fn verify_and_show(self: &Rc<Self>, file: &[u8]) {
        let checked = read_workbook(file, "version-zero").and_then(|(engine, sheets)| {
            let guard = self.live.borrow();
            let incoming = guard.as_ref().and_then(|l| l.doc.as_ref()).and_then(|d| d.incoming.as_ref());
            match incoming {
                Some(state) => same_cells(state, &engine, &sheets),
                None => Err(suite_common::i18n("The session ended.")),
            }
        });
        if let Err(e) = checked {
            return self.abort_join(&format!("{}: {e}", suite_common::i18n("The shared workbook does not match its file")));
        }
        let shown = self.with_doc(|doc| {
            doc.phase = Phase::Live;
            (doc.incoming.take(), doc.title.clone(), doc.role, std::mem::take(&mut doc.queued))
        });
        let Some((Some(state), title, role, queued)) = shown else { return };
        self.view.install_joined(state, &title);
        // A viewer reads: the controller refuses every edit, undo and redo.
        self.view.controller.borrow_mut().set_read_only(role < Role::Editor);
        self.install_observer();
        for update in queued {
            self.apply_update(update.id, update.author, update.bytes);
        }
        self.refresh_ui();
        self.toast(&suite_common::i18n("Joined %s").replace("%s", &title));
    }

    // ------------------------------------------------------------------ the live document

    fn with_doc<R>(&self, f: impl FnOnce(&mut Doc) -> R) -> Option<R> {
        self.live.borrow_mut().as_mut().and_then(|l| l.doc.as_mut()).map(f)
    }

    fn install_observer(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.view.controller.borrow_mut().observe_applied(Some(Box::new(
            move |state: &WorkbookState, ops: &[Op]| {
                if let Some(this) = weak.upgrade() {
                    this.on_local_change(state, ops);
                }
            },
        )));
    }

    /// This user's edit, undo or redo: into the replica, then out as a delta.
    fn on_local_change(self: &Rc<Self>, state: &WorkbookState, ops: &[Op]) {
        self.with_doc(|doc| {
            if let Some(replica) = doc.replica.as_mut() {
                replica.record(state, ops);
            }
        });
        self.flush_updates();
    }

    /// Publish what the replica has that the others do not.
    fn flush_updates(self: &Rc<Self>) {
        let sent = {
            let mut guard = self.live.borrow_mut();
            let Some(Live { client, doc: Some(doc), .. }) = guard.as_mut() else { return };
            if doc.phase != Phase::Live || doc.role < Role::Editor {
                return;
            }
            let Some(replica) = doc.replica.as_ref() else { return };
            let now = replica.version_bytes();
            if now == doc.last_sent {
                return;
            }
            match replica.export_since(&doc.last_sent) {
                Ok(delta) => {
                    doc.last_sent = now;
                    client.send(&Command::PublishUpdate { doc: doc.id.clone(), bytes: delta }).map(|_| ()).map_err(|e| e.to_string())
                }
                Err(e) => Err(e.to_string()),
            }
        };
        if let Err(why) = sent {
            self.fail_sidecar(&why);
        }
    }

    /// One update of another person: a dry run into the replica, an answer
    /// for the sidecar, and the workbook it makes shown.
    fn apply_update(self: &Rc<Self>, id: String, author: Author, bytes: Vec<u8>) {
        enum Outcome {
            Queued,
            Applied(Option<WorkbookState>),
            Rejected(String),
        }
        let at = clock();
        let seen = self.with_doc(|doc| {
            if doc.phase != Phase::Live {
                doc.queued.push(Queued { id: id.clone(), author: author.clone(), bytes: bytes.clone() });
                return (doc.id.clone(), Outcome::Queued);
            }
            let Some(replica) = doc.replica.as_mut() else {
                return (doc.id.clone(), Outcome::Rejected("there is no replica".into()));
            };
            let outcome = match replica.import_updates(&bytes) {
                Ok(report) => {
                    let state = if report.changed() { replica.rebuild().map(Some) } else { Ok(None) };
                    match state {
                        Ok(state) => {
                            doc.last_sent = replica.version_bytes();
                            doc.people.insert(author.principal.clone());
                            doc.last_update = Some(format!(
                                "{} {} {}",
                                suite_common::i18n("Last update from"),
                                short(&author.principal),
                                at
                            ));
                            Outcome::Applied(state)
                        }
                        Err(e) => Outcome::Rejected(e),
                    }
                }
                Err(e) => Outcome::Rejected(e.to_string()),
            };
            (doc.id.clone(), outcome)
        });
        let Some((doc_id, outcome)) = seen else { return };
        match outcome {
            Outcome::Queued => {}
            Outcome::Applied(state) => {
                self.send(Command::Applied { doc: doc_id, id }, None);
                if let Some(state) = state {
                    self.view.replace_remote(state, &short(&author.principal));
                }
            }
            Outcome::Rejected(why) => {
                self.send(Command::Rejected { doc: doc_id, id, why: why.clone() }, None);
                self.with_doc(|doc| doc.problem = Some(format!("{} {why}", suite_common::i18n("A change was refused:"))));
            }
        }
        self.refresh_ui();
    }

    fn on_event(self: &Rc<Self>, event: Event) {
        match event {
            Event::State { state, .. } => {
                if let Some(live) = self.live.borrow_mut().as_mut() {
                    live.conn = state;
                }
                self.refresh_ui();
            }
            Event::Genesis { doc, id, base_version, author, bytes } => self.on_genesis(&doc, id, base_version, author, bytes),
            Event::Update { doc, id, author, bytes, .. } => {
                if self.with_doc(|d| d.id == doc) == Some(true) {
                    self.apply_update(id, author, bytes);
                }
            }
            Event::Heads { doc, heads, conflict } => {
                self.with_doc(|d| {
                    if d.id == doc {
                        d.people.extend(heads.iter().map(|h| h.author.principal.clone()));
                        d.heads = heads;
                        d.conflict = conflict;
                    }
                });
                self.refresh_ui();
            }
            Event::Dropped { reason, .. } => {
                self.with_doc(|d| d.problem = Some(format!("{} {reason}", suite_common::i18n("A change was refused:"))));
                self.refresh_ui();
            }
            Event::Gap { author, expected, found, .. } => {
                let why = suite_common::i18n("Updates from %1 are missing: expected number %2, found %3.")
                    .replace("%1", &short(&author.principal))
                    .replace("%2", &expected.to_string())
                    .replace("%3", &found.to_string());
                self.with_doc(|d| d.problem = Some(why));
                self.refresh_ui();
            }
            Event::Error { message, .. } => {
                self.toast(&message);
            }
            // Answers nobody waits for: a published update, a left document.
            Event::Published { .. }
            | Event::Done { .. }
            | Event::Hello { .. }
            | Event::Connected { .. }
            | Event::Shared { .. }
            | Event::Joined { .. }
            | Event::VersionFile { .. } => {}
        }
    }

    // ------------------------------------------------------------------ saving and resolving

    /// A save has been committed to `path`: the file is the record, and a
    /// copy of it is published as a version.
    pub(crate) fn publish_saved(self: &Rc<Self>, path: &Path) {
        let doc = self.with_doc(|d| (d.phase == Phase::Live && d.role >= Role::Editor).then(|| d.id.clone())).flatten();
        let Some(doc) = doc else { return };
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => return self.toast(&format!("{} {e}", suite_common::i18n("Saved, but not shared yet:"))),
        };
        self.publish_version(doc, bytes, None, Vec::new(), suite_common::i18n("Saved and shared"));
    }

    fn publish_version(self: &Rc<Self>, doc: String, bytes: Vec<u8>, message: Option<String>, resolve: Vec<String>, success: String) {
        self.send(
            Command::PublishVersion { doc, bytes, message, resolve },
            Some(Box::new(move |this, event| match event {
                Event::Published { kind: PublishKind::Version, id, .. } => {
                    this.with_doc(|d| d.my_versions.insert(id));
                    this.toast(&success);
                }
                Event::Error { message, .. } => {
                    this.toast(&format!("{} {message}", suite_common::i18n("Saved, but not shared yet:")));
                }
                _ => {}
            })),
        );
    }

    /// The heads, who has them and when, for the Resolve dialog.
    pub(crate) fn heads_text(&self) -> Option<String> {
        let guard = self.live.borrow();
        let doc = guard.as_ref()?.doc.as_ref()?;
        let mut lines = Vec::new();
        for head in &doc.heads {
            let mine = doc.my_versions.contains(&head.id);
            let who = if mine { suite_common::i18n("You") } else { short(&head.author.principal) };
            let when = glib::DateTime::from_unix_local((head.ts_ms / 1000) as i64)
                .ok()
                .and_then(|t| t.format("%H:%M").ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            lines.push(format!("• {who}, {when}"));
        }
        Some(lines.join("\n"))
    }

    pub(crate) fn has_other_head(&self) -> bool {
        self.with_doc(|d| their_head(&d.heads, &d.my_versions).is_some()).unwrap_or(false)
    }

    /// Keep this workbook: publish it as the merge of every head.
    pub(crate) fn keep_mine(self: &Rc<Self>) {
        self.publish_merge(suite_common::i18n("Kept this version"));
    }

    fn publish_merge(self: &Rc<Self>, message: String) {
        let Some((doc, heads)) = self.with_doc(|d| (d.id.clone(), d.heads.iter().map(|h| h.id.clone()).collect::<Vec<_>>())) else {
            return;
        };
        let bytes = crate::persistence::autosave_bytes(&self.view.state.borrow());
        match bytes {
            Ok(bytes) => self.publish_version(doc, bytes, Some(message), heads, suite_common::i18n("The versions are merged")),
            Err(e) => self.alert(&suite_common::i18n("Could Not Merge"), &e),
        }
    }

    /// Fetch the file of the other version and hand it to `then`.
    fn fetch_theirs(self: &Rc<Self>, then: impl FnOnce(&Rc<Collab>, String, Vec<u8>) + 'static) {
        let Some((doc, theirs)) = self
            .with_doc(|d| their_head(&d.heads, &d.my_versions).map(|h| (d.id.clone(), h.id.clone())))
            .flatten()
        else {
            return;
        };
        let id = theirs.clone();
        self.send(
            Command::FetchVersion { doc, id },
            Some(Box::new(move |this, event| match event {
                Event::VersionFile { bytes, .. } => then(this, theirs, bytes),
                Event::Error { message, .. } => this.alert(&suite_common::i18n("Could Not Read the Other Version"), &message),
                _ => {}
            })),
        );
    }

    /// Take the other version's contents in as one undoable edit, which the
    /// peers receive as an ordinary update, then publish the merge.
    pub(crate) fn keep_theirs(self: &Rc<Self>) {
        self.fetch_theirs(|this, _, bytes| {
            let ops = read_workbook(&bytes, "theirs")
                .and_then(|(engine, sheets)| ops_to_match(&this.view.state.borrow(), &engine, &sheets));
            match ops {
                Err(e) => {
                    let hint = suite_common::i18n("Open both versions to compare them.");
                    this.alert(&suite_common::i18n("Could Not Keep Their Version"), &format!("{e}\n\n{hint}"));
                }
                Ok(ops) => {
                    let applied = ops.is_empty()
                        || this.view.controller.borrow_mut().apply_ops(suite_common::i18n("Keep Their Version"), ops);
                    if !applied {
                        let why = suite_common::i18n("This workbook refused the change.");
                        return this.alert(&suite_common::i18n("Could Not Keep Their Version"), &why);
                    }
                    this.view.refreshed();
                    this.publish_merge(suite_common::i18n("Kept the other version"));
                }
            }
        });
    }

    /// Open the other version in a window of its own; nothing is merged or published.
    pub(crate) fn open_both(self: &Rc<Self>) {
        self.fetch_theirs(|this, id, bytes| {
            let tag = format!("version-{}", id.chars().take(8).collect::<String>());
            match stage(&bytes, &tag) {
                Ok(path) => {
                    if let Some(app) = gio::Application::default() {
                        app.open(&[gio::File::for_path(path)], "");
                    }
                }
                Err(e) => this.alert(&suite_common::i18n("Could Not Open the Other Version"), &e),
            }
        });
    }

    /// Stop sharing: the workbook stays in this window, no longer shared.
    pub(crate) fn leave(self: &Rc<Self>) {
        if let Some(doc) = self.with_doc(|d| d.id.clone()).filter(|d| !d.is_empty()) {
            self.send(Command::Leave { doc }, None);
        }
        self.end_session();
        self.toast(&suite_common::i18n("Stopped sharing"));
    }

    // ------------------------------------------------------------------ what the window shows

    fn refresh_ui(&self) {
        let (model, conflict) = {
            let guard = self.live.borrow();
            match guard.as_ref().and_then(|l| l.doc.as_ref().map(|d| (l, d))) {
                None => (None, false),
                Some((live, doc)) => {
                    let (icon, summary) = connection_words(&live.conn, doc.people.len());
                    let people = doc
                        .people
                        .iter()
                        .map(|p| if *p == doc.me { format!("{} ({})", short(p), suite_common::i18n("you")) } else { short(p) })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let problem = doc.problem.clone().or_else(|| match &live.conn {
                        ConnState::Error { message } => Some(message.clone()),
                        _ => None,
                    });
                    (Some(StatusModel { icon, summary, people, last_update: doc.last_update.clone(), problem }), doc.conflict)
                }
            }
        };
        let sharing = model.is_some();
        self.status.update(model.as_ref());
        crate::collab_banner::show(&self.banner, conflict);
        self.actions.share.set_enabled(!sharing);
        self.actions.join.set_enabled(!sharing);
        self.actions.leave.set_enabled(sharing);
        self.actions.copy_code.set_enabled(sharing);
        self.actions.resolve.set_enabled(conflict);
    }
}
