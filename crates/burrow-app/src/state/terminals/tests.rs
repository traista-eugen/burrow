//! The router and the size rule, without GPUI or a pipe. The hub against a real `burrowd` is in
//! `state/tests.rs`.

use burrow_protocol::methods::TerminalScrollback;
use burrow_protocol::{
    CursorShape, ErrorKind, ErrorObject, Notification, Response, TerminalClosedParams,
    TerminalCursor, TerminalModes, TerminalScrollbackResult,
};

use super::*;

/// Records what the router sends, numbering requests from 1.
#[derive(Default)]
struct Recorder {
    sent: Vec<(RequestId, Outgoing)>,
    down: bool,
}

impl Wire for Recorder {
    fn send(&mut self, request: &Outgoing) -> Option<RequestId> {
        if self.down {
            return None;
        }
        let id = RequestId::Number(self.sent.len() as i64 + 1);
        self.sent.push((id.clone(), request.clone()));
        Some(id)
    }
}

impl Recorder {
    /// The id of the last request of this kind.
    fn last(&self, request: &Outgoing) -> RequestId {
        self.sent
            .iter()
            .rev()
            .find(|(_, sent)| sent == request)
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| panic!("{request:?} wasn't sent: {:?}", self.sent))
    }

    fn requests(&self) -> Vec<Outgoing> {
        self.sent
            .iter()
            .map(|(_, request)| request.clone())
            .collect()
    }
}

fn cursor() -> TerminalCursor {
    TerminalCursor {
        row: 0,
        col: 0,
        visible: true,
        shape: CursorShape::Block,
    }
}

fn snapshot(terminal: &str, cols: u16, rows: u16) -> TerminalSnapshot {
    TerminalSnapshot {
        terminal_id: terminal.into(),
        cols,
        rows,
        cursor: cursor(),
        lines: Vec::new(),
        modes: TerminalModes::default(),
        title: None,
        scrollback: 0,
        exit: None,
    }
}

fn update(terminal: &str, scrolled: u64) -> Incoming {
    Incoming::Notification(Notification::typed::<TerminalUpdated>(&TerminalUpdate {
        terminal_id: terminal.into(),
        reset: false,
        cols: None,
        rows: None,
        scrolled,
        lines: Vec::new(),
        cursor: cursor(),
        modes: None,
        title: None,
        scrollback: None,
        bell: false,
    }))
}

fn attached(id: RequestId, terminal: &str) -> Incoming {
    Incoming::Answer(Response::ok::<TerminalAttach>(
        id,
        &snapshot(terminal, 80, 24),
    ))
}

fn empty(id: RequestId) -> Incoming {
    Incoming::Answer(Response::ok::<TerminalDetach>(id, &Empty {}))
}

fn scrolled(message: Option<(Key, TerminalMessage)>) -> Option<(Key, u64)> {
    match message? {
        (key, TerminalMessage::Update(update)) => Some((key, update.scrolled)),
        (_, other) => panic!("expected an update, got {other:?}"),
    }
}

fn attach(terminal: &str) -> Outgoing {
    Outgoing::Attach(terminal.into())
}

fn detach(terminal: &str) -> Outgoing {
    Outgoing::Detach(terminal.into())
}

#[test]
fn updates_follow_the_snapshot_they_came_after() {
    let mut router = Router::new();
    let mut wire = Recorder::default();
    // Registered before the connection is up: it attaches once it is.
    let key = router.register("trm_a", &mut wire);
    assert!(wire.sent.is_empty());
    router.connected(&mut wire);
    assert_eq!(wire.requests(), [attach("trm_a")]);

    let id = wire.last(&attach("trm_a"));
    assert!(
        router.incoming(attached(id, "trm_a")).is_some_and(
            |(to, message)| to == key && matches!(message, TerminalMessage::Snapshot(_))
        )
    );
    assert!(router.is_attached("trm_a", key));
    assert_eq!(
        scrolled(router.incoming(update("trm_a", 2))),
        Some((key, 2))
    );
    // Another terminal's updates, and other notifications, aren't this one's.
    assert!(router.incoming(update("trm_other", 1)).is_none());
    let other = Incoming::Notification(Notification::new("test/other", None));
    assert!(router.incoming(other).is_none());
    // An answer nobody asked for is ignored.
    assert!(router.incoming(empty(RequestId::Number(99))).is_none());
}

#[test]
fn a_new_registration_gets_its_own_snapshot_and_none_of_the_old_updates() {
    let mut router = Router::new();
    let mut wire = Recorder::default();
    router.connected(&mut wire);
    let old = router.register("trm_a", &mut wire);
    router.incoming(attached(wire.last(&attach("trm_a")), "trm_a"));

    // Registering the same terminal again detaches, then attaches, in order.
    let new = router.register("trm_a", &mut wire);
    assert_eq!(
        wire.requests(),
        [attach("trm_a"), detach("trm_a"), attach("trm_a")]
    );
    // Updates of the old attachment, sent before burrowd handled the detach, reach no one.
    assert!(router.incoming(update("trm_a", 5)).is_none());
    assert!(
        router
            .incoming(empty(wire.last(&detach("trm_a"))))
            .is_none()
    );
    assert!(router.incoming(update("trm_a", 5)).is_none());
    // The new one's snapshot, then its updates.
    let (to, message) = router
        .incoming(attached(wire.last(&attach("trm_a")), "trm_a"))
        .unwrap();
    assert_eq!(to, new);
    assert!(matches!(message, TerminalMessage::Snapshot(_)));
    assert_eq!(
        scrolled(router.incoming(update("trm_a", 1))),
        Some((new, 1))
    );
    // The old key no longer detaches anything.
    router.unregister("trm_a", old, &mut wire);
    assert_eq!(wire.sent.len(), 3);
    assert!(router.is_attached("trm_a", new));
}

#[test]
fn unregistering_detaches_and_drops_later_updates() {
    let mut router = Router::new();
    let mut wire = Recorder::default();
    router.connected(&mut wire);
    let key = router.register("trm_a", &mut wire);
    // Unregistered before the attach answer: the answer is ignored, and a detach follows it.
    router.unregister("trm_a", key, &mut wire);
    assert_eq!(wire.requests(), [attach("trm_a"), detach("trm_a")]);
    assert!(
        router
            .incoming(attached(wire.last(&attach("trm_a")), "trm_a"))
            .is_none()
    );
    assert!(router.incoming(update("trm_a", 1)).is_none());
    // Unregistering what isn't registered sends nothing.
    router.unregister("trm_a", key, &mut wire);
    assert_eq!(wire.sent.len(), 2);
}

#[test]
fn scrollback_pages_arrive_in_order_with_the_updates() {
    let mut router = Router::new();
    let mut wire = Recorder::default();
    router.connected(&mut wire);
    let key = router.register("trm_a", &mut wire);
    // Not attached yet: nothing to page.
    assert!(!router.scrollback("trm_a", key, 0, 500, &mut wire));
    router.incoming(attached(wire.last(&attach("trm_a")), "trm_a"));

    assert!(router.scrollback("trm_a", key, -10, 5000, &mut wire));
    let page = Outgoing::Scrollback {
        terminal: "trm_a".into(),
        before: -10,
        limit: MAX_PAGE,
    };
    assert_eq!(wire.requests().last(), Some(&page));
    // An update burrowd sent before the page comes first; the page after it.
    assert_eq!(
        scrolled(router.incoming(update("trm_a", 3))),
        Some((key, 3))
    );
    let lines = vec![TerminalLine {
        index: -11,
        ..TerminalLine::default()
    }];
    let answer = Response::ok::<TerminalScrollback>(
        wire.last(&page),
        &TerminalScrollbackResult {
            lines: lines.clone(),
            complete: true,
        },
    );
    assert_eq!(
        router.incoming(Incoming::Answer(answer)),
        Some((
            key,
            TerminalMessage::History {
                lines,
                complete: true
            }
        ))
    );
    // A failed page ends paging.
    assert!(router.scrollback("trm_a", key, -11, 100, &mut wire));
    let failed = Response::error(
        Some(wire.sent.last().unwrap().0.clone()),
        ErrorObject::new(ErrorKind::NotFound, "gone"),
    );
    assert_eq!(
        router.incoming(Incoming::Answer(failed)),
        Some((
            key,
            TerminalMessage::History {
                lines: Vec::new(),
                complete: true
            }
        ))
    );
}

#[test]
fn a_reconnect_attaches_again_and_a_failure_is_reported() {
    let mut router = Router::new();
    let mut wire = Recorder::default();
    router.connected(&mut wire);
    let a = router.register("trm_a", &mut wire);
    let b = router.register("trm_b", &mut wire);
    let first_a = wire.last(&attach("trm_a"));
    router.incoming(attached(first_a.clone(), "trm_a"));

    let mut told = router.disconnected();
    told.sort_by_key(|(key, _)| *key);
    assert_eq!(
        told,
        [
            (a, TerminalMessage::Disconnected),
            (b, TerminalMessage::Disconnected)
        ]
    );
    assert!(!router.is_attached("trm_a", a));
    // Nothing is sent while down.
    wire.down = true;
    assert!(!router.scrollback("trm_a", a, 0, 10, &mut wire));

    wire.down = false;
    let mut wire = Recorder {
        sent: Vec::new(),
        down: false,
    };
    router.connected(&mut wire);
    let mut requests = wire.requests();
    requests.sort_by_key(|request| format!("{request:?}"));
    assert_eq!(requests, [attach("trm_a"), attach("trm_b")]);
    // The new connection's answers count; an attach that fails says why.
    let (to, message) = router
        .incoming(attached(wire.last(&attach("trm_a")), "trm_a"))
        .unwrap();
    assert_eq!(to, a);
    assert!(matches!(message, TerminalMessage::Snapshot(_)));
    let gone = Response::error(
        Some(wire.last(&attach("trm_b"))),
        ErrorObject::new(ErrorKind::NotFound, "no terminal trm_b"),
    );
    assert_eq!(
        router.incoming(Incoming::Answer(gone)),
        Some((b, TerminalMessage::Failed("no terminal trm_b".into())))
    );
    assert!(!router.is_attached("trm_b", b));
    // terminal/closed goes to the attached registration.
    let closed = Notification::typed::<TerminalClosedNotification>(&TerminalClosedParams {
        terminal_id: "trm_a".into(),
        exit_code: Some(3),
    });
    assert_eq!(
        router.incoming(Incoming::Notification(closed)),
        Some((a, TerminalMessage::Closed { exit_code: Some(3) }))
    );
}

fn size(cols: u16, rows: u16) -> TerminalSize {
    TerminalSize { cols, rows }
}

#[test]
fn a_session_started_here_follows_the_view() {
    let mut sizing = Sizing::new(true);
    // The view lays out before the snapshot: nothing to ask yet.
    assert_eq!(sizing.view_fits(size(100, 30)), None);
    // Started at an estimate one row off: the view's size is asked for at once.
    assert_eq!(sizing.attached(size(100, 31)), Some(size(100, 30)));
    sizing.resized(size(100, 30));
    // And follows the window while it holds the size.
    assert_eq!(sizing.view_fits(size(120, 40)), Some(size(120, 40)));
    assert_eq!(sizing.view_fits(size(120, 40)), None);
    assert_eq!(sizing.before_input(), None);
}

#[test]
fn a_glance_changes_nothing_until_the_user_types() {
    let mut sizing = Sizing::new(false);
    // Another client's size: shown as it is.
    assert_eq!(sizing.attached(size(80, 24)), None);
    assert_eq!(sizing.view_fits(size(120, 40)), None);
    assert_eq!(sizing.view_fits(size(110, 40)), None);
    // Typing asks for the view's size first, then follows the window.
    assert_eq!(sizing.before_input(), Some(size(110, 40)));
    assert!(sizing.holds());
    sizing.resized(size(110, 40));
    assert_eq!(sizing.view_fits(size(100, 40)), Some(size(100, 40)));
    // A size the terminal already has is asked for at once: nothing changes, and burrowd
    // remembers it for this client.
    let mut other = Sizing::new(false);
    assert_eq!(other.attached(size(80, 24)), None);
    assert_eq!(other.view_fits(size(80, 24)), Some(size(80, 24)));
    assert!(!other.holds());
}

#[test]
fn another_client_typing_takes_the_size_away() {
    let mut sizing = Sizing::new(true);
    sizing.view_fits(size(100, 30));
    assert_eq!(sizing.attached(size(100, 30)), Some(size(100, 30)));
    // The console typed: its size now.
    sizing.resized(size(80, 24));
    assert!(!sizing.holds());
    assert_eq!(sizing.view_fits(size(90, 30)), None);
    // Typing here takes it back.
    assert_eq!(sizing.before_input(), Some(size(90, 30)));
    // burrowd resizes on input to what this client asked; nothing more to ask.
    assert_eq!(sizing.before_input(), None);
}

#[test]
fn a_reconnect_asks_again_only_if_nothing_moved() {
    let mut sizing = Sizing::new(true);
    sizing.view_fits(size(100, 30));
    sizing.attached(size(100, 30));
    // Back at the same size: burrowd forgot this client's size, so it is asked again, which
    // changes nothing.
    assert_eq!(sizing.attached(size(100, 30)), Some(size(100, 30)));
    assert!(sizing.holds());
    // Someone else resized it meanwhile: this client no longer holds it.
    assert_eq!(sizing.attached(size(80, 24)), None);
    assert!(!sizing.holds());
}
