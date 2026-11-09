//! [`AppState`] and [`SessionStore`] against real pipes.
//!
//! The reconnect test runs the `burrowd` and `burrow-fake-agent` that `cargo build --workspace`
//! put next to the test executable's folder, on a pipe and data folder of its own. Without them
//! it says so and passes.

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use burrow_client::{Client, ConnectOptions};
use burrow_protocol::methods::{ProjectAdd, SessionCreate, SessionPrompt};
use burrow_protocol::{
    AgentId, ClientInfo, ClientKind, ContentBlock, Event, ProjectAddParams, SessionCreateParams,
    SessionPromptParams, SessionState,
};
use gpui::{Entity, TestAppContext};

use super::session::SessionStore;
use super::*;
use crate::backend::BackendConfig;

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
