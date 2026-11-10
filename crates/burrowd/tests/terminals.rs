//! Terminal sessions through the real `burrowd`, with the fake TUI (`burrow-fake-agent --tui`)
//! on a real pseudo console: attaching, typing, several clients, the resize rule, scrollback,
//! exits, stopping, restarts, and a client that doesn't read. No agent quota is spent.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use burrow_agents::launcher::{descendants, is_running};
use burrow_client::{CallError, Client, Notifications};
use burrow_protocol::methods::{
    SessionCreate, SessionGet, SessionList, SessionPrompt, SessionStop, TerminalAttach,
    TerminalDetach, TerminalInput, TerminalResize, TerminalScrollback,
};
use burrow_protocol::notifications::{TerminalClosedNotification, TerminalUpdated};
use burrow_protocol::*;
use common::*;

async fn start(dir: &Path, test: &str) -> (Daemon, Client) {
    let pipe = unique_pipe(test);
    let daemon = Daemon::start(dir, &pipe, true).await;
    let client = connect(&pipe).await;
    (daemon, client)
}

/// Creates a terminal session running the fake TUI, and returns it with its terminal's id.
async fn create(client: &Client, cwd: &Path) -> (Session, String) {
    let session = client
        .call::<SessionCreate>(&SessionCreateParams {
            project_id: None,
            agent: AgentId::Fake,
            model: None,
            mode: SessionMode::Terminal,
            agent_mode: None,
            cwd: Some(cwd.to_string_lossy().into_owned()),
            title: None,
            cols: None,
            rows: None,
        })
        .await
        .expect("session/create");
    assert_eq!(session.mode, SessionMode::Terminal);
    assert_eq!(session.state, SessionState::Idle);
    let terminal = terminal_of(client, &session.id).await;
    (session, terminal)
}

/// The terminal a session's stream announces.
async fn terminal_of(client: &Client, session: &str) -> String {
    let mut events = client
        .subscribe(StreamName::session(session), Some(0))
        .await
        .unwrap();
    let found = tokio::time::timeout(WAIT, async {
        loop {
            let event = events.next().await.expect("the stream ended");
            if let Event::TerminalOpened(opened) = event.event {
                return opened.terminal_id;
            }
        }
    })
    .await
    .expect("terminalOpened");
    events.unsubscribe().await.unwrap();
    found
}

/// What a client has drawn: the snapshot with every update applied, as protocol.md says, and
/// the scrollback lines that scrolled off its screen meanwhile.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Model {
    terminal: String,
    cols: u16,
    rows: u16,
    lines: Vec<TerminalLine>,
    /// Scrollback the client saw scroll off, newest first: `history[0]` is index -1.
    history: Vec<TerminalLine>,
    cursor: TerminalCursor,
    modes: TerminalModes,
    title: Option<String>,
    scrollback: u64,
    closed: Option<Option<i32>>,
    updates: usize,
    resets: usize,
    bells: usize,
}

impl Model {
    fn new(snapshot: TerminalSnapshot) -> Self {
        assert_eq!(snapshot.lines.len(), usize::from(snapshot.rows));
        for (row, line) in snapshot.lines.iter().enumerate() {
            assert_eq!(line.index, row as i64);
        }
        Self {
            terminal: snapshot.terminal_id,
            cols: snapshot.cols,
            rows: snapshot.rows,
            lines: snapshot.lines,
            history: Vec::new(),
            cursor: snapshot.cursor,
            modes: snapshot.modes,
            title: snapshot.title,
            scrollback: snapshot.scrollback,
            closed: snapshot.exit.map(|exit| exit.exit_code),
            updates: 0,
            resets: 0,
            bells: 0,
        }
    }

    fn blank(index: i64) -> TerminalLine {
        TerminalLine {
            index,
            ..TerminalLine::default()
        }
    }

    fn apply(&mut self, update: TerminalUpdate) {
        assert_eq!(update.terminal_id, self.terminal);
        self.updates += 1;
        if update.reset {
            let (Some(cols), Some(rows)) = (update.cols, update.rows) else {
                panic!("a reset without its size: {update:?}");
            };
            self.resets += 1;
            self.cols = cols;
            self.rows = rows;
            self.lines = (0..i64::from(rows)).map(Self::blank).collect();
            self.history.clear();
            assert_eq!(
                update.lines.len(),
                usize::from(rows),
                "a reset sends every row"
            );
            assert_eq!(update.scrolled, 0);
        }
        for _ in 0..update.scrolled {
            let mut top = self.lines.remove(0);
            top.index = -1;
            self.history.insert(0, top);
            self.lines.push(Self::blank(0));
        }
        for (row, line) in self.lines.iter_mut().enumerate() {
            line.index = row as i64;
        }
        for line in update.lines {
            if line.index >= 0 {
                let row = line.index as usize;
                self.lines[row] = line;
            } else {
                let at = (-line.index - 1) as usize;
                if self.history.len() <= at {
                    self.history.resize(at + 1, Self::blank(-1));
                }
                self.history[at] = line;
            }
        }
        self.cursor = update.cursor;
        if let Some(modes) = update.modes {
            self.modes = modes;
        }
        if let Some(title) = update.title {
            self.title = (!title.is_empty()).then_some(title);
        }
        if let Some(scrollback) = update.scrollback {
            self.scrollback = scrollback;
        }
        if update.bell {
            self.bells += 1;
        }
    }

    fn text(&self) -> Vec<String> {
        self.lines.iter().map(TerminalLine::text).collect()
    }

    fn row(&self, row: usize) -> String {
        self.lines[row].text()
    }

    fn contains(&self, needle: &str) -> bool {
        self.lines.iter().any(|line| line.text().contains(needle))
    }

    /// The fake TUI's status line, on the last row.
    fn status(&self) -> String {
        self.row(usize::from(self.rows) - 1)
    }
}

/// An attached client: its notifications and what it has drawn.
struct Attached {
    notifications: Notifications,
    model: Model,
}

async fn attach(client: &Client, terminal: &str) -> Attached {
    // Listen first: notifications that arrive while nobody listens are dropped.
    let notifications = client.notifications();
    let snapshot = client
        .call::<TerminalAttach>(&TerminalIdParams {
            terminal_id: terminal.to_owned(),
        })
        .await
        .expect("terminal/attach");
    Attached {
        notifications,
        model: Model::new(snapshot),
    }
}

impl Attached {
    /// Applies updates until `done` holds, and fails after [`WAIT`].
    async fn until(&mut self, what: &str, done: impl Fn(&Model) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.model) {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.notifications.next()).await {
                Ok(Some(notification)) => self.handle(notification),
                Ok(None) => panic!("the connection closed while waiting for {what}"),
                Err(_) => panic!(
                    "timed out waiting for {what}; the screen:\n{:#?}",
                    self.model.text()
                ),
            }
        }
    }

    fn handle(&mut self, notification: Notification) {
        match notification.method.as_str() {
            TerminalUpdated::NAME => self
                .model
                .apply(notification.params_as::<TerminalUpdated>().unwrap()),
            TerminalClosedNotification::NAME => {
                let closed = notification
                    .params_as::<TerminalClosedNotification>()
                    .unwrap();
                assert_eq!(closed.terminal_id, self.model.terminal);
                self.model.closed = Some(closed.exit_code);
            }
            other => panic!("unexpected notification {other}"),
        }
    }

    /// Applies whatever arrives within `quiet` of the previous notification.
    async fn settle(&mut self, quiet: Duration) {
        while let Ok(Some(notification)) =
            tokio::time::timeout(quiet, self.notifications.next()).await
        {
            self.handle(notification);
        }
    }
}

async fn input(client: &Client, terminal: &str, data: &str) {
    client
        .call::<TerminalInput>(&TerminalInputParams {
            terminal_id: terminal.to_owned(),
            data: data.to_owned(),
        })
        .await
        .expect("terminal/input");
}

async fn resize(client: &Client, terminal: &str, cols: u16, rows: u16) {
    client
        .call::<TerminalResize>(&TerminalResizeParams {
            terminal_id: terminal.to_owned(),
            cols,
            rows,
        })
        .await
        .expect("terminal/resize");
}

async fn get(client: &Client, session: &str) -> Session {
    client
        .call::<SessionGet>(&SessionIdParams {
            session_id: session.to_owned(),
        })
        .await
        .unwrap()
}

fn error_kind(result: Result<impl std::fmt::Debug, CallError>) -> ErrorKind {
    result
        .expect_err("expected an error")
        .rpc()
        .and_then(ErrorObject::kind)
        .expect("a Burrow error kind")
}

/// The two pids `/pids` prints.
fn pids(model: &Model) -> (u32, u32) {
    let line = model
        .text()
        .into_iter()
        .find(|line| line.starts_with("pids "))
        .expect("a pids line");
    let mut numbers = line
        .split_whitespace()
        .skip(1)
        .map(|n| n.parse::<u32>().unwrap());
    (numbers.next().unwrap(), numbers.next().unwrap())
}

/// Waits until `pids` are gone, giving process teardown a moment.
async fn gone(pids: &[u32]) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while pids.iter().any(|&pid| is_running(pid)) {
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_type_and_see_the_screen_change() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "terminal-attach").await;
    let (session, terminal) = create(&client, dir.path()).await;
    assert!(terminal.starts_with("trm_"), "{terminal}");

    let mut a = attach(&client, &terminal).await;
    assert_eq!((a.model.cols, a.model.rows), (120, 32), "the default size");
    a.until("the first screen", |m| {
        m.row(0).starts_with("fake tui") && m.status().starts_with("status 120x32")
    })
    .await;
    // Colours and styles come through the console: bold green, red, orange, inverse, wide.
    let title = &a.model.lines[0];
    assert_eq!(title.text(), "fake tui red orange inverse 漢字");
    assert_eq!(title.runs[0].text, "fake tui");
    assert_eq!(title.runs[0].fg, Some(Color::Indexed(2)));
    assert_eq!(title.runs[0].flags & TerminalRun::BOLD, TerminalRun::BOLD);
    // Conhost repaints the screen its own way: a colour may run on over the spaces after it.
    let run = |text: &str| {
        title
            .runs
            .iter()
            .find(|run| run.text.trim() == text)
            .unwrap_or_else(|| panic!("no run {text:?} in {:?}", title.runs))
            .clone()
    };
    assert_eq!(run("red").fg, Some(Color::Indexed(1)));
    assert_eq!(run("orange").fg, Some(Color::Rgb(255, 128, 0)));
    assert_eq!(
        run("inverse").flags & TerminalRun::INVERSE,
        TerminalRun::INVERSE
    );
    // Trailing blank cells are left out of a line.
    assert_eq!(a.model.row(1), ">");
    assert_eq!((a.model.cursor.row, a.model.cursor.col), (1, 2));
    assert!(a.model.cursor.visible);
    assert_eq!(
        a.model.lines[31].runs[0].flags & TerminalRun::INVERSE,
        TerminalRun::INVERSE
    );

    input(&client, &terminal, "hello").await;
    a.until("the echo of typing", |m| m.row(1) == "> hello")
        .await;
    assert_eq!(a.model.cursor.col, 7);
    input(&client, &terminal, "\r").await;
    a.until("the answer", |m| {
        m.row(2) == "echo: hello" && m.row(3) == ">"
    })
    .await;
    // Special keys reach the program as typed.
    input(&client, &terminal, "\x1b[A").await;
    a.until("the arrow key", |m| m.status().contains("last \\e[A"))
        .await;

    // Terminal sessions take no prompts, models or modes.
    let refused = client
        .call::<SessionPrompt>(&SessionPromptParams {
            session_id: session.id.clone(),
            content: vec![ContentBlock::text("hi")],
        })
        .await;
    assert_eq!(error_kind(refused), ErrorKind::InvalidState);
    let refused = client
        .call::<SessionCreate>(&SessionCreateParams {
            project_id: None,
            agent: AgentId::Fake,
            model: Some("fake".into()),
            mode: SessionMode::Terminal,
            agent_mode: None,
            cwd: Some(dir.path().to_string_lossy().into_owned()),
            title: None,
            cols: None,
            rows: None,
        })
        .await;
    assert!(matches!(refused, Err(CallError::Rpc(error)) if error.code == code::INVALID_PARAMS));

    // Attaching twice on one connection is refused; detaching twice is fine.
    let twice = client
        .call::<TerminalAttach>(&TerminalIdParams {
            terminal_id: terminal.clone(),
        })
        .await;
    assert_eq!(error_kind(twice), ErrorKind::InvalidState);
    let detach = TerminalIdParams {
        terminal_id: terminal.clone(),
    };
    client.call::<TerminalDetach>(&detach).await.unwrap();
    client.call::<TerminalDetach>(&detach).await.unwrap();
    let unknown = client
        .call::<TerminalAttach>(&TerminalIdParams {
            terminal_id: "trm_nope".into(),
        })
        .await;
    assert_eq!(error_kind(unknown), ErrorKind::NotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_terminal_session_starts_at_the_size_asked_and_names_its_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "terminal-create-size").await;
    let params = |mode: SessionMode, cols: Option<u16>, rows: Option<u16>| SessionCreateParams {
        mode,
        cols,
        rows,
        ..SessionCreateParams::new(AgentId::Fake, dir.path().to_string_lossy())
    };
    let session = client
        .call::<SessionCreate>(&params(SessionMode::Terminal, Some(90), Some(20)))
        .await
        .expect("session/create");

    // The session names its terminal, as its stream does.
    let terminal = session.terminal_id.clone().expect("Session.terminalId");
    assert_eq!(terminal_of(&client, &session.id).await, terminal);
    assert_eq!(
        get(&client, &session.id).await.terminal_id,
        Some(terminal.clone())
    );

    // The program drew its first screen at that size: no resize was needed.
    let mut a = attach(&client, &terminal).await;
    assert_eq!((a.model.cols, a.model.rows), (90, 20));
    a.until("the first screen at 90x20", |m| {
        m.status().starts_with("status 90x20")
    })
    .await;
    assert_eq!(a.model.resets, 0);

    // Lists and the index say it is a terminal session.
    let listed = client
        .call::<SessionList>(&SessionListParams::default())
        .await
        .unwrap();
    let summary = listed.iter().find(|s| s.id == session.id).unwrap();
    assert_eq!(summary.mode, SessionMode::Terminal);
    let mut index = client
        .subscribe(StreamName::index(), Some(0))
        .await
        .unwrap();
    let created = tokio::time::timeout(WAIT, async {
        loop {
            if let Event::EntityCreated(created) = index.next().await.unwrap().event
                && created.id == session.id
            {
                return created;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(created.mode, Some(SessionMode::Terminal));

    // Both or neither, within terminal/resize's bounds, and only for terminal sessions.
    for (mode, cols, rows) in [
        (SessionMode::Terminal, Some(90), None),
        (SessionMode::Terminal, None, Some(20)),
        (SessionMode::Terminal, Some(1), Some(20)),
        (SessionMode::Terminal, Some(90), Some(501)),
        (SessionMode::Structured, Some(90), Some(20)),
    ] {
        let refused = client
            .call::<SessionCreate>(&params(mode, cols, rows))
            .await;
        assert!(
            matches!(&refused, Err(CallError::Rpc(error)) if error.code == code::INVALID_PARAMS),
            "{mode:?} {cols:?}x{rows:?}: {refused:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_detaches_and_attaches_again_sees_exactly_the_current_screen() {
    let dir = tempfile::tempdir().unwrap();
    let (daemon, client) = start(dir.path(), "terminal-reattach").await;
    let (_session, terminal) = create(&client, dir.path()).await;

    let mut a = attach(&client, &terminal).await;
    a.until("the first screen", |m| {
        m.status().starts_with("status 120x32")
    })
    .await;
    for word in ["one", "two", "three"] {
        input(&client, &terminal, &format!("{word}\r")).await;
    }
    input(&client, &terminal, "/title fake title\r/bell\r").await;
    a.until("the answers", |m| {
        m.contains("echo: three") && m.title.is_some() && m.bells > 0
    })
    .await;
    assert_eq!(a.model.title.as_deref(), Some("fake title"));
    a.settle(Duration::from_millis(300)).await;
    client
        .call::<TerminalDetach>(&TerminalIdParams {
            terminal_id: terminal.clone(),
        })
        .await
        .unwrap();
    // Nothing follows the detach's answer, even as the screen changes.
    let other = connect(&daemon.pipe).await;
    input(&other, &terminal, "while detached\r").await;
    let late = tokio::time::timeout(Duration::from_millis(300), a.notifications.next()).await;
    assert!(late.is_err(), "an update after detaching: {late:?}");

    // A fresh attach, on this connection or another, has the screen as it is now.
    let mut b = attach(&other, &terminal).await;
    assert!(
        b.model.contains("echo: while detached"),
        "{:#?}",
        b.model.text()
    );
    let mut again = attach(&client, &terminal).await;
    again.settle(Duration::from_millis(300)).await;
    b.settle(Duration::from_millis(300)).await;
    assert_eq!(again.model.text(), b.model.text());
    assert_eq!(again.model.cursor, b.model.cursor);
    assert_eq!(again.model.title.as_deref(), Some("fake title"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_attached_at_once_follow_the_same_screen() {
    let dir = tempfile::tempdir().unwrap();
    let (daemon, client) = start(dir.path(), "terminal-two").await;
    let (_session, terminal) = create(&client, dir.path()).await;
    let other = connect(&daemon.pipe).await;
    let mut a = attach(&client, &terminal).await;
    let mut b = attach(&other, &terminal).await;

    input(&client, &terminal, "from a\r").await;
    input(&other, &terminal, "from b\r").await;
    let done = |m: &Model| m.contains("echo: from a") && m.contains("echo: from b");
    a.until("both answers on a", done).await;
    b.until("both answers on b", done).await;
    a.settle(Duration::from_millis(200)).await;
    b.settle(Duration::from_millis(200)).await;
    assert_eq!(a.model.text(), b.model.text());
    assert_eq!(a.model.cursor, b.model.cursor);
}

fn at(cols: u16, rows: u16) -> impl Fn(&Model) -> bool {
    move |m: &Model| {
        (m.cols, m.rows) == (cols, rows) && m.status().starts_with(&format!("status {cols}x{rows}"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_client_that_last_resized_or_typed_sets_the_size() {
    let dir = tempfile::tempdir().unwrap();
    let (daemon, client) = start(dir.path(), "terminal-resize").await;
    let (_session, terminal) = create(&client, dir.path()).await;
    let other = connect(&daemon.pipe).await;
    let mut a = attach(&client, &terminal).await;
    let mut b = attach(&other, &terminal).await;
    a.until("the first screen", at(120, 32)).await;

    // A resizes: both draw at A's size, and the program sees it.
    resize(&client, &terminal, 100, 30).await;
    a.until("100x30 on a", at(100, 30)).await;
    b.until("100x30 on b", at(100, 30)).await;

    // B resizes: B's size wins.
    resize(&other, &terminal, 80, 24).await;
    a.until("80x24 on a", at(80, 24)).await;
    b.until("80x24 on b", at(80, 24)).await;

    // A types: A asked for 100x30 last, so the terminal takes that size again first.
    input(&client, &terminal, "x").await;
    a.until("100x30 again on a", at(100, 30)).await;
    b.until("100x30 again on b", at(100, 30)).await;
    // Typing at the size already set changes nothing.
    input(&client, &terminal, "y").await;
    a.until("the typing", |m| m.contains("xy")).await;
    assert_eq!((a.model.cols, a.model.rows), (100, 30));

    // A client that never resized types without changing the size.
    let third = connect(&daemon.pipe).await;
    let _c = attach(&third, &terminal).await;
    input(&third, &terminal, "z").await;
    a.until("the typing", |m| m.contains("xyz")).await;
    assert_eq!((a.model.cols, a.model.rows), (100, 30));

    // Sizes have bounds.
    let refused = third
        .call::<TerminalResize>(&TerminalResizeParams {
            terminal_id: terminal.clone(),
            cols: 1,
            rows: 24,
        })
        .await;
    assert!(matches!(refused, Err(CallError::Rpc(error)) if error.code == code::INVALID_PARAMS));
}

async fn page(
    client: &Client,
    terminal: &str,
    before: i64,
    limit: u32,
) -> TerminalScrollbackResult {
    client
        .call::<TerminalScrollback>(&TerminalScrollbackParams {
            terminal_id: terminal.to_owned(),
            before: Some(before),
            limit: Some(limit),
        })
        .await
        .unwrap()
}

/// Pages back through the whole scrollback, 50 lines at a time: oldest first.
async fn whole_scrollback(client: &Client, terminal: &str) -> Vec<TerminalLine> {
    let mut lines: Vec<TerminalLine> = Vec::new();
    let mut before = 0;
    loop {
        let result = page(client, terminal, before, 50).await;
        assert!(result.lines.len() <= 50);
        for pair in result.lines.windows(2) {
            assert_eq!(pair[0].index + 1, pair[1].index, "consecutive indexes");
        }
        if let Some(last) = result.lines.last() {
            assert_eq!(last.index, before - 1, "the page ends just above `before`");
        }
        let complete = result.complete;
        if let Some(first) = result.lines.first() {
            before = first.index;
        }
        let mut older = result.lines;
        older.append(&mut lines);
        lines = older;
        if complete {
            return lines;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scrollback_pages_back_through_the_output_and_matches_what_scrolled() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "terminal-scrollback").await;
    let (_session, terminal) = create(&client, dir.path()).await;
    let mut a = attach(&client, &terminal).await;
    a.until("the first screen", |m| m.status().starts_with("status"))
        .await;
    input(&client, &terminal, "/flood 300\r").await;
    a.until("the flood's end", |m| m.contains("line 300")).await;
    a.settle(Duration::from_millis(300)).await;
    assert!(a.model.scrollback > 250, "{}", a.model.scrollback);

    // Paged from the attached connection: indexes relative to the screen the client has.
    let lines = whole_scrollback(&client, &terminal).await;
    assert_eq!(lines.len() as u64, a.model.scrollback);
    assert_eq!(lines.last().unwrap().index, -1);
    let texts: Vec<String> = lines.iter().map(TerminalLine::text).collect();
    let line_1 = texts
        .iter()
        .position(|text| text == "line 1")
        .unwrap_or_else(|| panic!("no line 1 in {texts:#?}"));
    for n in 1..=200 {
        assert_eq!(texts[line_1 + n - 1], format!("line {n}"));
    }
    // What the client built from `scrolled` and the corrections is the scrollback itself.
    let seen: Vec<String> = a
        .model
        .history
        .iter()
        .rev()
        .map(TerminalLine::text)
        .collect();
    assert_eq!(seen, texts[texts.len() - seen.len()..]);
    assert!(seen.len() > 250, "{}", seen.len());

    // More output scrolls: line 42 is 50 lines further up for the client once it has heard.
    let index_of_line_42 = lines[line_1 + 41].index;
    input(&client, &terminal, "/flood 50\r").await;
    a.until("the second flood", |m| {
        m.contains("line 50") && m.row(0) != "line 1"
    })
    .await;
    a.settle(Duration::from_millis(300)).await;
    let scrolled_since = a.model.history.len() as i64 - seen.len() as i64;
    assert!(scrolled_since >= 50, "{scrolled_since}");
    let result = page(&client, &terminal, index_of_line_42 - scrolled_since + 1, 1).await;
    assert_eq!(result.lines[0].index, index_of_line_42 - scrolled_since);
    assert_eq!(result.lines[0].text(), "line 42");
    // Above the oldest line there is nothing.
    let oldest = lines[0].index - scrolled_since;
    let none = page(&client, &terminal, oldest, 10).await;
    assert!(none.lines.is_empty() && none.complete);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_program_exiting_ends_the_session_with_its_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let (daemon, client) = start(dir.path(), "terminal-exit").await;

    for (code, state) in [(3, SessionState::Failed), (0, SessionState::Stopped)] {
        let (session, terminal) = create(&client, dir.path()).await;
        let mut events = client
            .subscribe(StreamName::session(&session.id), None)
            .await
            .unwrap();
        let mut a = attach(&client, &terminal).await;
        a.until("the first screen", |m| m.status().starts_with("status"))
            .await;
        input(&client, &terminal, &format!("/exit {code}\r")).await;
        a.until("terminal/closed", |m| m.closed.is_some()).await;
        assert_eq!(a.model.closed, Some(Some(code)));
        assert!(a.model.contains("bye"), "the last output arrives first");

        let closed = tokio::time::timeout(WAIT, async {
            let mut seen = Vec::new();
            loop {
                let event = events.next().await.unwrap().event;
                let last = matches!(&event, Event::StateChanged(_));
                seen.push(event);
                if last {
                    return seen;
                }
            }
        })
        .await
        .unwrap();
        assert!(
            matches!(&closed[..], [Event::TerminalClosed(c), Event::StateChanged(s)]
                if c.terminal_id == terminal && c.exit_code == Some(code) && s.state == state),
            "{closed:?}"
        );
        let session = get(&client, &session.id).await;
        assert_eq!(session.state, state);
        assert!(session.reason.unwrap().contains("exited"));

        // The last screen stays readable, but takes no input.
        let late = connect(&daemon.pipe).await;
        let b = attach(&late, &terminal).await;
        assert_eq!(b.model.closed, Some(Some(code)));
        assert!(b.model.contains("bye"));
        let refused = late
            .call::<TerminalInput>(&TerminalInputParams {
                terminal_id: terminal.clone(),
                data: "x".into(),
            })
            .await;
        assert_eq!(error_kind(refused), ErrorKind::InvalidState);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_a_terminal_session_leaves_no_stray_processes() {
    let dir = tempfile::tempdir().unwrap();
    let (daemon, client) = start(dir.path(), "terminal-stop").await;
    let (session, terminal) = create(&client, dir.path()).await;
    let mut a = attach(&client, &terminal).await;
    a.until("the first screen", |m| m.status().starts_with("status"))
        .await;
    input(&client, &terminal, "/pids\r").await;
    a.until("the pids", |m| m.contains("pids ")).await;
    let (tui, child) = pids(&a.model);
    assert!(is_running(tui) && is_running(child));
    let tree = descendants(daemon.pid());
    assert!(tree.contains(&tui) && tree.contains(&child), "{tree:?}");

    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();
    client
        .call::<SessionStop>(&SessionIdParams {
            session_id: session.id.clone(),
        })
        .await
        .unwrap();
    // Gone by the time session/stop answers: the TUI and its child.
    assert!(!is_running(tui), "the fake TUI survived");
    assert!(!is_running(child), "the fake TUI's child survived");
    // The console's host, burrowd's own child, too, once it has closed its pipe.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !descendants(daemon.pid()).is_empty() {
        assert!(
            Instant::now() < deadline,
            "left behind: {:?}",
            descendants(daemon.pid())
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let first = events.next().await.unwrap().event;
    let second = events.next().await.unwrap().event;
    assert!(
        matches!(&first, Event::TerminalClosed(c) if c.terminal_id == terminal && c.exit_code.is_none()),
        "{first:?}"
    );
    assert!(
        matches!(&second, Event::StateChanged(c)
            if c.state == SessionState::Stopped && c.reason.as_deref() == Some("stopped by request")),
        "{second:?}"
    );
    a.until("terminal/closed", |m| m.closed.is_some()).await;
    assert_eq!(
        a.model.closed,
        Some(None),
        "no exit code when Burrow ended it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_marks_terminal_sessions_stopped_and_their_programs_die_with_burrowd() {
    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("terminal-restart");
    let mut daemon = Daemon::start(dir.path(), &pipe, true).await;
    let client = connect(&pipe).await;
    let (session, terminal) = create(&client, dir.path()).await;
    let mut a = attach(&client, &terminal).await;
    a.until("the first screen", |m| m.status().starts_with("status"))
        .await;
    input(&client, &terminal, "/pids\r").await;
    a.until("the pids", |m| m.contains("pids ")).await;
    let (tui, child) = pids(&a.model);

    daemon.kill();
    assert!(gone(&[tui, child]).await, "the fake TUI outlived burrowd");
    drop(client);

    let daemon = Daemon::start(dir.path(), &pipe, true).await;
    let client = connect(&daemon.pipe).await;
    let session = get(&client, &session.id).await;
    assert_eq!(session.state, SessionState::Stopped);
    let mut events = client
        .subscribe(StreamName::session(&session.id), Some(0))
        .await
        .unwrap();
    let events = take(&mut events, 5).await;
    let types: Vec<&str> = events.iter().map(|event| event.event.type_name()).collect();
    assert_eq!(
        types,
        [
            "stateChanged",
            "terminalOpened",
            "stateChanged",
            "terminalClosed",
            "stateChanged"
        ]
    );
    // The terminal itself is gone with the old burrowd.
    let gone = client
        .call::<TerminalAttach>(&TerminalIdParams {
            terminal_id: terminal.clone(),
        })
        .await;
    assert_eq!(error_kind(gone), ErrorKind::NotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn modes_follow_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "terminal-modes").await;
    let (_session, terminal) = create(&client, dir.path()).await;
    let mut a = attach(&client, &terminal).await;
    a.until("the first screen", |m| m.status().starts_with("status"))
        .await;
    input(&client, &terminal, "/alt\r").await;
    a.until("the alternate screen", |m| {
        m.modes.alt_screen && m.row(0) == "alternate screen"
    })
    .await;
    a.settle(Duration::from_millis(300)).await;
    let modes = a.model.modes.clone();
    assert!(modes.bracketed_paste, "{modes:?}");
    assert!(modes.app_cursor, "{modes:?}");
    assert_eq!(modes.mouse, Some(MouseTracking::Click), "{modes:?}");
    assert!(modes.mouse_sgr, "{modes:?}");
    assert_eq!(a.model.scrollback, 0);
    // A bracketed paste arrives as typed text.
    input(&client, &terminal, "\x1b[200~pasted\x1b[201~").await;
    a.until("the paste", |m| m.contains("pasted")).await;

    input(&client, &terminal, "\r/main\r").await;
    a.until("the main screen", |m| {
        !m.modes.alt_screen && m.row(0).starts_with("fake tui")
    })
    .await;
    a.settle(Duration::from_millis(300)).await;
    assert!(
        !a.model.modes.bracketed_paste
            && !a.model.modes.app_cursor
            && a.model.modes.mouse.is_none(),
        "{:?}",
        a.model.modes
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_stops_reading_never_stalls_the_terminal() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ClientOptions;

    let dir = tempfile::tempdir().unwrap();
    let (daemon, client) = start(dir.path(), "terminal-slow").await;
    let (_session, terminal) = create(&client, dir.path()).await;

    // A client on a raw pipe that attaches, then reads nothing.
    let pipe = ClientOptions::new()
        .open(burrow_transport::pipe_path(&daemon.pipe))
        .unwrap();
    let (reader, mut writer) = tokio::io::split(pipe);
    let mut reader = BufReader::new(reader);
    let initialize = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":0,"client":{"name":"slow","version":"0","kind":"cli"}}}"#;
    let attach_line = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"terminal/attach","params":{{"terminalId":"{terminal}"}}}}"#
    );
    for line in [initialize.to_owned(), attach_line] {
        writer.write_all(line.as_bytes()).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
    }

    // Meanwhile a lot of output.
    let mut a = attach(&client, &terminal).await;
    a.until("the first screen", |m| m.status().starts_with("status"))
        .await;
    let started = Instant::now();
    input(&client, &terminal, "/flood 20000\r").await;
    a.until("the flood's end", |m| m.contains("line 20000"))
        .await;
    input(&client, &terminal, "/pids\r").await;
    a.until("an answer after the flood", |m| m.contains("pids "))
        .await;
    let elapsed = started.elapsed();
    a.settle(Duration::from_millis(300)).await;
    // Coalesced: one update per frame at most.
    let frames = elapsed.as_millis() / 16 + 10;
    assert!(
        (a.model.updates as u128) <= frames,
        "{} updates in {elapsed:?}",
        a.model.updates
    );

    // The slow client now reads: it gets a handful of updates, not one per frame, and ends up
    // with the same screen.
    let mut model: Option<Model> = None;
    let mut updates = 0;
    let mut line = String::new();
    while let Ok(Ok(read)) =
        tokio::time::timeout(Duration::from_millis(500), reader.read_line(&mut line)).await
    {
        assert!(read > 0, "the slow client was disconnected");
        match Message::parse(line.trim_end()).unwrap() {
            Message::Response(response) if response.id == Some(RequestId::Number(1)) => {
                let ResponseOutcome::Result(result) = response.outcome else {
                    panic!("attach failed: {response:?}");
                };
                let snapshot: TerminalSnapshot = serde_json::from_value(result).unwrap();
                model = Some(Model::new(snapshot));
            }
            Message::Response(_) => {}
            Message::Notification(notification) if notification.method == TerminalUpdated::NAME => {
                updates += 1;
                model
                    .as_mut()
                    .expect("the snapshot comes first")
                    .apply(notification.params_as::<TerminalUpdated>().unwrap());
            }
            other => panic!("unexpected {other:?}"),
        }
        line.clear();
    }
    let model = model.expect("a snapshot");
    assert_eq!(model.text(), a.model.text(), "the slow client caught up");
    assert!(
        updates < 100,
        "{updates} updates queued for a client that didn't read, over {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn narrowing_the_terminal_keeps_each_line_once() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "terminal-narrow").await;
    let (_session, terminal) = create(&client, dir.path()).await;
    let mut a = attach(&client, &terminal).await;
    a.until("the first screen", at(120, 32)).await;
    input(&client, &terminal, "/long 60\r").await;
    a.until("the long lines", |m| m.contains("long 60 ")).await;
    resize(&client, &terminal, 60, 32).await;
    a.until("60 columns", at(60, 32)).await;
    a.settle(Duration::from_millis(500)).await;

    // The whole text, wrapped lines joined: scrollback, then the screen.
    let mut lines = whole_scrollback(&client, &terminal).await;
    lines.extend(a.model.lines.iter().cloned());
    let mut text = Vec::new();
    let mut joined = String::new();
    for line in &lines {
        joined.push_str(line.text().trim_end());
        if !line.wrapped {
            text.push(std::mem::take(&mut joined));
        }
    }
    for n in 1..=60 {
        let marker = format!("long {n} ");
        let count = text.iter().filter(|line| line.starts_with(&marker)).count();
        assert_eq!(count, 1, "{marker:?} appears {count} times in {text:#?}");
    }
}
