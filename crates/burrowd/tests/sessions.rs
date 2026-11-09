//! M2 end to end: the real `burrowd` with the scripted fake agent (`burrow-fake-agent`, a real
//! child process tree), driven through `burrow-client`.
//!
//! `stopping_a_session_leaves_no_stray_processes` is M2's automated "Done when" check
//! (docs/plan.md).

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use burrow_agents::launcher::{descendants, is_running};
use burrow_client::{CallError, Client, Subscription};
use burrow_protocol::methods::{
    AgentList, AgentRefresh, InboxGet, InboxList, InboxRespond, Ping, ProjectAdd, ProjectGet,
    ProjectList, ProjectRemove, SessionArchive, SessionCancel, SessionCreate, SessionGet,
    SessionList, SessionPrompt, SessionSetModel, SessionStop, UsageGet,
};
use burrow_protocol::*;
use common::*;

/// A started `burrowd --dev --agents=fake` and a client.
async fn start(dir: &Path, test: &str) -> (Daemon, Client) {
    let pipe = unique_pipe(test);
    let daemon = Daemon::start(dir, &pipe, true).await;
    let client = connect(&pipe).await;
    (daemon, client)
}

fn create_params(cwd: &Path) -> SessionCreateParams {
    SessionCreateParams {
        project_id: None,
        agent: AgentId::Fake,
        model: None,
        mode: SessionMode::Structured,
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

fn states(events: &[StreamEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            Event::StateChanged(changed) => Some(changed.state.to_string()),
            _ => None,
        })
        .collect()
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

/// The fake's own pid and its child's, from its `/pids` reply.
async fn agent_pids(client: &Client, session: &str) -> (u32, u32) {
    let mut events = client
        .subscribe(StreamName::session(session), None)
        .await
        .unwrap();
    prompt(client, session, "/pids").await;
    let turn = until(&mut events, turn_ended).await;
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
async fn a_prompt_streams_its_answer_back() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "stream").await;
    let mut index = client
        .subscribe(StreamName::index(), Some(0))
        .await
        .unwrap();

    let session = create(&client, dir.path()).await;
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.agent, AgentId::Fake);
    assert_eq!(session.model.as_deref(), Some("fake"));
    assert!(session.models.iter().any(|model| model.id == "fake-alt"));
    assert!(session.id.starts_with("ses_"));

    let mut events = client
        .subscribe(StreamName::session(&session.id), Some(0))
        .await
        .unwrap();
    let words: Vec<String> = (1..=30).map(|n| format!("word{n}")).collect();
    let text = words.join(" ");
    let turn_id = prompt(&client, &session.id, &text).await;
    let all = until(&mut events, turn_ended).await;
    let after = until(&mut events, |event| matches!(event, Event::StateChanged(_))).await;

    assert_eq!(states(&all), ["starting", "idle", "working"]);
    assert_eq!(states(&after), ["idle"]);
    let Event::UserMessage(user) = &all[2].event else {
        panic!("expected the prompt third, got {:?}", types(&all));
    };
    assert_eq!(user.turn_id, turn_id);
    assert_eq!(user.content, [ContentBlock::text(text.clone())]);
    assert!(types(&all).contains(&"agentThoughtChunk"));
    assert!(types(&all).contains(&"usage"));
    assert_eq!(message_text(&all), format!("Echo: {text}"));
    let Event::TurnEnded(ended) = &all.last().unwrap().event else {
        unreachable!()
    };
    assert_eq!(
        (ended.turn_id.as_str(), &ended.reason),
        (turn_id.as_str(), &TurnEndReason::Completed)
    );

    // 31 words streamed 10 ms apart were stored as a few chunks of about 100 ms.
    let chunks = types(&all)
        .iter()
        .filter(|kind| **kind == "agentMessageChunk")
        .count();
    assert!((1..=words.len() / 2).contains(&chunks), "{chunks} chunks");
    for event in &all {
        if let Event::AgentMessageChunk(chunk) = &event.event {
            assert_eq!(chunk.turn_id, turn_id);
        }
    }

    // The session's totals and title, and the index summaries.
    let session = get(&client, &session.id).await;
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.turn_id, None);
    assert_eq!(
        session.title.as_deref(),
        Some(&text[..59]).map(|t| format!("{t}…")).as_deref()
    );
    let usage = session.usage.unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (30, 31));
    let summaries = until(
        &mut index,
        |event| matches!(event, Event::EntityStateChanged(changed) if changed.state == "idle"),
    )
    .await;
    assert!(matches!(&summaries[0].event, Event::EntityCreated(created)
        if created.id == session.id && created.state.as_deref() == Some("starting")));
    let list = client
        .call::<SessionList>(&SessionListParams::default())
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, session.id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_permission_request_waits_in_the_inbox_until_answered() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "permission").await;
    let session = create(&client, dir.path()).await;
    let mut index = client.subscribe(StreamName::index(), None).await.unwrap();
    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();

    prompt(&client, &session.id, "/permission").await;
    let asked = until(&mut events, |event| {
        matches!(event, Event::PermissionRequested(_))
    })
    .await;
    assert_eq!(
        types(&asked),
        [
            "userMessage",
            "stateChanged",
            "toolCall",
            "permissionRequested"
        ]
    );
    let Event::PermissionRequested(request) = &asked[3].event else {
        unreachable!()
    };
    let options: Vec<&str> = request
        .options
        .iter()
        .map(|o| o.option_id.as_str())
        .collect();
    assert_eq!(options, ["allow", "allow-always", "reject"]);
    let waiting = until(&mut events, |event| matches!(event, Event::StateChanged(_))).await;
    assert_eq!(states(&waiting), ["waiting"]);
    assert_eq!(get(&client, &session.id).await.state, SessionState::Waiting);

    // The item is in the inbox, and on the index.
    let open = client
        .call::<InboxList>(&InboxListParams {
            status: Some(InboxItemStatus::Open),
            ..InboxListParams::default()
        })
        .await
        .unwrap();
    assert_eq!(open.len(), 1);
    let item = &open[0];
    assert_eq!(item.id, request.inbox_item_id);
    assert_eq!(item.kind, InboxItemKind::Permission);
    assert_eq!(item.title, "Edit README.md");
    assert_eq!(item.source.session_id.as_deref(), Some(session.id.as_str()));
    assert_eq!(
        item.source.tool_call_id.as_deref(),
        Some(request.tool_call_id.as_str())
    );
    let added = until(&mut index, |event| {
        matches!(event, Event::InboxItemAdded(_))
    })
    .await;
    assert!(
        matches!(&added.last().unwrap().event, Event::InboxItemAdded(added) if added.item_id == item.id)
    );

    // A wrong action is refused and changes nothing.
    let wrong = client
        .call::<InboxRespond>(&InboxRespondParams {
            item_id: item.id.clone(),
            action: "maybe".into(),
            payload: None,
        })
        .await;
    assert_eq!(wrong.unwrap_err().rpc().unwrap().code, code::INVALID_PARAMS);

    // Answer it: the item is resolved, the session works again, and the turn carries on.
    let answer = InboxRespondParams {
        item_id: item.id.clone(),
        action: "allow".into(),
        payload: None,
    };
    let resolved = client.call::<InboxRespond>(&answer).await.unwrap();
    assert_eq!(resolved.status, InboxItemStatus::Resolved);
    let resolution = resolved.resolution.unwrap();
    assert_eq!(
        (resolution.action.as_str(), resolution.by.as_str()),
        ("allow", "local")
    );
    let rest = until(&mut events, turn_ended).await;
    assert_eq!(
        types(&rest)[..3],
        ["permissionResolved", "stateChanged", "toolCallUpdate"]
    );
    assert_eq!(states(&rest), ["working"]);
    assert!(rest.iter().any(|event| matches!(&event.event,
        Event::ToolCallUpdate(update) if update.status == Some(ToolCallStatus::Completed))));
    assert_eq!(message_text(&rest), "Done: I edited README.md.");
    assert!(matches!(&rest.last().unwrap().event,
        Event::TurnEnded(ended) if ended.reason == TurnEndReason::Completed));
    let resolved_on_index = until(&mut index, |event| {
        matches!(event, Event::InboxItemResolved(_))
    })
    .await;
    assert!(matches!(&resolved_on_index.last().unwrap().event,
        Event::InboxItemResolved(r) if r.status == InboxItemStatus::Resolved && r.action.as_deref() == Some("allow")));

    // Answering again is a conflict.
    assert_eq!(
        error_kind(client.call::<InboxRespond>(&answer).await),
        ErrorKind::Conflict
    );
    let item = client
        .call::<InboxGet>(&InboxItemIdParams {
            item_id: item.id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(item.status, InboxItemStatus::Resolved);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_ends_the_turn_and_expires_its_questions() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "cancel").await;
    let session = create(&client, dir.path()).await;
    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();
    let cancel = SessionIdParams {
        session_id: session.id.clone(),
    };

    // A slow turn, cancelled partway.
    prompt(&client, &session.id, "/slow").await;
    until(&mut events, |event| {
        matches!(event, Event::AgentMessageChunk(_))
    })
    .await;
    client.call::<SessionCancel>(&cancel).await.unwrap();
    let rest = until(&mut events, turn_ended).await;
    assert!(matches!(&rest.last().unwrap().event,
        Event::TurnEnded(ended) if ended.reason == TurnEndReason::Cancelled));
    assert!(!message_text(&rest).contains("tick 100"));
    until(
        &mut events,
        |event| matches!(event, Event::StateChanged(c) if c.state == SessionState::Idle),
    )
    .await;

    // Cancelling with no turn running does nothing.
    client.call::<SessionCancel>(&cancel).await.unwrap();

    // A turn waiting on a permission request: cancelling expires the request.
    prompt(&client, &session.id, "/permission").await;
    let asked = until(&mut events, |event| {
        matches!(event, Event::PermissionRequested(_))
    })
    .await;
    let Event::PermissionRequested(request) = &asked.last().unwrap().event else {
        unreachable!()
    };
    client.call::<SessionCancel>(&cancel).await.unwrap();
    let rest = until(&mut events, turn_ended).await;
    assert!(rest.iter().any(|event| matches!(&event.event,
        Event::PermissionResolved(r) if r.status == InboxItemStatus::Expired && r.inbox_item_id == request.inbox_item_id)));
    assert!(matches!(&rest.last().unwrap().event,
        Event::TurnEnded(ended) if ended.reason == TurnEndReason::Cancelled));
    let respond = client
        .call::<InboxRespond>(&InboxRespondParams {
            item_id: request.inbox_item_id.clone(),
            action: "allow".into(),
            payload: None,
        })
        .await;
    assert_eq!(error_kind(respond), ErrorKind::Conflict);

    // The session still works.
    until(
        &mut events,
        |event| matches!(event, Event::StateChanged(c) if c.state == SessionState::Idle),
    )
    .await;
    prompt(&client, &session.id, "still there?").await;
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(message_text(&turn), "Echo: still there?");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_simulated_limit_makes_the_session_limited() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "limit").await;
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
        .expect("a limitReached event");
    assert_eq!(limit.provider, "fake");
    let resets_at = limit.resets_at.expect("the fake says when it resets");
    assert!(resets_at > Timestamp::now());
    assert_eq!(states(&turn), ["working", "limited"]);
    assert!(matches!(&turn.last().unwrap().event,
        Event::TurnEnded(ended) if ended.reason == TurnEndReason::Limit));

    let limited = get(&client, &session.id).await;
    assert_eq!(limited.state, SessionState::Limited);
    assert_eq!(limited.limit.unwrap().resets_at, Some(resets_at));

    // Prompting before the reset is refused with limitReached and the reset time.
    let refused = client
        .call::<SessionPrompt>(&SessionPromptParams {
            session_id: session.id.clone(),
            content: vec![ContentBlock::text("again")],
        })
        .await
        .unwrap_err();
    let error = refused.rpc().unwrap();
    assert_eq!(error.kind(), Some(ErrorKind::LimitReached));
    assert_eq!(error.data.as_ref().unwrap().resets_at, Some(resets_at));

    // usage/get reports it.
    let usage = client.call::<UsageGet>(&Empty {}).await.unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].provider, "fake");
    assert!(usage[0].limited);
    assert_eq!(usage[0].resets_at, Some(resets_at));
    assert_eq!(usage[0].source, UsageSource::Reported);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dollar_cancel_abandons_a_slow_session_create() {
    let dir = tempfile::tempdir().unwrap();
    let (daemon, client) = start(dir.path(), "dollar-cancel").await;
    let mut params = create_params(dir.path());
    params.model = Some("fake-slow-start".into());
    let pending = client.start_call::<SessionCreate>(&params).unwrap();

    // The session exists, starting, with its agent processes running.
    let deadline = Instant::now() + WAIT;
    let starting = loop {
        let list = client
            .call::<SessionList>(&SessionListParams::default())
            .await
            .unwrap();
        if let Some(session) = list.first() {
            break session.clone();
        }
        assert!(Instant::now() < deadline, "the session never appeared");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(starting.state, SessionState::Starting);
    let deadline = Instant::now() + WAIT;
    let processes = loop {
        let found = descendants(daemon.pid());
        if found.len() >= 2 {
            break found;
        }
        assert!(Instant::now() < deadline, "the fake agent didn't start");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    // Requests run on their own tasks: the connection keeps answering meanwhile.
    client.call::<Ping>(&Empty {}).await.unwrap();

    pending.cancel().unwrap();
    let result = tokio::time::timeout(WAIT, pending.result()).await.unwrap();
    assert_eq!(error_kind(result), ErrorKind::Cancelled);
    let session = get(&client, &starting.id).await;
    assert_eq!(session.state, SessionState::Stopped);
    assert_eq!(session.reason.as_deref(), Some("cancelled while starting"));
    wait_gone(&processes).await;
}

/// M2, "Done when": stopping a session leaves no stray processes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_a_session_leaves_no_stray_processes() {
    let dir = tempfile::tempdir().unwrap();
    let (daemon, client) = start(dir.path(), "stop").await;
    let session = create(&client, dir.path()).await;
    let (agent, child) = agent_pids(&client, &session.id).await;
    assert!(is_running(agent) && is_running(child));
    let tree = descendants(daemon.pid());
    assert!(tree.contains(&agent) && tree.contains(&child), "{tree:?}");

    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();
    let stop = SessionIdParams {
        session_id: session.id.clone(),
    };
    client.call::<SessionStop>(&stop).await.unwrap();
    // Gone by the time session/stop answers, the child included.
    assert!(!is_running(agent), "the fake agent survived");
    assert!(!is_running(child), "the fake agent's child survived");
    assert!(descendants(daemon.pid()).is_empty());

    let stopped = until(&mut events, |event| matches!(event, Event::StateChanged(_))).await;
    assert!(matches!(&stopped.last().unwrap().event,
        Event::StateChanged(c) if c.state == SessionState::Stopped && c.reason.as_deref() == Some("stopped by request")));
    let session = get(&client, &session.id).await;
    assert_eq!(session.state, SessionState::Stopped);

    // Stopping again does nothing; prompting a stopped session is refused.
    client.call::<SessionStop>(&stop).await.unwrap();
    let refused = client
        .call::<SessionPrompt>(&SessionPromptParams {
            session_id: session.id.clone(),
            content: vec![ContentBlock::text("hello?")],
        })
        .await;
    assert_eq!(error_kind(refused), ErrorKind::InvalidState);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_marks_live_sessions_stopped_and_their_agents_die_with_burrowd() {
    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("restart-sessions");
    let mut daemon = Daemon::start(dir.path(), &pipe, true).await;
    let client = connect(&pipe).await;

    // One idle session, and one waiting on a permission request.
    let idle = create(&client, dir.path()).await;
    let waiting = create(&client, dir.path()).await;
    let mut events = client
        .subscribe(StreamName::session(&waiting.id), None)
        .await
        .unwrap();
    prompt(&client, &waiting.id, "/permission").await;
    let asked = until(&mut events, |event| {
        matches!(event, Event::PermissionRequested(_))
    })
    .await;
    let Event::PermissionRequested(request) = &asked.last().unwrap().event else {
        unreachable!()
    };
    until(
        &mut events,
        |event| matches!(event, Event::StateChanged(c) if c.state == SessionState::Waiting),
    )
    .await;
    let (agent, child) = agent_pids(&client, &idle.id).await;
    // Two fakes and their children, plus the hidden console host each console process gets.
    let tree = descendants(daemon.pid());
    assert!(
        tree.len() >= 4 && tree.contains(&agent) && tree.contains(&child),
        "{tree:?}"
    );
    let resume = events.resume_params();

    // Kill burrowd the hard way: the Job Objects take the agents with it.
    daemon.kill();
    drop(events);
    drop(client);
    wait_gone(&tree).await;
    assert!(!is_running(agent) && !is_running(child));

    // Restarted, both sessions are stopped, with events saying why.
    let _daemon = Daemon::start(dir.path(), &pipe, true).await;
    let client = connect(&pipe).await;
    for session in [&idle, &waiting] {
        let session = get(&client, &session.id).await;
        assert_eq!(session.state, SessionState::Stopped, "{}", session.id);
        assert_eq!(session.turn_id, None);
        assert!(session.reason.unwrap().contains("burrowd stopped"));
    }
    let mut events = client.subscribe_with(resume).await.unwrap();
    let after = until(&mut events, |event| matches!(event, Event::StateChanged(_))).await;
    assert_eq!(
        types(&after),
        ["permissionResolved", "turnEnded", "stateChanged"]
    );
    assert!(matches!(&after[0].event,
        Event::PermissionResolved(r) if r.status == InboxItemStatus::Expired && r.tool_call_id == request.tool_call_id));
    assert!(matches!(&after[1].event,
        Event::TurnEnded(ended) if ended.reason == TurnEndReason::Error));

    // The question expired, everywhere.
    let item = client
        .call::<InboxGet>(&InboxItemIdParams {
            item_id: request.inbox_item_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(item.status, InboxItemStatus::Expired);
    let open = client
        .call::<InboxList>(&InboxListParams {
            status: Some(InboxItemStatus::Open),
            ..InboxListParams::default()
        })
        .await
        .unwrap();
    assert!(open.is_empty());
    let respond = client
        .call::<InboxRespond>(&InboxRespondParams {
            item_id: request.inbox_item_id.clone(),
            action: "allow".into(),
            payload: None,
        })
        .await;
    assert_eq!(error_kind(respond), ErrorKind::Conflict);
    let mut index = client
        .subscribe(StreamName::index(), Some(0))
        .await
        .unwrap();
    let all = until(&mut index, |event| {
        matches!(event, Event::InboxItemResolved(r) if r.status == InboxItemStatus::Expired)
    })
    .await;
    assert!(!all.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_archives_and_bad_requests() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "misc").await;
    let session = create(&client, dir.path()).await;
    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();

    // setModel: the agent confirms, once.
    client
        .call::<SessionSetModel>(&SessionSetModelParams {
            session_id: session.id.clone(),
            model: "fake-alt".into(),
        })
        .await
        .unwrap();
    let changed = until(&mut events, |event| matches!(event, Event::ModelChanged(_))).await;
    assert!(matches!(&changed[0].event, Event::ModelChanged(m) if m.model == "fake-alt"));
    assert_eq!(
        get(&client, &session.id).await.model.as_deref(),
        Some("fake-alt")
    );
    client.call::<Ping>(&Empty {}).await.unwrap();
    prompt(&client, &session.id, "x").await;
    let turn = until(&mut events, turn_ended).await;
    assert!(
        !types(&turn).contains(&"modelChanged"),
        "{:?}",
        types(&turn)
    );

    // Bad requests.
    let mut terminal = create_params(dir.path());
    terminal.mode = SessionMode::Terminal;
    assert_eq!(
        error_kind(client.call::<SessionCreate>(&terminal).await),
        ErrorKind::Unavailable
    );
    let mut unknown = create_params(dir.path());
    unknown.agent = AgentId::Claude;
    assert_eq!(
        error_kind(client.call::<SessionCreate>(&unknown).await),
        ErrorKind::Unavailable
    );
    let mut nowhere = create_params(dir.path());
    nowhere.cwd = Some(dir.path().join("missing").display().to_string());
    let error = client.call::<SessionCreate>(&nowhere).await.unwrap_err();
    assert_eq!(error.rpc().unwrap().code, code::INVALID_PARAMS);
    let image = client
        .call::<SessionPrompt>(&SessionPromptParams {
            session_id: session.id.clone(),
            content: vec![ContentBlock::Resource {
                uri: "file:///x".into(),
            }],
        })
        .await
        .unwrap_err();
    assert_eq!(image.rpc().unwrap().code, code::INVALID_PARAMS);
    let missing = client
        .call::<SessionGet>(&SessionIdParams {
            session_id: "ses_nope".into(),
        })
        .await;
    assert_eq!(error_kind(missing), ErrorKind::NotFound);

    // Archiving stops the session and hides it from the default list.
    client
        .call::<SessionArchive>(&SessionIdParams {
            session_id: session.id.clone(),
        })
        .await
        .unwrap();
    let archived = get(&client, &session.id).await;
    assert!(archived.archived);
    assert_eq!(archived.state, SessionState::Stopped);
    let list = client
        .call::<SessionList>(&SessionListParams::default())
        .await
        .unwrap();
    assert!(list.iter().all(|s| s.id != session.id));
    let all = client
        .call::<SessionList>(&SessionListParams {
            include_archived: true,
            ..SessionListParams::default()
        })
        .await
        .unwrap();
    assert!(all.iter().any(|s| s.id == session.id && s.archived));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_and_projects() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, client) = start(dir.path(), "projects").await;

    let agents = client.call::<AgentList>(&Empty {}).await.unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, AgentId::Fake);
    assert!(agents[0].installed && agents[0].available);
    assert_eq!(agents[0].login.state, LoginState::LoggedIn);
    assert_eq!(
        client.call::<AgentRefresh>(&Empty {}).await.unwrap().len(),
        1
    );
    assert!(client.call::<UsageGet>(&Empty {}).await.unwrap().is_empty());

    // A plain folder: no remote. Adding it twice gives the same project.
    let folder = dir.path().join("notes");
    std::fs::create_dir(&folder).unwrap();
    let add = ProjectAddParams {
        path: folder.display().to_string(),
    };
    let notes = client.call::<ProjectAdd>(&add).await.unwrap();
    assert_eq!(notes.name, "notes");
    assert_eq!(notes.remote, None);
    assert_eq!(client.call::<ProjectAdd>(&add).await.unwrap().id, notes.id);

    // A git repository with a GitHub origin, if git is installed.
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .is_ok_and(|output| output.status.success())
    };
    if git(&["init", "-q"]) && git(&["remote", "add", "origin", "git@github.com:octo/hello.git"]) {
        let project = client
            .call::<ProjectAdd>(&ProjectAddParams {
                path: repo.display().to_string(),
            })
            .await
            .unwrap();
        let remote = project.remote.expect("the origin remote");
        assert_eq!(remote.host, Some(CodeHost::Github));
        assert_eq!(
            (remote.owner.as_deref(), remote.repo.as_deref()),
            (Some("octo"), Some("hello"))
        );
    }

    // Sessions in a project work in its folder.
    let session = client
        .call::<SessionCreate>(&SessionCreateParams {
            project_id: Some(notes.id.clone()),
            cwd: None,
            ..create_params(dir.path())
        })
        .await
        .unwrap();
    assert_eq!(session.project_id.as_deref(), Some(notes.id.as_str()));
    assert_eq!(session.cwd, notes.path);
    let in_project = client
        .call::<SessionList>(&SessionListParams {
            project_id: Some(notes.id.clone()),
            ..SessionListParams::default()
        })
        .await
        .unwrap();
    assert_eq!(in_project.len(), 1);

    // Relative paths and files are refused.
    let relative = client
        .call::<ProjectAdd>(&ProjectAddParams {
            path: "relative\\path".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(relative.rpc().unwrap().code, code::INVALID_PARAMS);

    // Remove: gone from the list, and from the index's point of view.
    let mut index = client
        .subscribe(StreamName::index(), Some(0))
        .await
        .unwrap();
    client
        .call::<ProjectRemove>(&ProjectIdParams {
            project_id: notes.id.clone(),
        })
        .await
        .unwrap();
    let removed = until(&mut index, |event| matches!(event, Event::EntityRemoved(_))).await;
    assert!(removed.iter().any(|event| matches!(&event.event,
        Event::EntityCreated(created) if created.id == notes.id && created.entity == EntityKind::Project)));
    let missing = client
        .call::<ProjectGet>(&ProjectIdParams {
            project_id: notes.id.clone(),
        })
        .await;
    assert_eq!(error_kind(missing), ErrorKind::NotFound);
    let projects = client.call::<ProjectList>(&Empty {}).await.unwrap();
    assert!(projects.iter().all(|project| project.id != notes.id));
    assert!(
        folder.is_dir(),
        "removing a project never touches its files"
    );
}
