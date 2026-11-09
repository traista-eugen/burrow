//! The adapter against Muse transcripts that `burrow-msp` recorded from Muse 1.4.14
//! (`crates/burrow-msp/tests/fixtures/`), replayed by a fake host. They run without Muse and
//! cover what the free echo provider can't: an approval round trip. The end-to-end tests
//! through `burrowd` are in `crates/burrowd/tests/muse.rs`.

use std::path::PathBuf;
use std::time::Duration;

use burrow_msp::{ConnectOptions, MspClient};
use burrow_protocol::{
    ContentBlock, PermissionOptionKind, ToolCallContent, ToolCallStatus, ToolKind, TurnEndReason,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::mpsc;

use super::session;
use crate::adapter::{AgentEvent, PermissionOutcome, SessionConfig, StartedSession};

const WAIT: Duration = Duration::from_secs(20);

const META_APPROVAL: &str = include_str!("../../../burrow-msp/tests/fixtures/meta-approval.jsonl");
const ECHO_INTERRUPT: &str =
    include_str!("../../../burrow-msp/tests/fixtures/echo-interrupt.jsonl");

/// The frames of a recording: `true` for Muse's, `false` for the client's.
fn frames(recording: &str) -> Vec<(bool, Value)> {
    recording
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let entry: Value = serde_json::from_str(line).unwrap();
            (entry["dir"] == "host", entry["msg"].clone())
        })
        .collect()
}

/// What the fake host answers to calls the recording doesn't have.
fn extra_answer(method: &str) -> Option<Value> {
    match method {
        "model/list" => Some(json!({
            "providerId": "meta", "source": "fakeCatalog",
            "models": [{"modelId": "muse-spark-1.3", "displayLabel": "muse-spark-1.3",
                        "providerId": "meta", "isActive": true, "isDefault": false}]
        })),
        "usage/read" => Some(json!({})),
        _ => None,
    }
}

/// Plays Muse's side of a recording on `host`. It writes Muse's frames in order and, for each
/// client frame, waits for the client to send the same method (or the response to the same
/// request). Along the way it answers `model/list` and `usage/read`, which the adapter calls
/// and the recording lacks, renumbers responses to the client's own ids, and replaces the
/// recorded `commandId`s with the client's, since Muse names a fresh turn after its commandId.
async fn replay(recording: &str, host: DuplexStream) {
    let (reader, mut writer) = tokio::io::split(host);
    let mut lines = BufReader::new(reader).lines();
    let mut ids: Vec<(Value, Value)> = Vec::new();
    let mut renames: Vec<(String, String)> = Vec::new();
    let answer = |message: &Value| -> Option<String> {
        let method = message.get("method")?.as_str()?;
        let id = message.get("id")?;
        let result = extra_answer(method)
            .unwrap_or_else(|| panic!("the client sent {message}, which the recording lacks"));
        Some(format!(
            "{}\n",
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        ))
    };
    for (from_host, frame) in frames(recording) {
        if from_host {
            let mut frame = frame;
            if frame.get("method").is_none()
                && let Some((_, actual)) = ids.iter().find(|(recorded, _)| frame["id"] == *recorded)
            {
                frame["id"] = actual.clone();
            }
            let mut line = frame.to_string();
            for (recorded, actual) in &renames {
                line = line.replace(recorded.as_str(), actual);
            }
            line.push('\n');
            writer.write_all(line.as_bytes()).await.unwrap();
            continue;
        }
        loop {
            let line = tokio::time::timeout(WAIT, lines.next_line())
                .await
                .unwrap_or_else(|_| panic!("the client never sent {frame}"))
                .unwrap()
                .unwrap_or_else(|| panic!("the client closed before sending {frame}"));
            let message: Value = serde_json::from_str(&line).unwrap();
            let same = match frame.get("method") {
                Some(method) => message.get("method") == Some(method),
                None => message.get("method").is_none() && message["id"] == frame["id"],
            };
            if !same {
                let reply = answer(&message).unwrap_or_else(|| {
                    panic!("the client sent {message}, the recording has {frame}")
                });
                writer.write_all(reply.as_bytes()).await.unwrap();
                continue;
            }
            if frame.get("method").is_some()
                && let Some(id) = frame.get("id")
            {
                ids.push((id.clone(), message["id"].clone()));
            }
            if let (Some(recorded), Some(actual)) = (
                frame["params"]["commandId"].as_str(),
                message["params"]["commandId"].as_str(),
            ) {
                renames.push((recorded.to_owned(), actual.to_owned()));
            }
            break;
        }
    }
    // Answer whatever else comes until the client closes its side, as Muse does.
    while let Ok(Some(line)) = lines.next_line().await {
        let message: Value = serde_json::from_str(&line).unwrap();
        if let Some(reply) = answer(&message) {
            writer.write_all(reply.as_bytes()).await.unwrap();
        }
    }
}

/// A session on a fake host replaying `recording`.
async fn replayed(recording: &'static str) -> (StartedSession, tokio::task::JoinHandle<()>) {
    let (client_end, host_end) = tokio::io::duplex(1 << 20);
    let host = tokio::spawn(replay(recording, host_end));
    let (reader, writer) = tokio::io::split(client_end);
    let (client, events) = MspClient::connect(reader, writer, ConnectOptions::default())
        .await
        .expect("handshake with the recording");
    let config = SessionConfig {
        session_id: "ses_test".into(),
        cwd: PathBuf::from(r"\\?\C:\work\ws"),
        model: None,
    };
    let started = session::open(client, events, None, &config)
        .await
        .expect("the session starts");
    (started, host)
}

/// Events until (and including) the first one `done` accepts.
async fn until(
    events: &mut mpsc::UnboundedReceiver<AgentEvent>,
    done: impl Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out; got {seen:#?}"))
            .unwrap_or_else(|| panic!("the events ended; got {seen:#?}"));
        let last = done(&event);
        seen.push(event);
        if last {
            return seen;
        }
    }
}

fn message_text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageChunk(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

async fn stop(mut started: StartedSession, host: tokio::task::JoinHandle<()>) {
    started.session.stop().await;
    let rest = until(&mut started.events, |event| {
        matches!(event, AgentEvent::Exited { .. })
    })
    .await;
    assert!(
        matches!(rest.last(), Some(AgentEvent::Exited { error: None })),
        "{rest:#?}"
    );
    tokio::time::timeout(WAIT, host).await.unwrap().unwrap();
}

#[tokio::test]
async fn an_approval_round_trip_against_a_recorded_host() {
    let (mut started, host) = replayed(META_APPROVAL).await;
    assert_eq!(started.model.as_deref(), Some("muse-spark-1.3"));
    assert_eq!(
        started.agent_session_id.as_deref(),
        Some("01a1bf46-b2e6-77e0-aa89-8be35bb80a39")
    );
    assert_eq!(started.models.len(), 1);
    assert_eq!(started.models[0].id, "muse-spark-1.3");

    started
        .session
        .prompt(vec![ContentBlock::text(
            "Run a command, then write a file.",
        )])
        .await
        .unwrap();
    let asked = until(&mut started.events, |event| {
        matches!(event, AgentEvent::Permission(_))
    })
    .await;
    // The empty usage/read first: not a limit.
    let AgentEvent::ProviderUsage(usage) = &asked[0] else {
        panic!("{asked:#?}")
    };
    assert_eq!(usage.provider, "muse");
    assert!(!usage.limited);
    // The approval arrives before the tool call starts, so it announces the call.
    let n = asked.len();
    let AgentEvent::ToolCall(call) = &asked[n - 2] else {
        panic!("{asked:#?}")
    };
    assert_eq!(call.status, ToolCallStatus::Pending);
    assert_eq!(call.kind, ToolKind::Execute);
    assert_eq!(call.title, "Run `echo burrow-approval-probe`");
    let shell = call.tool_call_id.clone();
    // Muse's commentary came before it asked.
    let commentary = message_text(&asked);
    assert_eq!(
        commentary,
        "I'll run that echo command and create the hello file."
    );
    let Some(AgentEvent::Permission(request)) = asked.into_iter().last() else {
        unreachable!()
    };
    assert_eq!(request.tool_call_id, shell);
    assert_eq!(request.title, "Run `echo burrow-approval-probe`");
    let options: Vec<_> = request
        .options
        .iter()
        .map(|option| (option.option_id.as_str(), option.kind.clone()))
        .collect();
    assert_eq!(
        options,
        [
            ("allow_once", PermissionOptionKind::AllowOnce),
            ("allow_local_prefix", PermissionOptionKind::AllowAlways),
            ("abort", PermissionOptionKind::RejectOnce),
        ]
    );
    request
        .reply
        .send(PermissionOutcome::Selected("allow_once".into()));

    let turn = until(&mut started.events, |event| {
        matches!(event, AgentEvent::TurnEnded { .. })
    })
    .await;
    let updates: Vec<_> = turn
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolCallUpdate(update) if update.tool_call_id == shell => {
                update.status.clone()
            }
            _ => None,
        })
        .collect();
    // It ran into Muse's unset-up Windows sandbox when it was recorded.
    assert_eq!(
        updates,
        [ToolCallStatus::InProgress, ToolCallStatus::Failed]
    );
    let write = turn
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall(call) if call.kind == ToolKind::Edit => Some(call),
            _ => None,
        })
        .expect("the write_file call");
    assert_eq!(write.title, r"Write C:\work\ws\hello.txt");
    let written = turn
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCallUpdate(update) if update.tool_call_id == write.tool_call_id => {
                update.content.clone()
            }
            _ => None,
        })
        .expect("the write's completion");
    assert_eq!(
        written[0],
        ToolCallContent::Diff {
            path: r"C:\work\ws\hello.txt".into(),
            old_text: None,
            new_text: "hi\n".into(),
        }
    );
    // The answer is a second message item: a new paragraph.
    assert_eq!(
        message_text(&turn),
        "\n\nShell probe failed but hello.txt was created."
    );
    let n = turn.len();
    assert!(
        matches!(
            &turn[n - 2..],
            [
                AgentEvent::Usage {
                    input_tokens: 77_362,
                    output_tokens: 359,
                    cost_usd: None
                },
                AgentEvent::TurnEnded {
                    reason: TurnEndReason::Completed,
                    message: None
                },
            ]
        ),
        "{turn:#?}"
    );
    stop(started, host).await;
}

#[tokio::test]
async fn cancel_interrupts_the_turn_against_a_recorded_host() {
    let (mut started, host) = replayed(ECHO_INTERRUPT).await;
    assert_eq!(started.model, None, "echo has no model");
    assert!(started.models.is_empty(), "echo can't switch models");
    // The recording sent the interrupt right behind the prompt; so does this.
    let session = &started.session;
    let (prompted, cancelled) = tokio::join!(
        session.prompt(vec![ContentBlock::text("interrupt me")]),
        session.cancel()
    );
    prompted.unwrap();
    cancelled.unwrap();
    let turn = until(&mut started.events, |event| {
        matches!(event, AgentEvent::TurnEnded { .. })
    })
    .await;
    assert_eq!(message_text(&turn), "echo: interrupt me");
    assert!(
        matches!(
            turn.last(),
            Some(AgentEvent::TurnEnded {
                reason: TurnEndReason::Cancelled,
                message: Some(message)
            }) if message == "cancelled during end-of-turn reminder wait"
        ),
        "{turn:#?}"
    );
    stop(started, host).await;
}
