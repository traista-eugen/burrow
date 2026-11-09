//! Offline tests against transcripts recorded from Muse 1.4.14 (`tests/fixtures/`), so they run
//! without Muse installed:
//! - `echo-turn`, `echo-interrupt`: `--provider echo`, a prompt and an interrupted prompt;
//! - `meta-approval`: one `meta` turn that ran a shell command behind an approval, approved
//!   once, plus a file write. The echo provider can't produce tool calls, so this is the only
//!   approval round trip on record;
//! - `meta-no-allowance`: one `meta` turn on an account without a subscription allowance.
//!
//! Paths and user names in them are replaced (`C:\work\ws`, `C:\Users\dev`).

mod common;

use burrow_msp::{
    ApprovalDecision, ApprovalPolicyResult, ApprovalResolvedBy, ApprovalSubjectKind,
    ConnectOptions, DeltaField, Event, ItemDetails, ItemKind, MessagePhase, MspClient,
    SessionStartParams, TurnTerminal,
};
use common::{Side, WAIT, events_until, fixture, malformed, replay, validate_transcript};
use serde_json::{Value, json};

const FIXTURES: &[&str] = &[
    "echo-turn.jsonl",
    "echo-interrupt.jsonl",
    "meta-approval.jsonl",
    "meta-no-allowance.jsonl",
];

#[test]
fn recorded_transcripts_validate_against_the_committed_schema() {
    for name in FIXTURES {
        let problems = validate_transcript(&fixture(name));
        assert!(problems.is_empty(), "{name}:\n{}", problems.join("\n"));
    }
}

/// The client half of a recorded transcript must be the probe's own handshake, so tests can
/// connect with the same client info.
fn probe_options() -> ConnectOptions {
    ConnectOptions {
        client_info: burrow_msp::ClientInfo {
            name: "burrow_probe".into(),
            title: None,
            version: "0.1.0".into(),
        },
        ..ConnectOptions::default()
    }
}

/// Connects a client to a fake host replaying `name`.
async fn replayed(
    name: &str,
) -> (
    MspClient,
    burrow_msp::Events,
    tokio::task::JoinHandle<()>,
    common::ClientLines,
) {
    let (client_end, host_end) = tokio::io::duplex(1 << 20);
    let (host, seen) = replay(fixture(name), host_end);
    let (reader, writer) = tokio::io::split(client_end);
    let (client, events) = MspClient::connect(reader, writer, probe_options())
        .await
        .expect("handshake with the recording");
    (client, events, host, seen)
}

#[tokio::test]
async fn an_approval_round_trip_against_a_recorded_host() {
    let (client, mut events, host, seen) = replayed("meta-approval.jsonl").await;
    assert_eq!(client.server().server_info.version, "1.4.14");
    assert!(client.schema_matches());

    let session = client
        .start_session(&SessionStartParams::new(r"C:\work\ws"))
        .await
        .unwrap();
    let session_id = session.session.session_id.clone();
    let turn = client
        .prompt(&session_id, "Run a command, then write a file.")
        .await
        .unwrap();

    // The approval arrives once, although Muse sent both `approval/request` and
    // `approval/requested`; the receipt went back without the test doing anything.
    let before = events_until(&mut events, |e| matches!(e, Event::ApprovalRequested(_))).await;
    let Some(Event::ApprovalRequested(request)) = before.last().cloned() else {
        unreachable!()
    };
    assert_eq!(request.session_id, session_id);
    assert_eq!(request.turn_id, turn.turn_id);
    assert_eq!(request.tool_name, "powershell");
    assert_eq!(request.subject.kind, ApprovalSubjectKind::Shell);
    assert_eq!(
        request.subject.command.as_deref(),
        Some("echo burrow-approval-probe")
    );
    assert_eq!(request.subject.stages.len(), 1);
    assert_eq!(
        request.subject.stages[0].argv,
        ["echo", "burrow-approval-probe"]
    );
    let choice = request.allow_once().expect("allow once is offered").clone();
    assert_eq!(choice.choice_id, "allow_once");
    assert_eq!(
        request.reject().map(|c| c.choice_id.as_str()),
        Some("abort")
    );
    let decided = client.decide_approval(&request, &choice).await.unwrap();
    assert!(decided.terminal);

    let after = events_until(
        &mut events,
        |e| matches!(e, Event::TurnCompleted(done) if done.turn_id == turn.turn_id),
    )
    .await;
    let all: Vec<Event> = before.iter().chain(&after).cloned().collect();
    assert!(malformed(&all).is_empty(), "{:#?}", malformed(&all));
    assert_eq!(
        all.iter()
            .filter(|e| matches!(e, Event::ApprovalRequested(_)))
            .count(),
        1,
        "one event per requirement"
    );

    let resolved = after
        .iter()
        .find_map(|e| match e {
            Event::ApprovalResolved(resolved) => Some(resolved),
            _ => None,
        })
        .expect("approval/resolved");
    assert_eq!(resolved.decision, ApprovalDecision::Approved);
    assert_eq!(resolved.policy_result, ApprovalPolicyResult::Allow);
    assert_eq!(resolved.resolved_by, ApprovalResolvedBy::User);
    assert!(after.iter().any(
        |e| matches!(e, Event::ApprovalUpdated(update) if update.change.kind == "stageResolved")
    ));

    // The shell call ran into Muse's unset-up Windows sandbox; the file write worked.
    let tools: Vec<_> = after
        .iter()
        .filter_map(|e| match e {
            Event::ItemCompleted(params) => match params.item.details() {
                ItemDetails::ToolCall(call) => {
                    Some((call.tool.to_owned(), params.item.status.clone()))
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        tools,
        [
            ("powershell".to_owned(), burrow_msp::ItemStatus::Failed),
            ("write_file".to_owned(), burrow_msp::ItemStatus::Completed),
        ]
    );
    let streamed: String = after
        .iter()
        .filter_map(|e| match e {
            Event::ItemDelta {
                delta,
                kind: Some(ItemKind::AgentMessage),
            } if delta.field() == DeltaField::Text => Some(delta.delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(streamed, "Shell probe failed but hello.txt was created.");
    let Some(Event::TurnCompleted(done)) = after.last() else {
        unreachable!()
    };
    assert_eq!(done.terminal, TurnTerminal::Completed);
    assert_eq!(done.failure(), None);

    client.shutdown().await;
    host.await.unwrap();

    // What the client sent is valid MSP: the receipt `{}` and a well-formed decision.
    let sent = seen.lock().unwrap().clone();
    let receipt = sent
        .iter()
        .find(|m| m.get("method").is_none())
        .expect("a receipt");
    assert_eq!(receipt, &json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
    let decide = sent
        .iter()
        .find(|m| m["method"] == "approval/decide")
        .expect("approval/decide");
    assert_eq!(decide["params"]["choiceId"], "allow_once");
    assert_eq!(
        decide["params"]["requirementId"],
        json!({"approvalId": request.approval_id, "sourceIndex": 0})
    );
    let frames: Vec<_> = fixture("meta-approval.jsonl")
        .into_iter()
        .filter(|(side, _)| *side == Side::Host)
        .chain(sent.into_iter().map(|m| (Side::Client, m)))
        .collect();
    let problems = validate_transcript(&frames);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[tokio::test]
async fn a_meta_turn_without_an_allowance_completes_and_reports_no_usage() {
    let (client, mut events, host, _) = replayed("meta-no-allowance.jsonl").await;
    let models = client.list_models(None).await.unwrap();
    assert!(
        models.models.iter().any(|m| m.is_default),
        "a default model"
    );
    assert_eq!(client.read_usage().await.unwrap(), None);
    let session = client
        .start_session(&SessionStartParams::new(r"C:\work\ws"))
        .await
        .unwrap();
    assert_eq!(session.session.provider_id.as_deref(), Some("meta"));
    assert_eq!(client.read_usage().await.unwrap(), None);
    let turn = client
        .prompt(
            &session.session.session_id,
            "Reply with the single word: ok",
        )
        .await
        .unwrap();
    let seen = events_until(
        &mut events,
        |e| matches!(e, Event::TurnCompleted(done) if done.turn_id == turn.turn_id),
    )
    .await;
    assert!(malformed(&seen).is_empty(), "{:#?}", malformed(&seen));
    let Some(Event::TurnCompleted(done)) = seen.last() else {
        unreachable!()
    };
    assert_eq!(done.terminal, TurnTerminal::Completed);
    let answer = seen.iter().find_map(|e| match e {
        Event::ItemCompleted(params) => match params.item.details() {
            ItemDetails::AgentMessage { text, phase } => Some((text.to_owned(), phase.cloned())),
            _ => None,
        },
        _ => None,
    });
    assert_eq!(
        answer,
        Some(("ok".to_owned(), Some(MessagePhase::FinalAnswer)))
    );
    let tokens = seen
        .iter()
        .find_map(|e| match e {
            Event::TokenUsage(usage) => Some(usage.usage.input_tokens),
            _ => None,
        })
        .expect("session/tokenUsage");
    assert!(
        tokens > 20_000,
        "Muse's system prompt alone is about 25k tokens"
    );
    // Still nothing observed after a real turn.
    assert_eq!(client.read_usage().await.unwrap(), None);
    client.shutdown().await;
    host.await.unwrap();
}

#[tokio::test]
async fn deltas_name_the_kind_of_item_they_extend() {
    let (client, mut events, host, _) = replayed("echo-turn.jsonl").await;
    assert_eq!(client.read_usage().await.unwrap(), None);
    let session = client
        .start_session(&SessionStartParams::new(r"C:\work\ws"))
        .await
        .unwrap();
    let turn = client
        .prompt(&session.session.session_id, "Hello there, echo!")
        .await
        .unwrap();
    let seen = events_until(
        &mut events,
        |e| matches!(e, Event::TurnCompleted(done) if done.turn_id == turn.turn_id),
    )
    .await;
    let deltas: Vec<_> = seen
        .iter()
        .filter_map(|e| match e {
            Event::ItemDelta { delta, kind } => Some((kind.clone(), delta.delta.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        deltas,
        [(
            Some(ItemKind::AgentMessage),
            "echo: Hello there, echo!".to_owned()
        )]
    );
    // After the turn: Muse closes the session when its stdin closes.
    client.shutdown().await;
    let rest: Vec<Event> = std::iter::from_fn(|| events.try_recv()).collect();
    assert!(rest.iter().any(|e| matches!(
        e,
        Event::SessionClosed(closed) if closed.reason == burrow_msp::SessionClosedReason::HostShutdown
    )));
    host.await.unwrap();
}

#[tokio::test]
async fn an_unknown_server_request_is_refused_and_unknown_notifications_pass_through() {
    let (client_end, host_end) = tokio::io::duplex(1 << 16);
    let initialize = fixture("echo-turn.jsonl")
        .into_iter()
        .find(|(side, m)| *side == Side::Host && m["id"] == 1)
        .unwrap()
        .1;
    let transcript = vec![
        (
            Side::Client,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        ),
        (Side::Host, initialize),
        (
            Side::Client,
            json!({"jsonrpc": "2.0", "method": "initialized"}),
        ),
        (
            Side::Host,
            json!({"jsonrpc": "2.0", "id": 7, "method": "hologram/request", "params": {}}),
        ),
        (
            Side::Client,
            json!({"jsonrpc": "2.0", "id": 7, "error": {}}),
        ),
        (
            Side::Host,
            json!({"jsonrpc": "2.0", "method": "hologram/changed", "params": {"x": 1}}),
        ),
        (
            Side::Host,
            json!({"jsonrpc": "2.0", "method": "turn/started", "params": {"nonsense": true}}),
        ),
    ];
    let (host, seen) = replay(transcript, host_end);
    let (reader, writer) = tokio::io::split(client_end);
    let (client, mut events) = MspClient::connect(reader, writer, probe_options())
        .await
        .unwrap();
    let first = tokio::time::timeout(WAIT, events.recv()).await.unwrap();
    assert_eq!(
        first,
        Some(Event::Other {
            method: "hologram/changed".into(),
            params: Some(json!({"x": 1})),
        })
    );
    let second = tokio::time::timeout(WAIT, events.recv()).await.unwrap();
    assert!(
        matches!(&second, Some(Event::Malformed { method, .. }) if method == "turn/started"),
        "{second:?}"
    );
    client.shutdown().await;
    host.await.unwrap();
    let refusal = seen.lock().unwrap()[2].clone();
    assert_eq!(refusal["error"]["code"], -32601);
    assert_eq!(
        refusal["error"]["data"]["kind"],
        Value::from("methodNotFound")
    );
}
