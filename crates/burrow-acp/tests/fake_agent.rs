//! `burrow-acp` against an in-process fake agent, built with the
//! `agent-client-protocol` SDK's agent side and connected through an
//! in-memory pipe. The fake agent's behaviour is chosen by the prompt text.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::v1::{ConnectionTo, Responder};
use agent_client_protocol::{Agent, ByteStreams, Client, UntypedMessage};
use burrow_acp::{
    AcpClient, AcpError, AcpEvent, AcpEvents, ConnectOptions, Content, LimitInfo, LimitSource,
    PermissionDecision, PermissionKind, SessionEvent, StopReason, ToolCallStatus, ToolKind,
    TurnEnd,
};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

const SESSION: &str = "fake-session-1";
const TIMEOUT: Duration = Duration::from_secs(10);

/// What the fake agent has seen, for assertions.
#[derive(Default)]
struct Seen {
    cancels: Vec<String>,
    config: Vec<(String, String)>,
    legacy_models: Vec<String>,
}

#[derive(Clone, Default)]
struct FakeAgent {
    seen: Arc<Mutex<Seen>>,
    cancelled: Arc<Notify>,
}

impl FakeAgent {
    async fn run(
        self,
        transport: ByteStreams<
            impl futures::AsyncWrite + Send + 'static,
            impl futures::AsyncRead + Send + 'static,
        >,
    ) {
        let requests = self.clone();
        let notifications = self.clone();
        let result = Agent
            .v1()
            .name("fake-agent")
            .on_receive_request(
                async move |message: UntypedMessage, responder: Responder<Value>, cx| {
                    requests.on_request(message, responder, cx)
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                async move |message: UntypedMessage, _cx| {
                    if message.method() == "session/cancel" {
                        let session = message.params()["sessionId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned();
                        notifications.seen.lock().unwrap().cancels.push(session);
                        notifications.cancelled.notify_one();
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .connect_to(transport)
            .await;
        if let Err(error) = result {
            eprintln!("fake agent ended with {error}");
        }
    }

    fn on_request(
        &self,
        message: UntypedMessage,
        responder: Responder<Value>,
        cx: ConnectionTo<Client>,
    ) -> Result<(), agent_client_protocol::Error> {
        let (method, params) = message.into_parts();
        match method.as_str() {
            "initialize" => responder.respond(json!({
                "protocolVersion": 1,
                "agentInfo": {"name": "fake-agent", "title": "Fake", "version": "0.0.1"},
                "agentCapabilities": {
                    "loadSession": true,
                    "promptCapabilities": {"embeddedContext": true},
                    "sessionCapabilities": {"close": {}}
                },
                "authMethods": [{"id": "fake-login", "name": "Fake login"}]
            })),
            "session/new" => {
                let cwd = params["cwd"].as_str().unwrap_or_default();
                if cwd.ends_with("needs-login") {
                    return responder
                        .respond_with_error(agent_client_protocol::Error::auth_required());
                }
                if cwd.ends_with("legacy-models") {
                    return responder.respond(json!({
                        "sessionId": SESSION,
                        "models": {
                            "currentModelId": "old-a",
                            "availableModels": [{"modelId": "old-a", "name": "A"}, {"modelId": "old-b", "name": "B"}]
                        }
                    }));
                }
                responder.respond(json!({
                    "sessionId": SESSION,
                    "modes": {
                        "currentModeId": "code",
                        "availableModes": [{"id": "code", "name": "Code"}, {"id": "plan", "name": "Plan"}]
                    },
                    "configOptions": [model_option("fast")]
                }))
            }
            "session/load" => {
                update(
                    &cx,
                    &params["sessionId"],
                    json!({
                        "sessionUpdate": "user_message_chunk",
                        "content": {"type": "text", "text": "earlier question"}
                    }),
                )?;
                update(
                    &cx,
                    &params["sessionId"],
                    json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "earlier answer"}
                    }),
                )?;
                responder.respond(json!({"configOptions": [model_option("smart")]}))
            }
            "session/set_config_option" => {
                let value = params["value"].as_str().unwrap_or_default().to_owned();
                self.seen.lock().unwrap().config.push((
                    params["configId"].as_str().unwrap_or_default().to_owned(),
                    value.clone(),
                ));
                responder.respond(json!({"configOptions": [model_option(&value)]}))
            }
            "session/set_model" => {
                self.seen
                    .lock()
                    .unwrap()
                    .legacy_models
                    .push(params["modelId"].as_str().unwrap_or_default().to_owned());
                responder.respond(json!({}))
            }
            "session/set_mode" => responder.respond(json!({})),
            "session/close" => responder.respond(json!({})),
            "session/prompt" => self.on_prompt(params, responder, cx),
            _ => responder.respond_with_error(agent_client_protocol::Error::method_not_found()),
        }
    }

    fn on_prompt(
        &self,
        params: Value,
        responder: Responder<Value>,
        cx: ConnectionTo<Client>,
    ) -> Result<(), agent_client_protocol::Error> {
        let session = params["sessionId"].clone();
        let text = params["prompt"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        match text.as_str() {
            "stream" => {
                chunk(&cx, &session, "agent_thought_chunk", "Thinking.")?;
                chunk(&cx, &session, "agent_message_chunk", "Hello")?;
                chunk(&cx, &session, "agent_message_chunk", ", world")?;
                update(
                    &cx,
                    &session,
                    json!({
                        "sessionUpdate": "tool_call",
                        "toolCallId": "t1",
                        "title": "Read README.md",
                        "kind": "read",
                        "status": "in_progress",
                        "locations": [{"path": "C:/repo/README.md", "line": 3}]
                    }),
                )?;
                update(
                    &cx,
                    &session,
                    json!({
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": "t1",
                        "status": "completed",
                        "content": [{"type": "content", "content": {"type": "text", "text": "# Burrow"}}]
                    }),
                )?;
                update(
                    &cx,
                    &session,
                    json!({
                        "sessionUpdate": "plan",
                        "entries": [{"content": "Say hello", "priority": "high", "status": "completed"}]
                    }),
                )?;
                update(
                    &cx,
                    &session,
                    json!({
                        "sessionUpdate": "usage_update", "used": 1200, "size": 200000,
                        "cost": {"amount": 0.01, "currency": "USD"},
                        "_meta": {"_claude/rateLimit": {
                            "status": "allowed_warning", "resetsAt": 1_793_000_000,
                            "rateLimitType": "five_hour", "utilization": 0.9
                        }}
                    }),
                )?;
                update(
                    &cx,
                    &session,
                    json!({"sessionUpdate": "current_mode_update", "currentModeId": "plan"}),
                )?;
                update(
                    &cx,
                    &session,
                    json!({"sessionUpdate": "some_future_update", "x": 1}),
                )?;
                responder.respond(json!({
                    "stopReason": "end_turn",
                    "usage": {"totalTokens": 30, "inputTokens": 20, "outputTokens": 10}
                }))
            }
            "permission" | "cancel-during-permission" => {
                let cancelling = text == "cancel-during-permission";
                let cancelled = self.cancelled.clone();
                update(
                    &cx,
                    &session,
                    json!({
                        "sessionUpdate": "tool_call",
                        "toolCallId": "t2",
                        "title": "Write hello.txt",
                        "kind": "edit",
                        "status": "pending"
                    }),
                )?;
                // A handler must not wait for the client, so the rest of the
                // turn runs in a task.
                tokio::spawn(async move {
                    let request = UntypedMessage::new("session/request_permission", json!({
                        "sessionId": session,
                        "toolCall": {"toolCallId": "t2", "title": "Write hello.txt", "kind": "edit"},
                        "options": [
                            {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
                            {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
                            {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
                        ]
                    }))
                    .unwrap();
                    let answer = cx.send_request(request).block_task().await;
                    let outcome = match &answer {
                        Ok(answer) => match answer["outcome"]["outcome"].as_str() {
                            Some("selected") => format!(
                                "selected {}",
                                answer["outcome"]["optionId"].as_str().unwrap_or("?")
                            ),
                            Some(other) => other.to_owned(),
                            None => format!("odd answer {answer}"),
                        },
                        Err(error) => format!("error {error}"),
                    };
                    if cancelling {
                        cancelled.notified().await;
                        let _ = chunk(&cx, &session, "agent_message_chunk", &outcome);
                        let _ = responder.respond(json!({"stopReason": "cancelled"}));
                        return;
                    }
                    let _ = update(
                        &cx,
                        &session,
                        json!({
                            "sessionUpdate": "tool_call_update",
                            "toolCallId": "t2",
                            "status": if outcome == "selected allow" { "completed" } else { "failed" }
                        }),
                    );
                    let _ = chunk(&cx, &session, "agent_message_chunk", &outcome);
                    let _ = responder.respond(json!({"stopReason": "end_turn"}));
                });
                Ok(())
            }
            "wait-for-cancel" => {
                let cancelled = self.cancelled.clone();
                chunk(&cx, &session, "agent_message_chunk", "Working")?;
                tokio::spawn(async move {
                    cancelled.notified().await;
                    let _ = responder.respond(json!({"stopReason": "cancelled"}));
                });
                Ok(())
            }
            "hang" => {
                // Never answers; the test kills the agent.
                std::mem::forget(responder);
                Ok(())
            }
            "error" => responder.respond_with_error(
                agent_client_protocol::Error::internal_error().data(json!({"details": "boom"})),
            ),
            // The shapes below are what each adapter sends, from its source
            // (see crate::limit).
            "claude-limit" => {
                // A rate-limit status the adapter sometimes sends first.
                update(
                    &cx,
                    &session,
                    json!({
                        "sessionUpdate": "usage_update", "used": 2, "size": 200000,
                        "_meta": {"_claude/rateLimit": {
                            "status": "rejected", "resetsAt": 1_793_000_000, "rateLimitType": "five_hour"
                        }}
                    }),
                )?;
                chunk(&cx, &session, "agent_message_chunk", CLAUDE_NOTICE)?;
                responder.respond_with_error(
                    agent_client_protocol::Error::new(
                        -32603,
                        format!("Internal error: {CLAUDE_NOTICE}"),
                    )
                    .data(json!({"errorKind": "rate_limit"})),
                )
            }
            "claude-busy" => responder.respond_with_error(
                agent_client_protocol::Error::new(
                    -32603,
                    "Internal error: API Error: Server is temporarily limiting requests (not your usage limit) · Rate limited",
                )
                .data(json!({"errorKind": "rate_limit"})),
            ),
            "codex-limit" => {
                chunk(
                    &cx,
                    &session,
                    "agent_message_chunk",
                    &format!("{CODEX_NOTICE}\n\n"),
                )?;
                responder.respond_with_error(
                    agent_client_protocol::Error::new(-32603, "Internal error").data(json!({
                        "message": CODEX_NOTICE, "codexErrorInfo": "usageLimitExceeded"
                    })),
                )
            }
            "codex-limit-then-hang" => {
                // codex-acp#747: the notice, then the turn never ends, not
                // even when cancelled.
                chunk(
                    &cx,
                    &session,
                    "agent_message_chunk",
                    &format!("{CODEX_NOTICE}\n\n"),
                )?;
                std::mem::forget(responder);
                Ok(())
            }
            "cursor-limit" => {
                chunk(
                    &cx,
                    &session,
                    "agent_message_chunk",
                    "\n\nUpgrade your plan to continue",
                )?;
                responder.respond(json!({"stopReason": "end_turn"}))
            }
            "notice-then-more-work" => {
                chunk(
                    &cx,
                    &session,
                    "agent_message_chunk",
                    "You've hit your limit · resets 3pm",
                )?;
                update(
                    &cx,
                    &session,
                    json!({"sessionUpdate": "tool_call", "toolCallId": "t9", "title": "ls", "kind": "execute"}),
                )?;
                chunk(&cx, &session, "agent_message_chunk", "Done.")?;
                responder.respond(json!({"stopReason": "end_turn"}))
            }
            "ask-for-a-file" => {
                tokio::spawn(async move {
                    let request = UntypedMessage::new(
                        "fs/read_text_file",
                        json!({"sessionId": session, "path": "C:/secret.txt"}),
                    )
                    .unwrap();
                    let answer = cx.send_request(request).block_task().await;
                    let text = match answer {
                        Ok(_) => "the client read the file".to_owned(),
                        Err(error) => format!("refused with {}", error.code.value()),
                    };
                    let _ = chunk(&cx, &session, "agent_message_chunk", &text);
                    let _ = responder.respond(json!({"stopReason": "end_turn"}));
                });
                Ok(())
            }
            other => {
                chunk(
                    &cx,
                    &session,
                    "agent_message_chunk",
                    &format!("echo: {other}"),
                )?;
                responder.respond(json!({"stopReason": "end_turn"}))
            }
        }
    }
}

fn model_option(current: &str) -> Value {
    json!({
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": current,
        "options": [{"value": "fast", "name": "Fast"}, {"value": "smart", "name": "Smart"}]
    })
}

fn update(
    cx: &ConnectionTo<Client>,
    session: &Value,
    update: Value,
) -> Result<(), agent_client_protocol::Error> {
    cx.send_notification(UntypedMessage::new(
        "session/update",
        json!({"sessionId": session, "update": update}),
    )?)
}

fn chunk(
    cx: &ConnectionTo<Client>,
    session: &Value,
    kind: &str,
    text: &str,
) -> Result<(), agent_client_protocol::Error> {
    update(
        cx,
        session,
        json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}),
    )
}

struct Harness {
    client: AcpClient,
    events: AcpEvents,
    agent: FakeAgent,
    agent_task: JoinHandle<()>,
}

const CLAUDE_NOTICE: &str = "You've hit your 5-hour limit · resets 3pm (Europe/Bucharest)";
const CODEX_NOTICE: &str = "You\u{2019}ve hit your usage limit. Visit https://chatgpt.com/settings/usage to purchase more credits or try again at Nov 12th, 2026 3:05 PM.";

async fn start() -> Harness {
    start_with(ConnectOptions::default()).await
}

async fn start_with(options: ConnectOptions) -> Harness {
    let (client_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (agent_read, agent_write) = tokio::io::split(agent_io);
    let (client_read, client_write) = tokio::io::split(client_io);
    let agent = FakeAgent::default();
    let agent_task = tokio::spawn(agent.clone().run(ByteStreams::new(
        agent_write.compat_write(),
        agent_read.compat(),
    )));
    let (client, events) = AcpClient::connect_tokio(client_write, client_read, options)
        .await
        .expect("connect");
    Harness {
        client,
        events,
        agent,
        agent_task,
    }
}

fn cwd() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\work\repo")
    } else {
        PathBuf::from("/work/repo")
    }
}

async fn next(events: &mut AcpEvents) -> AcpEvent {
    tokio::time::timeout(TIMEOUT, events.recv())
        .await
        .expect("timed out waiting for an event")
        .expect("the event stream ended")
}

/// Session events until the turn ends, the turn's end included.
async fn turn_events(events: &mut AcpEvents) -> Vec<SessionEvent> {
    let mut seen = Vec::new();
    loop {
        match next(events).await {
            AcpEvent::Session { session_id, event } => {
                assert_eq!(session_id, SESSION);
                let done = matches!(event, SessionEvent::TurnEnded(_));
                seen.push(event);
                if done {
                    return seen;
                }
            }
            other => panic!("unexpected event {other:?}"),
        }
    }
}

fn texts(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentMessageChunk {
                content: Content::Text(text),
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn initialize_reports_the_agent() {
    let h = start().await;
    let agent = h.client.agent();
    assert_eq!(agent.name.as_deref(), Some("fake-agent"));
    assert_eq!(agent.version.as_deref(), Some("0.0.1"));
    assert_eq!(agent.protocol_version, 1);
    assert!(agent.capabilities.load_session);
    assert!(agent.capabilities.close_session);
    assert!(!agent.capabilities.resume_session);
    assert_eq!(agent.auth_methods[0].id, "fake-login");
}

#[tokio::test]
async fn a_prompt_streams_typed_events_then_ends_the_turn() {
    let mut h = start().await;
    let session = h.client.new_session(&cwd()).await.unwrap();
    assert_eq!(session.session_id, SESSION);
    assert_eq!(session.modes.as_ref().unwrap().current, "code");
    assert_eq!(session.models.as_ref().unwrap().current, "fast");

    let result = h.client.prompt(SESSION, ["stream".into()]).await.unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    let usage = result.usage.expect("usage");
    assert_eq!((usage.input_tokens, usage.output_tokens), (20, 10));

    let events = turn_events(&mut h.events).await;
    assert_eq!(texts(&events), ["Hello", ", world"]);
    assert!(matches!(
        &events[0],
        SessionEvent::AgentThoughtChunk { content: Content::Text(text), .. } if text == "Thinking."
    ));
    let call = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ToolCall(call) => Some(call),
            _ => None,
        })
        .expect("a tool call");
    assert_eq!(
        (call.id.as_str(), call.kind, call.status),
        ("t1", ToolKind::Read, ToolCallStatus::InProgress)
    );
    assert_eq!(call.locations[0].line, Some(3));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::ToolCallUpdate(update) if update.status == Some(ToolCallStatus::Completed)
    )));
    assert!(events.iter().any(
        |event| matches!(event, SessionEvent::Plan(entries) if entries[0].status == "completed")
    ));
    let usage = events
        .iter()
        .position(|event| matches!(
            event,
            SessionEvent::ContextUsage { used: 1200, size: 200_000, cost: Some(cost) } if cost.currency == "USD"
        ))
        .expect("context usage");
    // Claude Code's rate-limit status rides on the usage update.
    assert!(matches!(
        &events[usage + 1],
        SessionEvent::RateLimit(status)
            if status.status == "allowed_warning" && status.utilization == Some(0.9)
    ));
    assert!(
        events.iter().any(
            |event| matches!(event, SessionEvent::ModeChanged { mode_id } if mode_id == "plan")
        )
    );
    // An update kind this crate doesn't know is passed on, and the
    // connection survives it.
    assert!(events.iter().any(
        |event| matches!(event, SessionEvent::Other { kind, .. } if kind == "some_future_update")
    ));
    assert!(matches!(
        events.last(),
        Some(SessionEvent::TurnEnded(TurnEnd::Stopped {
            reason: StopReason::EndTurn,
            usage: Some(_)
        }))
    ));

    // The connection still works.
    h.client.prompt(SESSION, ["again".into()]).await.unwrap();
    assert_eq!(texts(&turn_events(&mut h.events).await), ["echo: again"]);
}

#[tokio::test]
async fn a_permission_request_waits_for_the_callers_decision() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    let client = h.client.clone();
    let prompt = tokio::spawn(async move { client.prompt(SESSION, ["permission".into()]).await });

    let SessionEvent::ToolCall(call) = (match next(&mut h.events).await {
        AcpEvent::Session { event, .. } => event,
        other => panic!("expected the tool call first, got {other:?}"),
    }) else {
        panic!("expected a tool call");
    };
    assert_eq!(call.status, ToolCallStatus::Pending);

    let AcpEvent::Permission(request) = next(&mut h.events).await else {
        panic!("expected a permission request");
    };
    assert_eq!(request.session_id, SESSION);
    assert_eq!(request.tool_call.id, "t2");
    assert_eq!(request.options.len(), 3);
    assert_eq!(
        request.option(&PermissionKind::RejectOnce).unwrap().id,
        "reject"
    );
    assert!(request.is_open());
    // The agent waits while the caller decides.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!prompt.is_finished());

    assert!(matches!(
        request.respond(PermissionDecision::Select("nope".into())),
        Err(AcpError::Protocol { .. })
    ));
    request.allow_once().unwrap();
    assert!(!request.is_open());
    assert!(request.reject_once().is_err(), "a second answer is refused");

    let events = turn_events(&mut h.events).await;
    assert_eq!(texts(&events), ["selected allow"]);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::ToolCallUpdate(update) if update.status == Some(ToolCallStatus::Completed)
    )));
    assert_eq!(
        prompt.await.unwrap().unwrap().stop_reason,
        StopReason::EndTurn
    );
}

#[tokio::test]
async fn a_dropped_permission_request_is_answered_as_cancelled() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    let client = h.client.clone();
    let prompt = tokio::spawn(async move { client.prompt(SESSION, ["permission".into()]).await });
    loop {
        if let AcpEvent::Permission(request) = next(&mut h.events).await {
            drop(request);
            break;
        }
    }
    assert_eq!(texts(&turn_events(&mut h.events).await), ["cancelled"]);
    prompt.await.unwrap().unwrap();
}

#[tokio::test]
async fn cancel_ends_the_turn_as_cancelled() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    let client = h.client.clone();
    let prompt =
        tokio::spawn(async move { client.prompt(SESSION, ["wait-for-cancel".into()]).await });
    assert!(matches!(
        next(&mut h.events).await,
        AcpEvent::Session {
            event: SessionEvent::AgentMessageChunk { .. },
            ..
        }
    ));
    h.client.cancel(SESSION).await.unwrap();
    let result = tokio::time::timeout(TIMEOUT, prompt)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.stop_reason, StopReason::Cancelled);
    assert!(matches!(
        turn_events(&mut h.events).await.last(),
        Some(SessionEvent::TurnEnded(TurnEnd::Stopped {
            reason: StopReason::Cancelled,
            ..
        }))
    ));
    assert_eq!(h.agent.seen.lock().unwrap().cancels, [SESSION]);
}

#[tokio::test]
async fn cancel_answers_open_permission_requests_as_cancelled() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    let client = h.client.clone();
    let prompt = tokio::spawn(async move {
        client
            .prompt(SESSION, ["cancel-during-permission".into()])
            .await
    });
    let request = loop {
        if let AcpEvent::Permission(request) = next(&mut h.events).await {
            break request;
        }
    };
    h.client.cancel(SESSION).await.unwrap();
    assert!(!request.is_open());
    assert!(
        request.allow_once().is_err(),
        "too late: the turn was cancelled"
    );
    let events = turn_events(&mut h.events).await;
    assert_eq!(texts(&events), ["cancelled"]);
    assert_eq!(
        tokio::time::timeout(TIMEOUT, prompt)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .stop_reason,
        StopReason::Cancelled
    );
}

#[tokio::test]
async fn an_agent_error_fails_the_prompt_and_ends_the_turn() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    let error = h
        .client
        .prompt(SESSION, ["error".into()])
        .await
        .unwrap_err();
    let AcpError::Agent { code, data, .. } = &error else {
        panic!("expected an agent error, got {error:?}");
    };
    assert_eq!(*code, -32603);
    assert_eq!(data.as_ref().unwrap()["details"], "boom");
    assert!(matches!(
        turn_events(&mut h.events).await.last(),
        Some(SessionEvent::TurnEnded(TurnEnd::Error {
            code: Some(-32603),
            ..
        }))
    ));
}

/// Runs a prompt that should hit a limit; returns the limit and the turn's
/// events, after checking the turn ends with `LimitReached`, `TurnEnded`.
async fn expect_limit(h: &mut Harness, prompt: &str) -> (LimitInfo, Vec<SessionEvent>) {
    let error = h.client.prompt(SESSION, [prompt.into()]).await.unwrap_err();
    let AcpError::LimitReached(info) = error else {
        panic!("expected a limit, got {error:?}");
    };
    let events = turn_events(&mut h.events).await;
    let n = events.len();
    assert!(matches!(&events[n - 2], SessionEvent::LimitReached(found) if *found == info));
    assert!(
        matches!(&events[n - 1], SessionEvent::TurnEnded(TurnEnd::Limit(found)) if *found == info)
    );
    (info, events)
}

/// `text` at `local` (`YYYY-MM-DD HH:MM`) in this machine's time zone.
fn local(text: &str) -> jiff::Timestamp {
    text.parse::<jiff::civil::DateTime>()
        .unwrap()
        .to_zoned(jiff::tz::TimeZone::system())
        .unwrap()
        .timestamp()
}

#[tokio::test]
async fn a_claude_usage_limit_becomes_limit_reached() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    let (info, events) = expect_limit(&mut h, "claude-limit").await;
    assert_eq!(info.source, LimitSource::Error);
    assert_eq!(info.code, Some(-32603));
    assert_eq!(info.message, CLAUDE_NOTICE);
    // 3pm in Bucharest, the next one after now.
    let reset = info.resets_at.expect("a reset time");
    let in_bucharest = reset.to_zoned(jiff::tz::TimeZone::get("Europe/Bucharest").unwrap());
    assert_eq!((in_bucharest.hour(), in_bucharest.minute()), (15, 0));
    assert!(reset > jiff::Timestamp::now());
    // The notice streamed as text before the error, and the status came too.
    assert_eq!(texts(&events), [CLAUDE_NOTICE]);
    assert!(events.iter().any(
        |event| matches!(event, SessionEvent::RateLimit(status) if status.status == "rejected")
    ));
}

#[tokio::test]
async fn a_short_lived_claude_rate_limit_is_an_ordinary_error() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    let error = h
        .client
        .prompt(SESSION, ["claude-busy".into()])
        .await
        .unwrap_err();
    assert!(
        matches!(error, AcpError::Agent { code: -32603, .. }),
        "{error:?}"
    );
    assert!(matches!(
        turn_events(&mut h.events).await.last(),
        Some(SessionEvent::TurnEnded(TurnEnd::Error { .. }))
    ));
}

#[tokio::test]
async fn a_codex_usage_limit_becomes_limit_reached() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    let (info, _) = expect_limit(&mut h, "codex-limit").await;
    assert_eq!(info.source, LimitSource::Error);
    assert_eq!(info.message, CODEX_NOTICE);
    assert_eq!(info.resets_at, Some(local("2026-11-12 15:05")));
}

#[tokio::test]
async fn a_codex_turn_that_never_ends_after_a_limit_is_cancelled() {
    let mut h = start_with(ConnectOptions {
        limit_grace: Duration::from_millis(100),
        ..ConnectOptions::default()
    })
    .await;
    h.client.new_session(&cwd()).await.unwrap();
    let (info, _) = tokio::time::timeout(TIMEOUT, expect_limit(&mut h, "codex-limit-then-hang"))
        .await
        .expect("the client gives up on the turn");
    assert_eq!(info.source, LimitSource::Reply);
    assert_eq!(info.resets_at, Some(local("2026-11-12 15:05")));
    assert_eq!(h.agent.seen.lock().unwrap().cancels, [SESSION]);
}

#[tokio::test]
async fn a_cursor_usage_limit_becomes_limit_reached() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    let (info, _) = expect_limit(&mut h, "cursor-limit").await;
    assert_eq!(info.source, LimitSource::Reply);
    assert_eq!(info.message, "Upgrade your plan to continue");
    assert_eq!(info.resets_at, None);
}

#[tokio::test]
async fn a_notice_the_agent_works_past_is_not_a_limit() {
    let h = start_with(ConnectOptions {
        limit_grace: Duration::from_millis(100),
        ..ConnectOptions::default()
    })
    .await;
    h.client.new_session(&cwd()).await.unwrap();
    let result = h
        .client
        .prompt(SESSION, ["notice-then-more-work".into()])
        .await
        .unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert!(h.agent.seen.lock().unwrap().cancels.is_empty());
}

#[tokio::test]
async fn auth_required_is_surfaced_with_the_agents_methods() {
    let h = start().await;
    let error = h
        .client
        .new_session(&cwd().join("needs-login"))
        .await
        .unwrap_err();
    let AcpError::AuthRequired { methods, .. } = error else {
        panic!("expected AuthRequired, got {error:?}");
    };
    assert_eq!(methods[0].id, "fake-login");
}

#[tokio::test]
async fn file_and_terminal_requests_are_refused() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    h.client
        .prompt(SESSION, ["ask-for-a-file".into()])
        .await
        .unwrap();
    assert_eq!(
        texts(&turn_events(&mut h.events).await),
        ["refused with -32601"]
    );
}

#[tokio::test]
async fn models_are_set_through_the_model_config_option() {
    let h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    h.client.set_model(SESSION, "smart").await.unwrap();
    let options = h
        .client
        .set_config_option(SESSION, "model", "fast")
        .await
        .unwrap();
    assert_eq!(options[0].id, "model");
    h.client.set_mode(SESSION, "plan").await.unwrap();
    assert_eq!(
        h.agent.seen.lock().unwrap().config,
        [
            ("model".to_owned(), "smart".to_owned()),
            ("model".to_owned(), "fast".to_owned())
        ]
    );
}

#[tokio::test]
async fn models_fall_back_to_session_set_model() {
    let h = start().await;
    let session = h
        .client
        .new_session(&cwd().join("legacy-models"))
        .await
        .unwrap();
    assert_eq!(session.models.as_ref().unwrap().config_id, None);
    h.client.set_model(SESSION, "old-b").await.unwrap();
    assert_eq!(h.agent.seen.lock().unwrap().legacy_models, ["old-b"]);
}

#[tokio::test]
async fn load_replays_history_before_returning() {
    let mut h = start().await;
    let session = h.client.load_session(SESSION, &cwd()).await.unwrap();
    assert_eq!(session.session_id, SESSION);
    assert_eq!(session.models.unwrap().current, "smart");
    let mut replayed = Vec::new();
    while let Some(AcpEvent::Session { event, .. }) = h.events.try_recv() {
        replayed.push(event);
    }
    assert!(
        matches!(&replayed[0], SessionEvent::UserMessageChunk { content: Content::Text(text) } if text == "earlier question")
    );
    assert_eq!(texts(&replayed), ["earlier answer"]);
    assert!(matches!(
        h.client.resume_session(SESSION, &cwd()).await,
        Err(AcpError::Unsupported(_))
    ));
    h.client.close_session(SESSION).await.unwrap();
}

#[tokio::test]
async fn stop_closes_the_connection_and_the_agent_sees_eof() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    h.client.stop().await;
    assert!(h.client.is_closed());
    assert!(matches!(next(&mut h.events).await, AcpEvent::Closed { .. }));
    tokio::time::timeout(TIMEOUT, h.agent_task)
        .await
        .expect("the agent ends")
        .unwrap();
    assert!(matches!(
        h.client.prompt(SESSION, ["after stop".into()]).await,
        Err(AcpError::Closed)
    ));
}

#[tokio::test]
async fn an_agent_that_dies_fails_the_open_prompt() {
    let mut h = start().await;
    h.client.new_session(&cwd()).await.unwrap();
    let client = h.client.clone();
    let prompt = tokio::spawn(async move { client.prompt(SESSION, ["hang".into()]).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.agent_task.abort();
    let result = tokio::time::timeout(TIMEOUT, prompt)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(AcpError::Closed)), "got {result:?}");
    loop {
        match next(&mut h.events).await {
            AcpEvent::Closed { .. } => break,
            AcpEvent::Session { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    // Nothing follows Closed, even from calls that fail afterwards.
    assert!(matches!(
        h.client.prompt(SESSION, ["late".into()]).await,
        Err(AcpError::Closed)
    ));
    assert!(h.events.try_recv().is_none());
}

#[test]
fn cwd_is_absolute() {
    assert!(Path::new(&cwd()).is_absolute());
}
