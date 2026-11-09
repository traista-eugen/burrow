//! Live tests against the default `meta` provider. They spend real tokens (Muse's system prompt
//! alone is about 25k input tokens per model call), so they're `#[ignore]`d. Run them by hand:
//!
//! ```powershell
//! cargo test -p burrow-msp --test live_meta -- --ignored --nocapture
//! ```
//!
//! They print what Muse returned, so the account's state can be read off the output.

mod common;

use std::time::Duration;

use burrow_msp::{
    ConnectOptions, Event, FailureClass, ItemDetails, MspClient, MuseCommand, MuseProcess,
    SessionStartParams, TurnStartParams, TurnTerminal,
};
use common::{Recording, Side, Tap, malformed, validate_transcript};

const TURN_TIMEOUT: Duration = Duration::from_secs(240);

struct Live {
    muse: MuseProcess,
    client: MspClient,
    events: burrow_msp::Events,
    sent: Recording,
    received: Recording,
    workspace: tempfile::TempDir,
}

async fn start(command: MuseCommand) -> Live {
    let mut muse = command.no_session_log().spawn().expect("muse serve starts");
    let stdin = muse.child_mut().stdin.take().unwrap();
    let stdout = muse.child_mut().stdout.take().unwrap();
    let (stdin, sent) = Tap::new(stdin);
    let (stdout, received) = Tap::new(stdout);
    let (client, events) = MspClient::connect(stdout, stdin, ConnectOptions::default())
        .await
        .expect("handshake");
    Live {
        muse,
        client,
        events,
        sent,
        received,
        workspace: tempfile::tempdir().unwrap(),
    }
}

impl Live {
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

/// What does a `meta` turn do on this account? On 2026-11-09, with no subscription allowance
/// (`muse usage --json`: `{"allowance":null,"reason":"no_allowance"}`), the turn completed
/// normally and `usage/read` stayed empty. On an exhausted account it should instead fail with
/// a quota ([`FailureClass::Limit`]).
#[tokio::test]
#[ignore = "spends meta tokens; run by hand"]
async fn a_meta_turn_on_this_account() {
    let Some(command) = MuseCommand::locate() else {
        eprintln!("skipping: muse isn't on PATH");
        return;
    };
    let mut live = start(command).await;
    println!(
        "usage/read before: {:?}",
        live.client.read_usage().await.unwrap()
    );
    let session = live
        .client
        .start_session(&SessionStartParams::new(
            live.workspace.path().to_str().unwrap(),
        ))
        .await
        .unwrap();
    println!(
        "session: provider {:?}, model {:?}",
        session.session.provider_id, session.session.model_id
    );
    let mut params = TurnStartParams::text(
        &session.session.session_id,
        "Reply with the single word: ok",
    );
    params.reasoning_effort = Some(burrow_msp::ReasoningEffort::Minimal);
    let turn = live.client.start_turn(&params).await.unwrap();

    let mut seen = Vec::new();
    let done = tokio::time::timeout(TURN_TIMEOUT, async {
        while let Some(event) = live.events.recv().await {
            if let Event::TurnCompleted(done) = &event
                && done.turn_id == turn.turn_id
            {
                return done.clone();
            }
            seen.push(event);
        }
        panic!("the connection closed mid-turn");
    })
    .await
    .expect("the turn ends");
    assert!(malformed(&seen).is_empty(), "{:#?}", malformed(&seen));
    for event in &seen {
        match event {
            Event::ItemCompleted(params) => {
                if let ItemDetails::AgentMessage { text, .. } = params.item.details() {
                    println!("agent: {text}");
                }
            }
            Event::TokenUsage(usage) => println!("tokens: {:?}", usage.usage),
            Event::UsageChanged(usage) => println!("usage/changed: {usage:?}"),
            _ => {}
        }
    }
    println!(
        "turn/completed: terminal {}, error {:?}, failure class {:?}",
        done.terminal,
        done.error,
        done.failure()
    );
    println!(
        "usage/read after: {:?}",
        live.client.read_usage().await.unwrap()
    );
    match done.terminal {
        TurnTerminal::Completed => {}
        _ => assert!(
            matches!(
                done.failure(),
                Some(FailureClass::Limit { .. } | FailureClass::Unavailable)
            ),
            "a failure that isn't a limit or unavailability: {done:?}"
        ),
    }
    live.finish().await;
}

/// A real approval round trip: the model runs a shell command, Muse asks, the test allows it
/// once. The LLM judge is turned off (`MUSE_DISABLE_APPROVAL_JUDGE`) so a person decides. The
/// shell may still fail afterwards, on Windows when Muse's sandbox isn't set up.
#[tokio::test]
#[ignore = "spends meta tokens; run by hand"]
async fn a_meta_approval_round_trip() {
    let Some(command) = MuseCommand::locate() else {
        eprintln!("skipping: muse isn't on PATH");
        return;
    };
    let mut live = start(command.env("MUSE_DISABLE_APPROVAL_JUDGE", "1")).await;
    let session = live
        .client
        .start_session(&SessionStartParams::new(
            live.workspace.path().to_str().unwrap(),
        ))
        .await
        .unwrap();
    let mut params = TurnStartParams::text(
        &session.session.session_id,
        "Use your shell tool to run exactly this command once: echo burrow-live-probe. \
         Then reply with one short sentence and stop.",
    );
    params.reasoning_effort = Some(burrow_msp::ReasoningEffort::Minimal);
    let turn = live.client.start_turn(&params).await.unwrap();

    let mut requests = 0;
    let mut resolved = Vec::new();
    let mut seen = Vec::new();
    let done = tokio::time::timeout(TURN_TIMEOUT, async {
        while let Some(event) = live.events.recv().await {
            match &event {
                Event::ApprovalRequested(request) => {
                    requests += 1;
                    println!(
                        "approval: {} {:?}",
                        request.tool_name, request.subject.command
                    );
                    let choice = request.allow_once().expect("an allowing choice").clone();
                    let decided = live.client.decide_approval(request, &choice).await.unwrap();
                    println!("decided: {decided:?}");
                }
                Event::ApprovalResolved(done) => resolved.push(done.clone()),
                Event::TurnCompleted(done) if done.turn_id == turn.turn_id => return done.clone(),
                _ => {}
            }
            seen.push(event);
        }
        panic!("the connection closed mid-turn");
    })
    .await
    .expect("the turn ends");
    assert!(malformed(&seen).is_empty(), "{:#?}", malformed(&seen));
    println!("turn/completed: {} {:?}", done.terminal, done.error);
    assert!(requests >= 1, "the model never asked to run the command");
    assert!(
        resolved
            .iter()
            .any(|r| r.resolved_by == burrow_msp::ApprovalResolvedBy::User && r.decision.allows())
    );
    live.finish().await;
}
