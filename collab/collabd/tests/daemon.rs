//! The daemon's command loop against `dcp_core`'s in-memory store: no server, no
//! network. What needs a real Durable Streams server (TLS, signatures, the long
//! poll, files on the wire) is `collab/headless`.
//!
//! Two daemons share one `MemoryBackend`, so they are two principals of one
//! store; each is driven by sending `Input`s and reading the `Message`s it writes,
//! exactly the two ends of the pipe `main` wires to stdin and stdout.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use collabd::connector::{ConnectError, Connector};
use collabd::daemon::{Daemon, Input};
use collabd::output::Output;
use dcp_core::memory::{ManualClock, MemoryBackend};
use dcp_core::DocumentStore;
use suite_collab::{
    Binding, Command, ConnectConfig, ErrorCode, Event, Line, Message, Person, PublishKind, Request,
    Role,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

const MIME: &str = "text/plain";

struct MemoryConnector {
    backend: MemoryBackend,
    principal: String,
}

#[async_trait]
impl Connector for MemoryConnector {
    fn bindings(&self) -> Vec<Binding> {
        vec![Binding::Streams]
    }
    async fn connect(
        &self,
        _: Binding,
        _: &ConnectConfig,
        device: &str,
    ) -> Result<Arc<dyn DocumentStore>, ConnectError> {
        Ok(Arc::new(self.backend.store(&self.principal, device)))
    }
}

struct Peer {
    tx: UnboundedSender<Input>,
    rx: UnboundedReceiver<Message>,
    next_req: u64,
    backlog: Vec<Message>,
}

impl Peer {
    fn start(backend: &MemoryBackend, principal: &str) -> Peer {
        let (out, rx) = Output::channel();
        let connector = MemoryConnector {
            backend: backend.clone(),
            principal: principal.to_owned(),
        };
        let daemon = Daemon::new(out, Arc::new(connector), format!("dev-{principal}"));
        let (tx, input) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(daemon.run(input));
        Peer {
            tx,
            rx,
            next_req: 1,
            backlog: Vec::new(),
        }
    }

    fn send(&mut self, command: Command) -> u64 {
        let req = self.next_req;
        self.next_req += 1;
        self.tx
            .send(Input::Request(Request {
                req: Some(req),
                command,
            }))
            .unwrap();
        req
    }

    /// The next event matching `pick`, in order; earlier events that do not match
    /// stay available to later calls.
    async fn wait<T>(&mut self, what: &str, mut pick: impl FnMut(&Message) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(i) = self.backlog.iter().position(|m| pick(m).is_some()) {
                let m = self.backlog.remove(i);
                return pick(&m).unwrap();
            }
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some(m)) => self.backlog.push(m),
                Ok(None) => panic!("the daemon ended while waiting for {what}"),
                Err(_) => panic!(
                    "timed out waiting for {what}; saw {:#?}",
                    self.backlog.iter().map(|m| &m.event).collect::<Vec<_>>()
                ),
            }
        }
    }

    async fn reply(&mut self, req: u64) -> Event {
        self.wait(&format!("the answer to request {req}"), |m| {
            (m.req == Some(req)).then(|| m.event.clone())
        })
        .await
    }

    async fn hello(&mut self) {
        let req = self.send(Command::Hello {
            protocol: 1,
            min_protocol: None,
            client: "test".into(),
        });
        assert!(matches!(self.reply(req).await, Event::Hello { protocol: 1, .. }));
    }

    async fn connect(&mut self) -> String {
        let req = self.send(Command::Connect {
            binding: Binding::Streams,
            config: ConnectConfig {
                server: "http://127.0.0.1:1/".into(),
                ca_file: None,
            },
        });
        match self.reply(req).await {
            Event::Connected { principal, .. } => principal,
            e => panic!("{e:?}"),
        }
    }

    async fn no_event_for(&mut self, what: &str, mut bad: impl FnMut(&Message) -> bool) {
        let until = tokio::time::Instant::now() + Duration::from_millis(1500);
        while let Ok(Some(m)) = tokio::time::timeout_at(until, self.rx.recv()).await {
            assert!(!bad(&m), "{what}: {m:?}");
            self.backlog.push(m);
        }
        assert!(!self.backlog.iter().any(&mut bad), "{what}");
    }
}

fn backend() -> MemoryBackend {
    MemoryBackend::new(Arc::new(ManualClock::new(1_000)))
}

async fn share(owner: &mut Peer, people: Vec<Person>, genesis: &[u8]) -> (String, String, String) {
    let req = owner.send(Command::Share {
        title: "Budget".into(),
        mime: MIME.into(),
        first_version: b"version zero".to_vec(),
        genesis: Some(genesis.to_vec()),
        doc_model: "tables/1".into(),
        people,
    });
    match owner.reply(req).await {
        Event::Shared {
            doc,
            invite,
            base_version,
            ..
        } => (doc, invite, base_version),
        e => panic!("{e:?}"),
    }
}

async fn join(who: &mut Peer, invite: &str) -> Event {
    let req = who.send(Command::Join {
        invite: invite.into(),
    });
    who.reply(req).await
}

#[tokio::test]
async fn nothing_works_before_hello_and_a_hello_that_shares_no_version_is_refused() {
    let b = backend();
    let mut p = Peer::start(&b, "@a:mem");
    let req = p.send(Command::Connect {
        binding: Binding::Streams,
        config: ConnectConfig {
            server: "http://127.0.0.1:1/".into(),
            ca_file: None,
        },
    });
    assert!(matches!(
        p.reply(req).await,
        Event::Error { code: ErrorCode::NotConnected, .. }
    ));
    for (protocol, min_protocol) in [(0, None), (5, Some(2))] {
        let req = p.send(Command::Hello {
            protocol,
            min_protocol,
            client: "t".into(),
        });
        assert!(
            matches!(
                p.reply(req).await,
                Event::Error { code: ErrorCode::UnsupportedProtocol, .. }
            ),
            "{protocol} {min_protocol:?}"
        );
    }
    // A newer client that still speaks 1 gets 1.
    let req = p.send(Command::Hello {
        protocol: 7,
        min_protocol: Some(1),
        client: "t".into(),
    });
    match p.reply(req).await {
        Event::Hello { protocol, bindings, .. } => {
            assert_eq!(protocol, 1);
            assert_eq!(bindings, vec![Binding::Streams]);
        }
        e => panic!("{e:?}"),
    }
    p.connect().await;
}

#[tokio::test]
async fn a_binding_this_build_does_not_have_or_that_has_no_server_is_an_error() {
    let mut p = Peer::start(&backend(), "@a:mem");
    p.hello().await;
    let req = p.send(Command::Connect {
        binding: Binding::Matrix,
        config: ConnectConfig {
            server: "https://matrix.example".into(),
            ca_file: None,
        },
    });
    assert!(matches!(
        p.reply(req).await,
        Event::Error { code: ErrorCode::UnsupportedBinding, .. }
    ));
    let req = p.send(Command::Connect {
        binding: Binding::Streams,
        config: ConnectConfig {
            server: "  ".into(),
            ca_file: None,
        },
    });
    assert!(matches!(
        p.reply(req).await,
        Event::Error { code: ErrorCode::UnconfiguredBinding, .. }
    ));
    // Commands for a document, or a share, before a connect, are told to connect.
    let req = p.send(Command::Join {
        invite: "streams:http://127.0.0.1:1/#d".into(),
    });
    assert!(matches!(
        p.reply(req).await,
        Event::Error { code: ErrorCode::NotConnected, .. }
    ));
    let req = p.send(Command::PublishUpdate {
        doc: "nope".into(),
        bytes: vec![1],
    });
    assert!(matches!(
        p.reply(req).await,
        Event::Error { code: ErrorCode::UnknownDocument, .. }
    ));
}

#[test]
fn a_bad_line_becomes_an_error_input_and_never_ends_the_stream() {
    for (line, want) in [
        (Line::Text("{".into()), ErrorCode::MalformedLine),
        (Line::Text(r#"{"cmd":"selfdestruct"}"#.into()), ErrorCode::UnknownCommand),
        (Line::Text(r#"{"cmd":"join"}"#.into()), ErrorCode::MalformedLine),
        (Line::TooLong { limit: 10 }, ErrorCode::LineTooLong),
        (Line::NotUtf8, ErrorCode::MalformedLine),
    ] {
        match Input::from_line(line.clone()) {
            Some(Input::Bad { code, .. }) => assert_eq!(code, want, "{line:?}"),
            _ => panic!("{line:?} is not a bad line"),
        }
    }
    assert!(Input::from_line(Line::Eof).is_none());
    assert!(matches!(
        Input::from_line(Line::Text(r#"{"req":4,"cmd":"leave","doc":"d"}"#.into())),
        Some(Input::Request(Request { req: Some(4), command: Command::Leave { .. } }))
    ));
}

#[tokio::test]
async fn share_join_publish_and_the_head_moving_does_not_drop_an_update() {
    let b = backend();
    let (mut alice, mut bob) = (Peer::start(&b, "@alice:mem"), Peer::start(&b, "@bob:mem"));
    alice.hello().await;
    bob.hello().await;
    alice.connect().await;
    let bob_principal = bob.connect().await;
    assert_eq!(bob_principal, "@bob:mem");

    let (doc, invite, base) = share(
        &mut alice,
        vec![Person { principal: bob_principal, role: Role::Editor }],
        b"GENESIS-SNAPSHOT",
    )
    .await;
    assert!(invite.starts_with("streams:http://127.0.0.1:1/#"), "{invite}");

    // Bob joins: the genesis arrives first and carries the base.
    let joined = join(&mut bob, &invite).await;
    match joined {
        Event::Joined { role, base_version, title, .. } => {
            assert_eq!((role, title.as_str()), (Role::Editor, "Budget"));
            assert_eq!(base_version.as_deref(), Some(base.as_str()));
        }
        e => panic!("{e:?}"),
    }
    let (gid, gbase, gbytes) = bob
        .wait("the genesis", |m| match &m.event {
            Event::Genesis { id, base_version, bytes, .. } => {
                Some((id.clone(), base_version.clone(), bytes.clone()))
            }
            _ => None,
        })
        .await;
    assert_eq!((gbase.as_str(), gbytes.as_slice()), (base.as_str(), &b"GENESIS-SNAPSHOT"[..]));
    bob.send(Command::Applied { doc: doc.clone(), id: gid });
    let heads = bob
        .wait("heads", |m| match &m.event {
            Event::Heads { heads, conflict, .. } => Some((heads.len(), *conflict)),
            _ => None,
        })
        .await;
    assert_eq!(heads, (1, false));

    // Alice's own genesis never comes back to her as an update.
    alice
        .no_event_for("alice got her own genesis", |m| {
            matches!(m.event, Event::Genesis { .. } | Event::Update { .. })
        })
        .await;

    // Alice edits; Bob receives it, applies it and acknowledges.
    let req = alice.send(Command::PublishUpdate { doc: doc.clone(), bytes: b"edit-1".to_vec() });
    match alice.reply(req).await {
        Event::Published { kind: PublishKind::Update, seq: Some(seq), .. } => assert_eq!(seq, 1),
        e => panic!("{e:?}"),
    }
    let (uid, ubytes) = bob
        .wait("alice's update", |m| match &m.event {
            Event::Update { id, bytes, author, .. } => {
                assert!(author.device.contains("alice"));
                Some((id.clone(), bytes.clone()))
            }
            _ => None,
        })
        .await;
    assert_eq!(ubytes, b"edit-1");
    bob.send(Command::Applied { doc: doc.clone(), id: uid });

    // Bob saves a version: the head moves. Then Alice writes, never having seen
    // it. Bob must still get her update (loro/1 profile section 3).
    let req = bob.send(Command::PublishVersion {
        doc: doc.clone(),
        bytes: b"version one by bob".to_vec(),
        message: Some("bob saved".into()),
        resolve: vec![],
    });
    let Event::Published { kind: PublishKind::Version, parents, id: v1, .. } = bob.reply(req).await else {
        panic!("not published")
    };
    assert_eq!(parents, vec![base.clone()], "a save is a child of the version Bob started from");
    let req = alice.send(Command::PublishUpdate { doc: doc.clone(), bytes: b"edit-2".to_vec() });
    let Event::Published { .. } = alice.reply(req).await else { panic!("not published") };
    let uid2 = bob
        .wait("alice's second update after the head moved", |m| match &m.event {
            Event::Update { id, bytes, .. } if bytes == b"edit-2" => Some(id.clone()),
            _ => None,
        })
        .await;
    bob.send(Command::Applied { doc: doc.clone(), id: uid2 });

    // Alice sees the new head, and both heads are not a conflict: v1 is a child of v0.
    let conflict = alice
        .wait("alice sees bob's version", |m| match &m.event {
            Event::Heads { heads, conflict, .. } if heads.iter().any(|h| h.id == v1) => Some(*conflict),
            _ => None,
        })
        .await;
    assert!(!conflict);

    // Alice's next save is a child of v0 (what she started from), so there are two heads.
    let req = alice.send(Command::PublishVersion {
        doc: doc.clone(),
        bytes: b"version one by alice".to_vec(),
        message: None,
        resolve: vec![],
    });
    let Event::Published { id: a1, parents, .. } = alice.reply(req).await else { panic!("not published") };
    assert_eq!(parents, vec![base.clone()]);
    let heads = alice
        .wait("a conflict", |m| match &m.event {
            Event::Heads { heads, conflict: true, .. } => Some(heads.iter().map(|h| h.id.clone()).collect::<Vec<_>>()),
            _ => None,
        })
        .await;
    assert_eq!(heads.len(), 2);

    // Merging: a version whose parents are both heads is one head again.
    let req = alice.send(Command::PublishVersion {
        doc: doc.clone(),
        bytes: b"merged".to_vec(),
        message: Some("merge".into()),
        resolve: vec![v1.clone(), a1.clone()],
    });
    let Event::Published { parents, id: merged, .. } = alice.reply(req).await else { panic!("not published") };
    assert_eq!(parents, vec![v1, a1]);
    let one = alice
        .wait("a single head", |m| match &m.event {
            Event::Heads { heads, conflict: false, .. } if heads.len() == 1 => Some(heads[0].id.clone()),
            _ => None,
        })
        .await;
    assert_eq!(one, merged);

    // The file of a version comes back verified.
    let req = bob.send(Command::FetchVersion { doc: doc.clone(), id: merged.clone() });
    match bob.reply(req).await {
        Event::Error { code: ErrorCode::NotFound, .. } => {
            // Bob's view may not have caught up yet; ask again once it has.
            bob.wait("the merged head", |m| match &m.event {
                Event::Heads { heads, .. } if heads.iter().any(|h| h.id == merged) => Some(()),
                _ => None,
            })
            .await;
            let req = bob.send(Command::FetchVersion { doc: doc.clone(), id: merged.clone() });
            assert!(matches!(bob.reply(req).await, Event::VersionFile { bytes, .. } if bytes == b"merged"));
        }
        Event::VersionFile { bytes, .. } => assert_eq!(bytes, b"merged"),
        e => panic!("{e:?}"),
    }
}

#[tokio::test]
async fn an_update_over_34000_bytes_travels_as_a_file_and_arrives_whole() {
    let b = backend();
    let (mut alice, mut bob) = (Peer::start(&b, "@alice:mem"), Peer::start(&b, "@bob:mem"));
    for p in [&mut alice, &mut bob] {
        p.hello().await;
        p.connect().await;
    }
    let (doc, invite, _) = share(
        &mut alice,
        vec![Person { principal: "@bob:mem".into(), role: Role::Editor }],
        b"g",
    )
    .await;
    join(&mut bob, &invite).await;
    let big: Vec<u8> = (0..120_000u32).map(|i| (i % 251) as u8).collect();
    let req = alice.send(Command::PublishUpdate { doc: doc.clone(), bytes: big.clone() });
    alice.reply(req).await;
    let got = bob
        .wait("the big update", |m| match &m.event {
            Event::Update { bytes, .. } => Some(bytes.clone()),
            _ => None,
        })
        .await;
    assert_eq!(got, big);
    // The store holds it as a file payload: the one update in the log is `encoding: file`.
    let log = b.store("@alice:mem", "probe").updates(&doc).await.unwrap();
    assert!(
        log.iter().any(|u| matches!(u.body.payload, dcp_core::Payload::File { .. }) && u.body.seq == 1),
        "{log:?}"
    );
}

#[tokio::test]
async fn a_viewer_joins_and_reads_but_cannot_publish() {
    let b = backend();
    let (mut alice, mut vera) = (Peer::start(&b, "@alice:mem"), Peer::start(&b, "@vera:mem"));
    for p in [&mut alice, &mut vera] {
        p.hello().await;
        p.connect().await;
    }
    let (doc, invite, _) = share(
        &mut alice,
        vec![Person { principal: "@vera:mem".into(), role: Role::Viewer }],
        b"g",
    )
    .await;
    assert!(matches!(
        join(&mut vera, &invite).await,
        Event::Joined { role: Role::Viewer, .. }
    ));
    vera.wait("the genesis", |m| matches!(m.event, Event::Genesis { .. }).then_some(())).await;
    let req = vera.send(Command::PublishUpdate { doc: doc.clone(), bytes: b"x".to_vec() });
    assert!(matches!(vera.reply(req).await, Event::Error { code: ErrorCode::Forbidden, .. }));
    let req = vera.send(Command::PublishVersion {
        doc: doc.clone(),
        bytes: b"y".to_vec(),
        message: None,
        resolve: vec![],
    });
    assert!(matches!(vera.reply(req).await, Event::Error { code: ErrorCode::Forbidden, .. }));
    // Nothing of hers reached the log.
    assert_eq!(b.store("@alice:mem", "probe").updates(&doc).await.unwrap().len(), 1);
    assert_eq!(b.store("@alice:mem", "probe").versions(&doc).await.unwrap().len(), 1);
    // And she cannot change the roster.
    let req = vera.send(Command::Grant { doc, principal: "@vera:mem".into(), role: Role::Owner });
    assert!(matches!(vera.reply(req).await, Event::Error { code: ErrorCode::Forbidden, .. }));
}

#[tokio::test]
async fn a_shell_rejection_is_reported_as_a_drop_and_an_invite_for_another_server_is_refused() {
    let b = backend();
    let (mut alice, mut bob) = (Peer::start(&b, "@alice:mem"), Peer::start(&b, "@bob:mem"));
    for p in [&mut alice, &mut bob] {
        p.hello().await;
        p.connect().await;
    }
    let (doc, invite, _) = share(
        &mut alice,
        vec![Person { principal: "@bob:mem".into(), role: Role::Editor }],
        b"g",
    )
    .await;
    let wrong = invite.replace("127.0.0.1:1", "elsewhere.example");
    assert!(matches!(
        join(&mut bob, &wrong).await,
        Event::Error { code: ErrorCode::InvalidInvite, .. }
    ));
    assert!(matches!(
        join(&mut bob, "not an invite").await,
        Event::Error { code: ErrorCode::InvalidInvite, .. }
    ));
    join(&mut bob, &invite).await;
    let gid = bob
        .wait("genesis", |m| match &m.event {
            Event::Genesis { id, .. } => Some(id.clone()),
            _ => None,
        })
        .await;
    bob.send(Command::Rejected { doc: doc.clone(), id: gid.clone(), why: "does not rebuild".into() });
    let (kind, id, reason) = bob
        .wait("the drop", |m| match &m.event {
            Event::Dropped { kind, id, reason, .. } => Some((*kind, id.clone(), reason.clone())),
            _ => None,
        })
        .await;
    assert_eq!((kind, id), (suite_collab::DropKind::Update, gid));
    assert!(reason.contains("does not rebuild"), "{reason}");
}

#[tokio::test]
async fn leaving_stops_the_document_and_a_second_join_of_an_open_one_is_refused() {
    let b = backend();
    let mut alice = Peer::start(&b, "@alice:mem");
    alice.hello().await;
    alice.connect().await;
    let (doc, invite, _) = share(&mut alice, vec![], b"g").await;
    assert!(matches!(
        join(&mut alice, &invite).await,
        Event::Error { code: ErrorCode::Conflict, .. }
    ));
    let req = alice.send(Command::Leave { doc: doc.clone() });
    assert!(matches!(alice.reply(req).await, Event::Done { .. }));
    let req = alice.send(Command::PublishUpdate { doc, bytes: vec![1] });
    assert!(matches!(
        alice.reply(req).await,
        Event::Error { code: ErrorCode::UnknownDocument, .. }
    ));
}

#[tokio::test]
async fn a_file_that_does_not_open_is_never_shared() {
    let mut alice = Peer::start(&backend(), "@alice:mem");
    alice.hello().await;
    alice.connect().await;
    let req = alice.send(Command::Share {
        title: "Broken".into(),
        mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
        first_version: b"PK this is not a zip".to_vec(),
        genesis: None,
        doc_model: "tables/1".into(),
        people: vec![],
    });
    assert!(matches!(
        alice.reply(req).await,
        Event::Error { code: ErrorCode::Invalid, .. }
    ));
    let req = alice.send(Command::Share {
        title: "Odd".into(),
        mime: MIME.into(),
        first_version: b"x".to_vec(),
        genesis: Some(b"g".to_vec()),
        doc_model: "decks/1".into(),
        people: vec![],
    });
    assert!(matches!(
        alice.reply(req).await,
        Event::Error { code: ErrorCode::Invalid, .. }
    ));
}
