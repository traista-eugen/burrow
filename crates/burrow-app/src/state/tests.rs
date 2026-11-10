//! [`AppState`] and [`SessionStore`] against real pipes.
//!
//! The reconnect and terminal tests run the `burrowd` and `burrow-fake-agent` that
//! `cargo build --workspace` put next to the test executable's folder, on a pipe and data folder
//! of their own. Without them they say so and pass.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::rc::Rc;
use std::time::{Duration, Instant};

use burrow_client::{Client, ConnectOptions};
use burrow_protocol::methods::{
    ProjectAdd, SessionCreate, SessionPrompt, TerminalAttach, TerminalScrollback,
};
use burrow_protocol::{
    AgentId, ClientInfo, ClientKind, ContentBlock, Event, ProjectAddParams, SessionCreateParams,
    SessionMode, SessionPromptParams, SessionState, TerminalIdParams, TerminalScrollbackParams,
    TerminalSize,
};
use gpui::{Entity, TestAppContext};

use super::session::SessionStore;
use super::terminals::{Key, TerminalHub, TerminalMessage};
use super::*;
use crate::backend::BackendConfig;
use crate::views::session::terminal::convert;
use crate::views::terminal::grid::TerminalGrid;

fn unique_pipe(tag: &str) -> String {
    format!(
        "burrow-app-test-{tag}-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    )
}

/// Runs GPUI until `done` holds. Between rounds it sleeps, so the I/O thread makes progress, and
/// moves the test clock on, so backoff timers fire.
fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
        cx.executor().advance_clock(Duration::from_millis(100));
    }
}

fn install_backend(cx: &mut TestAppContext, pipe: &str) {
    let backend = Backend::new(BackendConfig {
        pipe: Some(pipe.to_owned()),
        daemon: None,
    })
    .unwrap();
    cx.update(|cx| cx.set_global(backend));
}

fn status(cx: &mut TestAppContext, state: &Entity<AppState>) -> ConnectionStatus {
    state.read_with(cx, |state, _| state.connection().clone())
}

#[gpui::test]
fn nothing_on_the_pipe_is_not_running(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let pipe = unique_pipe("empty");
    install_backend(cx, &pipe);
    let state = cx.new(AppState::new);
    wait_until(cx, "NotRunning", |cx| {
        matches!(status(cx, &state), ConnectionStatus::NotRunning { .. })
    });
    let ConnectionStatus::NotRunning { detail } = status(cx, &state) else {
        unreachable!()
    };
    assert!(detail.contains(&pipe), "{detail}");
    assert!(!state.read_with(cx, |state, _| state.index().is_loaded()));
}

/// `burrowd` and the fake agent from the workspace build, if they are there.
fn built_burrowd() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // target\debug\deps\burrow-<hash>.exe → target\debug\burrowd.exe
    let dir = exe.parent()?.parent()?;
    let burrowd = dir.join("burrowd.exe");
    let fake = dir.join("burrow-fake-agent.exe");
    (burrowd.is_file() && fake.is_file()).then_some(burrowd)
}

struct Daemon(Child);

impl Daemon {
    fn start(burrowd: &PathBuf, pipe: &str, data: &std::path::Path) -> Self {
        let child = Command::new(burrowd)
            .args(["--dev", "--agents", "fake", "--pipe", pipe, "--data-dir"])
            .arg(data)
            .env("BURROW_LOG", "warn")
            .spawn()
            .expect("start burrowd");
        Self(child)
    }

    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.kill();
    }
}

#[gpui::test]
fn lists_and_streams_survive_a_burrowd_restart(cx: &mut TestAppContext) {
    let Some(burrowd) = built_burrowd() else {
        eprintln!("skipped: no burrowd.exe next to the tests; run `cargo build --workspace` first");
        return;
    };
    cx.executor().allow_parking();
    let pipe = unique_pipe("restart");
    let data = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let mut daemon = Daemon::start(&burrowd, &pipe, data.path());

    install_backend(cx, &pipe);
    let state = cx.new(AppState::new);
    wait_until(cx, "the lists", |cx| {
        status(cx, &state).is_connected() && state.read_with(cx, |s, _| s.index().is_loaded())
    });
    wait_until(cx, "the agents", |cx| {
        state.read_with(cx, |s, _| s.agents().iter().any(|a| a.id == AgentId::Fake))
    });

    // Another client makes changes; the app follows them on the index.
    let runtime = burrow_client::io_runtime().unwrap();
    let other = runtime.block_on(async {
        let info = ClientInfo {
            name: "test".into(),
            version: "0".into(),
            kind: ClientKind::Cli,
        };
        Client::connect(ConnectOptions::new(info).pipe(pipe.clone()))
            .await
            .unwrap()
    });
    let session_id = runtime.block_on(async {
        let project = other
            .call::<ProjectAdd>(&ProjectAddParams {
                path: project_dir.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
        let session = other
            .call::<SessionCreate>(&SessionCreateParams {
                project_id: Some(project.id),
                agent: AgentId::Fake,
                model: None,
                mode: Default::default(),
                agent_mode: None,
                cwd: None,
                title: None,
                cols: None,
                rows: None,
            })
            .await
            .unwrap();
        other
            .call::<SessionPrompt>(&SessionPromptParams {
                session_id: session.id.clone(),
                content: vec![ContentBlock::text("hello there")],
            })
            .await
            .unwrap();
        session.id
    });

    wait_until(cx, "the session on the index", |cx| {
        state.read_with(cx, |s, _| {
            s.index().projects().len() == 1
                && s.index().session(&session_id).is_some_and(|session| {
                    session.state == SessionState::Idle
                        && session.agent == Some(AgentId::Fake)
                        && session.title.as_deref() == Some("hello there")
                })
        })
    });

    // Open the session: its history replays, ending with the turn.
    let store = cx.new(|cx| SessionStore::new(session_id.clone(), cx));
    let has_turn_end = |cx: &mut TestAppContext, store: &Entity<SessionStore>| {
        store.read_with(cx, |store, _| {
            store
                .log()
                .events()
                .iter()
                .any(|event| matches!(event.event, Event::TurnEnded(_)))
        })
    };
    wait_until(cx, "the session's history", |cx| has_turn_end(cx, &store));
    let seq_before = store.read_with(cx, |store, _| store.log().seq());
    let index_seq_before = state.read_with(cx, |s, _| s.index().seq());

    // burrowd dies and comes back with the same data: the app reconnects, resumes both streams
    // after the last seq it had, and sees the restart stop the session.
    drop(other);
    daemon.kill();
    wait_until(cx, "the disconnect", |cx| {
        !status(cx, &state).is_connected()
    });
    let mut daemon = Daemon::start(&burrowd, &pipe, data.path());
    wait_until(cx, "the session stopped after the restart", |cx| {
        status(cx, &state).is_connected()
            && state.read_with(cx, |s, _| {
                s.index()
                    .session(&session_id)
                    .is_some_and(|session| session.state == SessionState::Stopped)
            })
    });
    wait_until(cx, "the store's stopped state", |cx| {
        store.read_with(cx, |store, _| store.log().state() == SessionState::Stopped)
    });
    store.read_with(cx, |store, _| {
        let events = store.log().events();
        // Nothing was replayed twice: seqs strictly increase.
        assert!(events.windows(2).all(|pair| pair[0].seq < pair[1].seq));
        assert!(store.log().seq() > seq_before);
    });
    assert!(state.read_with(cx, |s, _| s.index().seq()) > index_seq_before);
    daemon.kill();
}

/// What a terminal's sink was told, applied to the terminal view's plain model as the session
/// view applies it.
#[derive(Default)]
struct Seen {
    grid: Option<TerminalGrid>,
    snapshots: usize,
    disconnects: usize,
    closed: Option<Option<i32>>,
    failed: Option<String>,
    history_complete: bool,
}

impl Seen {
    fn apply(&mut self, message: TerminalMessage) {
        match message {
            TerminalMessage::Snapshot(snapshot) => {
                let mut grid = TerminalGrid::new(1, 1);
                grid.apply_snapshot(convert::snapshot(snapshot));
                self.grid = Some(grid);
                self.snapshots += 1;
            }
            TerminalMessage::Update(update) => {
                let grid = self.grid.as_mut().expect("an update before the snapshot");
                match convert::reset(&update) {
                    Some(snapshot) => {
                        grid.apply_snapshot(snapshot);
                    }
                    None => {
                        grid.apply_update(convert::update(update));
                    }
                }
            }
            TerminalMessage::History { lines, complete } => {
                let grid = self.grid.as_mut().expect("history before the snapshot");
                grid.apply_history(lines.into_iter().map(convert::line).collect(), complete);
                self.history_complete = complete;
            }
            TerminalMessage::Closed { exit_code } => self.closed = Some(exit_code),
            TerminalMessage::Disconnected => self.disconnects += 1,
            TerminalMessage::Failed(message) => self.failed = Some(message),
        }
    }

    fn screen(&self) -> Vec<String> {
        let Some(grid) = &self.grid else {
            return Vec::new();
        };
        (0..i64::from(grid.rows()))
            .map(|row| grid.line(row).map(|line| line.text()).unwrap_or_default())
            .collect()
    }

    fn size(&self) -> Option<(u16, u16)> {
        self.grid.as_ref().map(|grid| (grid.cols(), grid.rows()))
    }

    /// The fake TUI's status line, on the last row.
    fn status(&self) -> String {
        self.screen().last().cloned().unwrap_or_default()
    }
}

/// Attaches `terminal` through the hub, applying what arrives to a [`Seen`].
fn attach_seen(
    cx: &mut TestAppContext,
    hub: &Entity<TerminalHub>,
    terminal: &str,
) -> (Rc<RefCell<Seen>>, Key) {
    let seen = Rc::new(RefCell::new(Seen::default()));
    let sink = seen.clone();
    let key = hub.update(cx, |hub, _| {
        hub.attach(terminal, move |message, _| sink.borrow_mut().apply(message))
    });
    (seen, key)
}

/// M4's "done when", through the app's own path: a terminal session started at a size, typed
/// into, paged back, watched by a second client, detached and attached again to the same screen,
/// resized, reconnected, and ended.
#[gpui::test]
fn a_terminal_session_runs_through_the_hub(cx: &mut TestAppContext) {
    let Some(burrowd) = built_burrowd() else {
        eprintln!("skipped: no burrowd.exe next to the tests; run `cargo build --workspace` first");
        return;
    };
    cx.executor().allow_parking();
    let pipe = unique_pipe("terminal");
    let data = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _daemon = Daemon::start(&burrowd, &pipe, data.path());

    install_backend(cx, &pipe);
    let state = cx.new(AppState::new);
    wait_until(cx, "the connection", |cx| status(cx, &state).is_connected());
    let hub = state.read_with(cx, |state, _| state.terminals().clone());

    // A terminal session, at the size a client asks for.
    let runtime = burrow_client::io_runtime().unwrap();
    let other = runtime.block_on(async {
        let info = ClientInfo {
            name: "test".into(),
            version: "0".into(),
            kind: ClientKind::Cli,
        };
        Client::connect(ConnectOptions::new(info).pipe(pipe.clone()))
            .await
            .unwrap()
    });
    let session = runtime
        .block_on(other.call::<SessionCreate>(&SessionCreateParams {
            mode: SessionMode::Terminal,
            cols: Some(100),
            rows: Some(30),
            ..SessionCreateParams::new(AgentId::Fake, dir.path().to_string_lossy())
        }))
        .unwrap();
    let terminal = session.terminal_id.clone().expect("Session.terminalId");

    // Attach: the first screen, at that size.
    let (seen, key) = attach_seen(cx, &hub, &terminal);
    wait_until(cx, "the first screen", |_| {
        seen.borrow().status().starts_with("status 100x30")
    });
    assert_eq!(seen.borrow().size(), Some((100, 30)));
    assert!(seen.borrow().screen()[0].starts_with("fake tui"));
    assert!(hub.read_with(cx, |hub, _| hub.is_attached(&terminal, key)));

    // Type, and see the screen change.
    let input = |cx: &mut TestAppContext, data: &str| {
        hub.read_with(cx, |hub, cx| hub.input(&terminal, data.into(), cx))
    };
    input(cx, "hello");
    wait_until(cx, "the echo", |_| seen.borrow().screen()[1] == "> hello");
    input(cx, "\r/flood 60\r");
    wait_until(cx, "the flood", |_| {
        seen.borrow().screen().iter().any(|row| row == "line 60")
    });
    // Page the scrollback in: it is what scrolled off, as burrowd keeps it.
    assert!(hub.update(cx, |hub, _| hub.scrollback(&terminal, key, 0, 1000)));
    wait_until(cx, "the scrollback", |_| seen.borrow().history_complete);
    let kept: Vec<String> = runtime
        .block_on(other.call::<TerminalScrollback>(&TerminalScrollbackParams {
            terminal_id: terminal.clone(),
            before: None,
            limit: Some(1000),
        }))
        .unwrap()
        .lines
        .iter()
        .map(|line| line.text())
        .collect();
    {
        let seen = seen.borrow();
        let grid = seen.grid.as_ref().unwrap();
        let history: Vec<String> = (1..=grid.history_len() as i64)
            .rev()
            .map(|n| grid.line(-n).unwrap().text())
            .collect();
        assert_eq!(history, kept);
        for line in ["fake tui red orange inverse 漢字", "echo: hello", "line 1"] {
            assert!(history.contains(&line.to_owned()), "{line}: {history:?}");
        }
    }

    // A second client sees the same screen.
    let theirs = runtime
        .block_on(other.call::<TerminalAttach>(&TerminalIdParams {
            terminal_id: terminal.clone(),
        }))
        .unwrap();
    let mut grid = TerminalGrid::new(1, 1);
    grid.apply_snapshot(convert::snapshot(theirs));
    let their_screen: Vec<String> = (0..30).map(|row| grid.line(row).unwrap().text()).collect();
    assert_eq!(their_screen, seen.borrow().screen());

    // Detach and attach again: exactly the same screen.
    let before = seen.borrow().screen();
    hub.update(cx, |hub, _| hub.detach(&terminal, key));
    let (seen, key) = attach_seen(cx, &hub, &terminal);
    wait_until(cx, "the snapshot after attaching again", |_| {
        seen.borrow().snapshots == 1
    });
    assert_eq!(seen.borrow().screen(), before);

    // Resize: the program redraws at the new size.
    hub.read_with(cx, |hub, cx| {
        hub.resize(&terminal, TerminalSize { cols: 80, rows: 24 }, cx)
    });
    wait_until(cx, "the resize", |_| {
        let seen = seen.borrow();
        seen.size() == Some((80, 24)) && seen.status().starts_with("status 80x24")
    });

    // The connection drops while burrowd runs on: the hub attaches again, with a fresh snapshot.
    let before = seen.borrow().screen();
    cx.update(|cx| Backend::global(cx).client().unwrap().close());
    wait_until(cx, "the attachment after the reconnect", |cx| {
        let seen = seen.borrow();
        seen.disconnects == 1
            && seen.snapshots == 2
            && hub.read_with(cx, |hub, _| hub.is_attached(&terminal, key))
    });
    assert_eq!(seen.borrow().screen(), before);
    // (The status line now covers the prompt's row, so the answer shows that typing arrived.)
    input(cx, "after\r");
    wait_until(cx, "typing after the reconnect", |_| {
        seen.borrow()
            .screen()
            .iter()
            .any(|row| row == "echo: after")
    });

    // The program exits: its code arrives, and the last screen stays.
    input(cx, "/exit 3\r");
    wait_until(cx, "the exit", |_| seen.borrow().closed.is_some());
    assert_eq!(seen.borrow().closed, Some(Some(3)));
    assert!(
        seen.borrow()
            .screen()
            .iter()
            .any(|row| row.contains("echo: after"))
    );
    assert!(seen.borrow().failed.is_none());
}
