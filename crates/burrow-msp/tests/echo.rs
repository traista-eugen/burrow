//! Integration tests against a real `muse serve --provider echo`. Echo is free: it spends no
//! tokens and needs no login. It replies `echo: <prompt>`, can't call tools, and so never asks
//! for approvals; the approval round trip is in `tests/fixtures.rs`.
//!
//! Each test skips (passing) when Muse isn't on `PATH`. Every frame on the wire, both ways,
//! is validated against the committed schema.

mod common;

use std::time::Duration;

use burrow_msp::{
    ApprovalMode, ConnectOptions, DeltaField, ErrorKind, Event, Events, ItemDetails, ItemKind,
    MessagePhase, MspClient, MspError, MuseCommand, MuseProcess, SessionClosedReason,
    SessionStartParams, TurnStartParams, TurnTerminal,
};
use common::{Recording, Side, Tap, WAIT, events_until, malformed, validate_transcript};

/// A running echo host, with the wire recorded.
struct Echo {
    muse: MuseProcess,
    client: MspClient,
    events: Events,
    sent: Recording,
    received: Recording,
    workspace: tempfile::TempDir,
}

impl Echo {
    async fn start() -> Option<Self> {
        let Some(command) = MuseCommand::locate() else {
            eprintln!("skipping: muse isn't on PATH");
            return None;
        };
        let mut muse = command
            .provider("echo")
            .no_session_log()
            .spawn()
            .expect("muse serve starts");
        let stdin = muse.child_mut().stdin.take().unwrap();
        let stdout = muse.child_mut().stdout.take().unwrap();
        let (stdin, sent) = Tap::new(stdin);
        let (stdout, received) = Tap::new(stdout);
        let (client, events) = MspClient::connect(stdout, stdin, ConnectOptions::default())
            .await
            .expect("handshake");
        Some(Self {
            muse,
            client,
            events,
            sent,
            received,
            workspace: tempfile::tempdir().unwrap(),
        })
    }

    async fn session(&self) -> String {
        let workspace = self.workspace.path().to_str().unwrap();
        let started = self
            .client
            .start_session(&SessionStartParams::new(workspace))
            .await
            .expect("session/start");
        assert_eq!(started.session.provider_id.as_deref(), Some("echo"));
        started.session.session_id
    }

    async fn until_turn_completes(&mut self, turn_id: &str) -> Vec<Event> {
        let turn_id = turn_id.to_owned();
        let events = events_until(
            &mut self.events,
            |e| matches!(e, Event::TurnCompleted(done) if done.turn_id == turn_id),
        )
        .await;
        assert!(malformed(&events).is_empty(), "{:#?}", malformed(&events));
        events
    }

    /// Shuts down cleanly and checks the whole wire against the schema.
    async fn finish(self) {
        let status = self
            .muse
            .shutdown(&self.client, Duration::from_secs(15))
            .await
            .unwrap();
        assert!(status.success(), "muse exited with {status}");
        let mut frames = self.sent.frames(Side::Client);
        frames.extend(self.received.frames(Side::Host));
        let problems = validate_transcript(&frames);
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }
}

fn last_turn_completed(events: &[Event]) -> &burrow_msp::TurnCompletedParams {
    match events.last() {
        Some(Event::TurnCompleted(done)) => done,
        other => panic!("expected turn/completed, got {other:?}"),
    }
}

#[tokio::test]
async fn the_handshake_reports_muse_and_its_schema() {
    let Some(echo) = Echo::start().await else {
        return;
    };
    let server = echo.client.server();
    assert_eq!(server.server_info.name, "muse");
    assert_eq!(server.schema.version, 1);
    if !echo.client.schema_matches() {
        eprintln!(
            "note: muse {} has schema {}, burrow-msp was written against {} ({}); \
             regenerate crates/burrow-msp/schema if tests fail",
            server.server_info.version,
            server.schema.fingerprint,
            burrow_msp::SCHEMA_FINGERPRINT,
            burrow_msp::SCHEMA_MUSE_VERSION,
        );
    }
    echo.finish().await;
}

#[tokio::test]
async fn a_prompt_streams_its_echo_back() {
    let Some(mut echo) = Echo::start().await else {
        return;
    };
    let session_id = echo.session().await;
    let prompt = "Hello from Burrow";
    let turn = echo.client.prompt(&session_id, prompt).await.unwrap();
    assert!(turn.started_new_turn);
    let events = echo.until_turn_completes(&turn.turn_id).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::TurnStarted(started) if started.turn_id == turn.turn_id))
    );
    let user = events.iter().find_map(|e| match e {
        Event::ItemCompleted(params) => match params.item.details() {
            ItemDetails::UserMessage { text } => Some(text.to_owned()),
            _ => None,
        },
        _ => None,
    });
    assert_eq!(user.as_deref(), Some(prompt));

    let expected = format!("echo: {prompt}");
    let streamed: String = events
        .iter()
        .filter_map(|e| match e {
            Event::ItemDelta {
                delta,
                kind: Some(ItemKind::AgentMessage),
            } if delta.field() == DeltaField::Text => Some(delta.delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(streamed, expected);
    let reply = events.iter().find_map(|e| match e {
        Event::ItemCompleted(params) => match params.item.details() {
            ItemDetails::AgentMessage { text, phase } => Some((text.to_owned(), phase.cloned())),
            _ => None,
        },
        _ => None,
    });
    assert_eq!(reply, Some((expected, Some(MessagePhase::FinalAnswer))));
    assert!(events.iter().any(|e| matches!(e, Event::TokenUsage(_))));

    let done = last_turn_completed(&events);
    assert_eq!(done.terminal, TurnTerminal::Completed);
    assert_eq!(done.failure(), None);
    echo.finish().await;
}

#[tokio::test]
async fn usage_read_reports_nothing_observed() {
    let Some(echo) = Echo::start().await else {
        return;
    };
    assert_eq!(echo.client.read_usage().await.unwrap(), None);
    let session_id = echo.session().await;
    assert_eq!(echo.client.read_usage().await.unwrap(), None);
    let pending = echo.client.list_pending(&session_id).await.unwrap();
    assert!(pending.approvals.is_empty() && pending.user_inputs.is_empty());
    echo.finish().await;
}

/// Echo answers in milliseconds, but Muse's end-of-turn reminder agents hold every turn open a
/// little longer. An interrupt sent in the same write as `turn/start` lands in that window.
#[tokio::test]
async fn an_interrupt_cancels_the_running_turn() {
    let Some(mut echo) = Echo::start().await else {
        return;
    };
    let session_id = echo.session().await;
    let params = TurnStartParams::text(&session_id, "interrupt me");
    let (started, interrupted) = tokio::join!(
        echo.client.start_turn(&params),
        echo.client.interrupt_turn(&session_id, None),
    );
    let started = started.unwrap();
    assert_eq!(interrupted.unwrap().turn_id, started.turn_id);
    let events = echo.until_turn_completes(&started.turn_id).await;
    let done = last_turn_completed(&events);
    assert_eq!(done.terminal, TurnTerminal::Cancelled, "{done:?}");
    assert_eq!(done.failure(), None);
    echo.finish().await;
}

#[tokio::test]
async fn a_cancel_cancels_the_running_turn() {
    let Some(mut echo) = Echo::start().await else {
        return;
    };
    let session_id = echo.session().await;
    let params = TurnStartParams::text(&session_id, "cancel me");
    let (started, cancelled) = tokio::join!(
        echo.client.start_turn(&params),
        echo.client.cancel_turn(&session_id, None),
    );
    let started = started.unwrap();
    assert_eq!(cancelled.unwrap().turn_id, started.turn_id);
    let events = echo.until_turn_completes(&started.turn_id).await;
    assert_eq!(
        last_turn_completed(&events).terminal,
        TurnTerminal::Cancelled
    );
    echo.finish().await;
}

#[tokio::test]
async fn interrupting_an_idle_session_is_refused() {
    let Some(echo) = Echo::start().await else {
        return;
    };
    let session_id = echo.session().await;
    let error = echo
        .client
        .interrupt_turn(&session_id, None)
        .await
        .unwrap_err();
    let rpc = error.rpc().expect("a JSON-RPC error");
    assert_eq!(rpc.kind(), ErrorKind::CommandRejected);
    assert_eq!(rpc.reason(), Some("missing_run"));
    echo.finish().await;
}

#[tokio::test]
async fn models_list_and_echo_refuses_a_switch() {
    let Some(echo) = Echo::start().await else {
        return;
    };
    let session_id = echo.session().await;
    let catalog = echo.client.list_models(Some(&session_id)).await.unwrap();
    // The meta catalog, possibly still loading; echo itself has no models.
    assert_eq!(catalog.provider_id, "meta");
    assert!(catalog.models.iter().all(|model| !model.is_active));
    let error = echo
        .client
        .set_model(&session_id, "muse-spark-1.3")
        .await
        .unwrap_err();
    assert_eq!(error.kind(), Some(ErrorKind::CommandRejected));
    assert_eq!(error.rpc().unwrap().reason(), Some("invalid_target"));
    echo.finish().await;
}

#[tokio::test]
async fn the_approval_mode_can_be_selected() {
    let Some(echo) = Echo::start().await else {
        return;
    };
    let session_id = echo.session().await;
    let result = echo
        .client
        .set_approval_mode(&session_id, ApprovalMode::DenyUnmatched)
        .await
        .unwrap();
    assert_eq!(result.effective_mode.mode, ApprovalMode::DenyUnmatched);
    echo.finish().await;
}

#[tokio::test]
async fn shutdown_closes_the_session_and_muse_exits_cleanly() {
    let Some(command) = MuseCommand::locate() else {
        eprintln!("skipping: muse isn't on PATH");
        return;
    };
    let mut muse = command.provider("echo").no_session_log().spawn().unwrap();
    let (client, mut events) = muse.connect(ConnectOptions::default()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = client
        .start_session(&SessionStartParams::new(workspace.path().to_str().unwrap()))
        .await
        .unwrap();
    let status = muse
        .shutdown(&client, Duration::from_secs(15))
        .await
        .unwrap();
    assert!(status.success(), "muse exited with {status}");
    assert!(client.is_closed());
    assert!(matches!(client.read_usage().await, Err(MspError::Closed)));

    let mut rest = Vec::new();
    while let Some(event) = tokio::time::timeout(WAIT, events.recv()).await.unwrap() {
        rest.push(event);
    }
    assert!(rest.iter().any(|e| matches!(
        e,
        Event::SessionClosed(closed)
            if closed.session_id == session.session.session_id
                && closed.reason == SessionClosedReason::HostShutdown
    )));
}

/// The fallback when the launcher's layout isn't there: `muse.cmd` itself, through PowerShell.
/// Slower to start, but the same protocol on the same pipes.
#[cfg(windows)]
#[tokio::test]
async fn the_launcher_works_too() {
    let Some(launcher) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("muse.cmd"))
        .find(|path| path.is_file())
    else {
        eprintln!("skipping: no muse.cmd on PATH");
        return;
    };
    let mut muse = MuseCommand::new(&launcher)
        .provider("echo")
        .no_session_log()
        .spawn()
        .unwrap();
    let (client, mut events) = muse.connect(ConnectOptions::default()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = client
        .start_session(&SessionStartParams::new(workspace.path().to_str().unwrap()))
        .await
        .unwrap();
    let turn = client
        .prompt(&session.session.session_id, "through the launcher")
        .await
        .unwrap();
    let seen = events_until(
        &mut events,
        |e| matches!(e, Event::TurnCompleted(done) if done.turn_id == turn.turn_id),
    )
    .await;
    assert_eq!(last_turn_completed(&seen).terminal, TurnTerminal::Completed);
    let status = muse
        .shutdown(&client, Duration::from_secs(15))
        .await
        .unwrap();
    assert!(status.success(), "the launcher exited with {status}");
}

/// `--provider local` needs macOS; elsewhere `muse serve` exits before answering `initialize`.
/// The client reports the exit and Muse's own reason, not a bare "connection closed".
#[cfg(windows)]
#[tokio::test]
async fn a_provider_muse_cannot_build_ends_the_process_with_its_reason() {
    let Some(command) = MuseCommand::locate() else {
        eprintln!("skipping: muse isn't on PATH");
        return;
    };
    let mut muse = command
        .provider("local")
        .no_session_log()
        .env("MUSE_EXPERIMENTAL_LOCAL_MODELS", "1")
        .spawn()
        .unwrap();
    match muse.connect(ConnectOptions::default()).await {
        Err(MspError::Exited { status, stderr }) => {
            assert!(!status.success());
            assert!(stderr.contains("Local models"), "{stderr}");
        }
        other => panic!("expected the process to exit, got {other:?}"),
    }
}
