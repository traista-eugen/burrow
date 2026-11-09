//! The ACP adapter end to end: the real `burrowd` with the scripted fake ACP agent
//! (`burrow-fake-acp-agent`, a real process tree speaking real ACP), driven through
//! `burrow-client`. Spends no quota; `acp_live.rs` has the same checks against the real agents.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use burrow_agents::acp::fake_agent::LIMIT_NOTICE;
use burrow_agents::launcher::{descendants, is_running};
use burrow_client::{CallError, Client, Subscription};
use burrow_protocol::methods::{
    AgentInstall, AgentList, InboxList, InboxRespond, SessionCancel, SessionCreate, SessionGet,
    SessionPrompt, SessionSetAgentMode, SessionSetModel, SessionStop, UsageGet,
};
use burrow_protocol::*;
use common::*;

fn fake_acp() -> AgentId {
    AgentId::from("fake-acp")
}

/// A started `burrowd --dev --agents=fake-acp` and a client.
async fn start(dir: &Path, test: &str) -> (Daemon, Client) {
    let pipe = unique_pipe(test);
    let agents = dir.join("agents");
    let daemon = Daemon::start_with(
        dir,
        &pipe,
        &[
            "--dev",
            "--agents=fake-acp",
            "--agents-dir",
            agents.to_str().unwrap(),
        ],
    )
    .await;
    let client = connect(&pipe).await;
    (daemon, client)
}

fn create_params(cwd: &Path) -> SessionCreateParams {
    SessionCreateParams {
        project_id: None,
        agent: fake_acp(),
        model: None,
        mode: SessionMode::Structured,
        agent_mode: None,
        cwd: Some(cwd.display().to_string()),
        title: None,
    }
}

async fn create(client: &Client, cwd: &Path) -> Session {
    client
        .call::<SessionCreate>(&create_params(cwd))
        .await
        .expect("session/create")
}

async fn prompt(client: &Client, session: &str, text: &str) -> String {
    client
        .call::<SessionPrompt>(&SessionPromptParams {
            session_id: session.to_owned(),
            content: vec![ContentBlock::text(text)],
        })
        .await
        .expect("session/prompt")
        .turn_id
}

async fn get(client: &Client, session: &str) -> Session {
    client
        .call::<SessionGet>(&SessionIdParams {
            session_id: session.to_owned(),
        })
        .await
        .expect("session/get")
}

async fn cancel(client: &Client, session: &str) {
    client
        .call::<SessionCancel>(&SessionIdParams {
            session_id: session.to_owned(),
        })
        .await
        .expect("session/cancel");
}

/// Events until (and including) the first one `done` accepts.
async fn until(subscription: &mut Subscription, done: impl Fn(&Event) -> bool) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    tokio::time::timeout(WAIT, async {
        loop {
            let event = subscription.next().await.expect("the subscription ended");
            let last = done(&event.event);
            events.push(event);
            if last {
                return;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out; got {:?}", types(&events)));
    events
}

fn types(events: &[StreamEvent]) -> Vec<&str> {
    events.iter().map(|event| event.event.type_name()).collect()
}

fn turn_ended(event: &Event) -> bool {
    matches!(event, Event::TurnEnded(_))
}

fn last_turn_end(events: &[StreamEvent]) -> &TurnEnded {
    events
        .iter()
        .rev()
        .find_map(|event| match &event.event {
            Event::TurnEnded(ended) => Some(ended),
            _ => None,
        })
        .expect("a turnEnded")
}

fn message_text(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match &event.event {
            Event::AgentMessageChunk(chunk) => Some(chunk.text.as_str()),
            _ => None,
        })
        .collect()
}

fn error_kind(result: Result<impl std::fmt::Debug, CallError>) -> ErrorKind {
    result
        .expect_err("expected an error")
        .rpc()
        .and_then(ErrorObject::kind)
        .expect("a Burrow error kind")
}

fn error_code(result: Result<impl std::fmt::Debug, CallError>) -> i64 {
    result.expect_err("expected an error").rpc().unwrap().code
}

/// The fake's own pid and its child's, from its `/pids` reply. `events` follows the session.
async fn agent_pids(client: &Client, session: &str, events: &mut Subscription) -> (u32, u32) {
    prompt(client, session, "/pids").await;
    let turn = until(events, turn_ended).await;
    let reply = message_text(&turn);
    let mut words = reply.split_whitespace();
    assert_eq!(words.next(), Some("pids"), "{reply}");
    let agent = words.next().unwrap().parse().unwrap();
    let child = words.next().unwrap().parse().unwrap();
    (agent, child)
}

async fn wait_gone(pids: &[u32]) {
    let deadline = Instant::now() + WAIT;
    while pids.iter().any(|&pid| is_running(pid)) {
        assert!(
            Instant::now() < deadline,
            "processes {pids:?} are still running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_acp_session_streams_and_starts_in_the_mode_that_asks() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "acp-stream").await;

    let agents = client.call::<AgentList>(&Empty {}).await.unwrap();
    assert_eq!(agents.len(), 1);
    let agent = &agents[0];
    assert_eq!(agent.id, fake_acp());
    assert_eq!(agent.integration, Integration::Acp);
    assert!(agent.available, "{:?}", agent.problem);
    assert_eq!(agent.default_agent_mode.as_deref(), Some("ask"));
    assert_eq!(agent.adapter, None);

    let session = create(&client, dir.path()).await;
    assert_eq!(session.state, SessionState::Idle);
    // The fake starts in "auto", which never asks; Burrow switched it to "ask".
    assert_eq!(session.agent_mode.as_deref(), Some("ask"));
    let modes: Vec<&str> = session.agent_modes.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(modes, ["auto", "ask", "plan"]);
    assert_eq!(session.agent_modes[1].name.as_deref(), Some("Ask"));
    assert_eq!(session.model.as_deref(), Some("fake-fast"));
    let models: Vec<&str> = session.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(models, ["fake-fast", "fake-smart"]);

    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();
    let turn_id = prompt(&client, &session.id, "hello there").await;
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(message_text(&turn), "Echo: hello there");
    assert!(
        types(&turn).contains(&"agentThoughtChunk"),
        "{:?}",
        types(&turn)
    );
    let usage = turn
        .iter()
        .find_map(|event| match &event.event {
            Event::Usage(usage) => Some(usage.clone()),
            _ => None,
        })
        .expect("usage");
    assert_eq!((usage.input_tokens, usage.output_tokens), (2, 3));
    assert_eq!(usage.cost_usd, Some(0.0125));
    assert_eq!(usage.turn_id.as_deref(), Some(turn_id.as_str()));
    let ended = last_turn_end(&turn);
    assert_eq!(
        (ended.turn_id.as_str(), &ended.reason),
        (turn_id.as_str(), &TurnEndReason::Completed)
    );

    // A second turn reports only its own share of the session's cost.
    prompt(&client, &session.id, "again").await;
    let turn = until(&mut events, turn_ended).await;
    let cost = turn.iter().find_map(|event| match &event.event {
        Event::Usage(usage) => usage.cost_usd,
        _ => None,
    });
    assert_eq!(cost, Some(0.0125));
    let session = get(&client, &session.id).await;
    assert_eq!(session.usage.unwrap().cost_usd, Some(0.025));

    // A plan, with ACP's values in camelCase.
    prompt(&client, &session.id, "/plan").await;
    let turn = until(&mut events, turn_ended).await;
    let plan = turn
        .iter()
        .find_map(|event| match &event.event {
            Event::Plan(plan) => Some(plan.clone()),
            _ => None,
        })
        .expect("a plan");
    let statuses: Vec<&str> = plan.entries.iter().map(|e| e.status.as_str()).collect();
    assert_eq!(statuses, ["completed", "inProgress", "pending"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_acp_permission_request_round_trips_through_the_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "acp-permission").await;
    let session = create(&client, dir.path()).await;
    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();

    prompt(&client, &session.id, "/permission").await;
    let asked = until(&mut events, |event| {
        matches!(event, Event::PermissionRequested(_))
    })
    .await;
    let Some(Event::ToolCall(call)) = asked
        .iter()
        .map(|event| &event.event)
        .find(|event| matches!(event, Event::ToolCall(_)))
    else {
        panic!("no tool call before the question: {:?}", types(&asked));
    };
    assert_eq!(
        (call.title.as_str(), &call.kind, &call.status),
        ("Write notes.txt", &ToolKind::Edit, &ToolCallStatus::Pending)
    );
    let Event::PermissionRequested(request) = &asked.last().unwrap().event else {
        unreachable!()
    };
    assert_eq!(request.tool_call_id, call.tool_call_id);
    let options: Vec<(&str, &PermissionOptionKind)> = request
        .options
        .iter()
        .map(|option| (option.option_id.as_str(), &option.kind))
        .collect();
    assert_eq!(
        options,
        [
            ("allow-once", &PermissionOptionKind::AllowOnce),
            ("allow-always", &PermissionOptionKind::AllowAlways),
            ("reject-once", &PermissionOptionKind::RejectOnce),
        ]
    );
    until(
        &mut events,
        |event| matches!(event, Event::StateChanged(c) if c.state == SessionState::Waiting),
    )
    .await;
    let open = client
        .call::<InboxList>(&InboxListParams {
            status: Some(InboxItemStatus::Open),
            ..InboxListParams::default()
        })
        .await
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, request.inbox_item_id);
    assert_eq!(open[0].title, "Write notes.txt");

    client
        .call::<InboxRespond>(&InboxRespondParams {
            item_id: request.inbox_item_id.clone(),
            action: "allow-once".into(),
            payload: None,
        })
        .await
        .unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert!(turn.iter().any(|event| matches!(&event.event,
        Event::ToolCallUpdate(u) if u.status == Some(ToolCallStatus::Completed))));
    assert_eq!(message_text(&turn), "Allowed.");
    assert_eq!(last_turn_end(&turn).reason, TurnEndReason::Completed);

    // A question about a tool call the agent never announced: Burrow announces it first.
    prompt(&client, &session.id, "/sudden-permission").await;
    let asked = until(&mut events, |event| {
        matches!(event, Event::PermissionRequested(_))
    })
    .await;
    assert!(asked.iter().any(|event| matches!(&event.event,
        Event::ToolCall(c) if c.title == "Run echo hi" && c.kind == ToolKind::Execute)));
    let Event::PermissionRequested(request) = &asked.last().unwrap().event else {
        unreachable!()
    };
    client
        .call::<InboxRespond>(&InboxRespondParams {
            item_id: request.inbox_item_id.clone(),
            action: "reject-once".into(),
            payload: None,
        })
        .await
        .unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert!(turn.iter().any(|event| matches!(&event.event,
        Event::ToolCallUpdate(u) if u.status == Some(ToolCallStatus::Failed))));
    assert_eq!(message_text(&turn), "Rejected.");

    // In the fake's own "auto" mode nothing asks.
    let mut params = create_params(dir.path());
    params.agent_mode = Some("auto".into());
    let auto = client.call::<SessionCreate>(&params).await.unwrap();
    assert_eq!(auto.agent_mode.as_deref(), Some("auto"));
    let mut auto_events = client
        .subscribe(StreamName::session(&auto.id), None)
        .await
        .unwrap();
    prompt(&client, &auto.id, "/permission").await;
    let turn = until(&mut auto_events, turn_ended).await;
    assert!(!types(&turn).contains(&"permissionRequested"));
    assert!(message_text(&turn).contains("without asking"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_the_agent_ignores_ends_the_turn_and_restarts_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let (daemon, client) = start(dir.path(), "acp-cancel").await;
    let session = create(&client, dir.path()).await;
    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();

    // An agent that honours the cancel.
    prompt(&client, &session.id, "/slow").await;
    until(&mut events, |event| {
        matches!(event, Event::AgentMessageChunk(_))
    })
    .await;
    cancel(&client, &session.id).await;
    let turn = until(&mut events, turn_ended).await;
    let ended = last_turn_end(&turn);
    assert_eq!(
        (&ended.reason, &ended.message),
        (&TurnEndReason::Cancelled, &None)
    );

    // A cancel expires the open question.
    prompt(&client, &session.id, "/permission").await;
    until(&mut events, |event| {
        matches!(event, Event::PermissionRequested(_))
    })
    .await;
    cancel(&client, &session.id).await;
    let turn = until(&mut events, turn_ended).await;
    assert!(turn.iter().any(|event| matches!(&event.event,
        Event::PermissionResolved(r) if r.status == InboxItemStatus::Expired)));
    assert_eq!(last_turn_end(&turn).reason, TurnEndReason::Cancelled);

    // An agent that drops the cancel (codex-acp#641): the watchdog ends the turn after the
    // grace period and restarts the agent, which reopens the same session.
    let (agent, child) = agent_pids(&client, &session.id, &mut events).await;
    prompt(&client, &session.id, "/hang").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let cancelled = Instant::now();
    cancel(&client, &session.id).await;
    let turn = until(&mut events, turn_ended).await;
    let waited = cancelled.elapsed();
    let ended = last_turn_end(&turn);
    assert_eq!(ended.reason, TurnEndReason::Cancelled);
    let message = ended.message.as_deref().unwrap_or_default();
    assert!(message.contains("restarted"), "{message}");
    assert!(
        (Duration::from_millis(900)..Duration::from_secs(10)).contains(&waited),
        "the watchdog fired after {waited:?}"
    );
    wait_gone(&[agent, child]).await;

    // The new process carries on, in the mode the session had.
    let (new_agent, _) = agent_pids(&client, &session.id, &mut events).await;
    assert_ne!(new_agent, agent);
    assert!(descendants(daemon.pid()).contains(&new_agent));
    let session_now = get(&client, &session.id).await;
    assert_eq!(session_now.state, SessionState::Idle);
    assert_eq!(session_now.agent_mode.as_deref(), Some("ask"));
    prompt(&client, &session.id, "/permission").await;
    until(&mut events, |event| {
        matches!(event, Event::PermissionRequested(_))
    })
    .await;
    cancel(&client, &session.id).await;
    until(&mut events, turn_ended).await;
    prompt(&client, &session.id, "still here").await;
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(message_text(&turn), "Echo: still here");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_acp_usage_limit_makes_the_session_limited() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "acp-limit").await;
    let session = create(&client, dir.path()).await;
    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();

    prompt(&client, &session.id, "/limit").await;
    let turn = until(&mut events, turn_ended).await;
    let limit = turn
        .iter()
        .find_map(|event| match &event.event {
            Event::LimitReached(limit) => Some(limit.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no limitReached: {:?}", types(&turn)));
    assert_eq!(limit.provider, "fake-acp");
    assert_eq!(limit.message, LIMIT_NOTICE);
    // "resets 3pm (Europe/Bucharest)": the next 3 pm there, within a day.
    let resets_at = limit.resets_at.expect("a reset time");
    let ahead = resets_at.as_unix_millis() - Timestamp::now().as_unix_millis();
    assert!((0..=24 * 3600 * 1000).contains(&ahead), "{resets_at}");
    assert_eq!(last_turn_end(&turn).reason, TurnEndReason::Limit);

    let session = get(&client, &session.id).await;
    assert_eq!(session.state, SessionState::Limited);
    assert_eq!(session.limit.unwrap().resets_at, Some(resets_at));
    let usage = client.call::<UsageGet>(&Empty {}).await.unwrap();
    assert!(usage.iter().any(|u| u.provider == "fake-acp" && u.limited));
    // Prompting before the reset is refused.
    let refused = client
        .call::<SessionPrompt>(&SessionPromptParams {
            session_id: session.id.clone(),
            content: vec![ContentBlock::text("hello?")],
        })
        .await;
    assert_eq!(error_kind(refused), ErrorKind::LimitReached);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_modes_models_and_rate_limits() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "acp-modes").await;
    let session = create(&client, dir.path()).await;
    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();
    let set_mode = |mode: &str| SessionSetAgentModeParams {
        session_id: session.id.clone(),
        agent_mode: mode.into(),
    };
    let set_model = |model: &str| SessionSetModelParams {
        session_id: session.id.clone(),
        model: model.into(),
    };

    // The agent switches its own mode, then Burrow switches it back.
    prompt(&client, &session.id, "/mode").await;
    let turn = until(&mut events, turn_ended).await;
    assert!(turn.iter().any(|event| matches!(&event.event,
        Event::AgentModeChanged(c) if c.agent_mode == "plan")));
    assert_eq!(
        get(&client, &session.id).await.agent_mode.as_deref(),
        Some("plan")
    );
    client
        .call::<SessionSetAgentMode>(&set_mode("ask"))
        .await
        .unwrap();
    let changed = until(&mut events, |event| {
        matches!(event, Event::AgentModeChanged(_))
    })
    .await;
    assert!(matches!(&changed.last().unwrap().event,
        Event::AgentModeChanged(c) if c.agent_mode == "ask"));
    assert_eq!(
        error_code(client.call::<SessionSetAgentMode>(&set_mode("yolo")).await),
        code::INVALID_PARAMS
    );

    // Models: the agent's own switch, Burrow's, and an unknown one.
    prompt(&client, &session.id, "/model").await;
    let turn = until(&mut events, turn_ended).await;
    assert!(turn.iter().any(|event| matches!(&event.event,
        Event::ModelChanged(c) if c.model == "fake-smart")));
    client
        .call::<SessionSetModel>(&set_model("fake-fast"))
        .await
        .unwrap();
    until(
        &mut events,
        |event| matches!(event, Event::ModelChanged(c) if c.model == "fake-fast"),
    )
    .await;
    assert_eq!(
        error_code(client.call::<SessionSetModel>(&set_model("gpt-9")).await),
        code::INVALID_PARAMS
    );

    // Claude Code's rate-limit status on the side becomes provider usage.
    prompt(&client, &session.id, "/rate").await;
    until(&mut events, turn_ended).await;
    let usage = client.call::<UsageGet>(&Empty {}).await.unwrap();
    let rate = usage
        .iter()
        .find(|u| u.provider == "fake-acp")
        .expect("reported usage");
    assert_eq!((rate.used, rate.limit), (Some(90.0), Some(100.0)));
    assert_eq!(rate.unit.as_deref(), Some("percent"));
    assert!(!rate.limited);
    assert!(rate.resets_at.is_some());

    // session/create with a model and a mode; an unknown mode fails the start.
    let mut params = create_params(dir.path());
    params.model = Some("fake-smart".into());
    params.agent_mode = Some("plan".into());
    let other = client.call::<SessionCreate>(&params).await.unwrap();
    assert_eq!(other.model.as_deref(), Some("fake-smart"));
    assert_eq!(other.agent_mode.as_deref(), Some("plan"));
    params.agent_mode = Some("yolo".into());
    assert_eq!(
        error_code(client.call::<SessionCreate>(&params).await),
        code::INVALID_PARAMS
    );
    params.agent_mode = None;
    params.model = Some("gpt-9".into());
    assert_eq!(
        error_code(client.call::<SessionCreate>(&params).await),
        code::INVALID_PARAMS
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_an_acp_session_leaves_no_stray_processes() {
    let dir = tempfile::tempdir().unwrap();
    let (daemon, client) = start(dir.path(), "acp-stop").await;
    let session = create(&client, dir.path()).await;
    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();
    let (agent, child) = agent_pids(&client, &session.id, &mut events).await;
    let tree = descendants(daemon.pid());
    assert!(tree.contains(&agent) && tree.contains(&child), "{tree:?}");

    client
        .call::<SessionStop>(&SessionIdParams {
            session_id: session.id.clone(),
        })
        .await
        .unwrap();
    assert!(!is_running(agent), "the fake ACP agent survived");
    assert!(!is_running(child), "its child survived");
    assert!(descendants(daemon.pid()).is_empty());
    let stopped = get(&client, &session.id).await;
    assert_eq!(stopped.state, SessionState::Stopped);

    // An agent that dies by itself fails the session, saying how it exited.
    let crashing = create(&client, dir.path()).await;
    let mut events = client
        .subscribe(StreamName::session(&crashing.id), None)
        .await
        .unwrap();
    prompt(&client, &crashing.id, "/crash").await;
    until(
        &mut events,
        |event| matches!(event, Event::StateChanged(c) if c.state == SessionState::Failed),
    )
    .await;
    let failed = get(&client, &crashing.id).await;
    let reason = failed.reason.unwrap_or_default();
    assert!(reason.contains("exit code 3"), "{reason}");
    let deadline = Instant::now() + WAIT;
    while !descendants(daemon.pid()).is_empty() {
        assert!(
            Instant::now() < deadline,
            "the crashed agent's tree lingers"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_install_refuses_agents_with_nothing_to_install() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "acp-install").await;
    let nothing = client
        .call::<AgentInstall>(&AgentInstallParams { agent: fake_acp() })
        .await;
    assert_eq!(error_kind(nothing), ErrorKind::InvalidState);
    let not_offered = client
        .call::<AgentInstall>(&AgentInstallParams {
            agent: AgentId::Claude,
        })
        .await;
    assert_eq!(error_kind(not_offered), ErrorKind::Unavailable);
}
