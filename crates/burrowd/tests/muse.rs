//! Muse Code end to end: the real `burrowd` running real `muse serve` sessions, driven through
//! `burrow-client`.
//!
//! The tests run Muse with its free `echo` provider (`BURROW_MUSE_PROVIDER=echo`), which
//! replies `echo: <prompt>` and spends nothing. Echo can't call tools, so the approval round
//! trip is tested against a recorded transcript in `burrow-agents` (`src/msp/tests.rs`), and
//! live by `a_live_meta_session`, which spends real tokens and is ignored:
//!
//! ```powershell
//! cargo test -p burrowd --test muse -- --ignored --nocapture
//! ```
//!
//! Each test skips (passing) when Muse isn't installed.

mod common;

use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use burrow_agents::launcher::{descendants, is_running};
use burrow_agents::msp::{NO_SANDBOX_ENV, PROVIDER_ENV, muse_binary};
use burrow_client::{CallError, Client, ConnectError, Subscription};
use burrow_protocol::methods::{
    AgentList, InboxList, InboxRespond, SessionCancel, SessionCreate, SessionPrompt,
    SessionSetModel, SessionStop, UsageGet,
};
use burrow_protocol::*;
use common::{WAIT, burrowd, connect, options, read_logs, unique_pipe};

/// How long a live `meta` turn may take.
const LIVE_TURN: Duration = Duration::from_secs(240);

/// `burrowd --agents=muse`, with Muse's provider set by `BURROW_MUSE_PROVIDER`. Killed when
/// dropped.
struct Daemon {
    child: Child,
    data_dir: PathBuf,
}

impl Daemon {
    /// Starts `burrowd` offering only Muse, on `provider`, and connects. With `no_sandbox`,
    /// Muse runs without its sandbox (`BURROW_MUSE_DISABLE_SANDBOX=1`), so it asks before shell
    /// commands. `None` when Muse isn't installed.
    async fn start(
        data_dir: &Path,
        test: &str,
        provider: &str,
        no_sandbox: bool,
    ) -> Option<(Self, Client)> {
        if muse_binary().is_none() {
            eprintln!("skipping: Muse Code isn't installed");
            return None;
        }
        let pipe = unique_pipe(test);
        let mut command = burrowd();
        command
            .arg("--pipe")
            .arg(&pipe)
            .arg("--data-dir")
            .arg(data_dir)
            .arg("--agents=muse")
            .env(PROVIDER_ENV, provider)
            .env_remove(NO_SANDBOX_ENV);
        if no_sandbox {
            command.env(NO_SANDBOX_ENV, "1");
        }
        let mut daemon = Self {
            child: command.spawn().expect("start burrowd"),
            data_dir: data_dir.to_owned(),
        };
        let deadline = Instant::now() + WAIT;
        loop {
            match Client::connect(options(&pipe)).await {
                Ok(_) => break,
                Err(ConnectError::NotRunning { .. } | ConnectError::Busy { .. }) => {}
                Err(error) => panic!("connecting to burrowd failed: {error}"),
            }
            if let Some(status) = daemon.child.try_wait().unwrap() {
                panic!("burrowd exited with {status}\n{}", daemon.logs());
            }
            assert!(
                Instant::now() < deadline,
                "burrowd didn't start\n{}",
                daemon.logs()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let client = connect(&pipe).await;
        // Wait for detection, which runs Muse too, so it doesn't count as a stray process.
        client
            .call::<AgentList>(&Empty {})
            .await
            .expect("agent/list");
        Some((daemon, client))
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn logs(&self) -> String {
        read_logs(&self.data_dir)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn create(client: &Client, cwd: &Path) -> Session {
    client
        .call::<SessionCreate>(&SessionCreateParams {
            project_id: None,
            agent: AgentId::Muse,
            model: None,
            mode: SessionMode::Structured,
            cwd: Some(cwd.display().to_string()),
            title: None,
            agent_mode: None,
        })
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

/// Events until (and including) the first one `done` accepts, waiting at most `wait`.
async fn until_within(
    subscription: &mut Subscription,
    wait: Duration,
    done: impl Fn(&Event) -> bool,
) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    tokio::time::timeout(wait, async {
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
    .unwrap_or_else(|_| panic!("timed out; got:\n{}", describe(&events)));
    events
}

async fn until(subscription: &mut Subscription, done: impl Fn(&Event) -> bool) -> Vec<StreamEvent> {
    until_within(subscription, WAIT, done).await
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

/// A readable account of a turn's events: text, tool calls and their output, usage.
fn describe(events: &[StreamEvent]) -> String {
    let mut lines = Vec::new();
    for event in events {
        let line = match &event.event {
            Event::AgentMessageChunk(chunk) => format!("  says: {:?}", chunk.text),
            Event::AgentThoughtChunk(_) => continue,
            Event::ToolCall(call) => format!(
                "  tool call {}: {} [{}] {}",
                call.tool_call_id, call.title, call.kind, call.status
            ),
            Event::ToolCallUpdate(update) => format!(
                "  tool update {}: {:?} {:?}",
                update.tool_call_id, update.status, update.content
            ),
            Event::PermissionRequested(request) => format!(
                "  asks: {} {:?}",
                request.title,
                request
                    .options
                    .iter()
                    .map(|option| option.option_id.as_str())
                    .collect::<Vec<_>>()
            ),
            Event::Usage(usage) => format!(
                "  usage: {} in, {} out",
                usage.input_tokens, usage.output_tokens
            ),
            Event::TurnEnded(ended) => format!("  turn ended {} {:?}", ended.reason, ended.message),
            other => format!("  {}", other.type_name()),
        };
        lines.push(line);
    }
    lines.join("\n")
}

fn end_reason(events: &[StreamEvent]) -> TurnEndReason {
    match events.last().map(|event| &event.event) {
        Some(Event::TurnEnded(ended)) => ended.reason.clone(),
        other => panic!("expected turnEnded last, got {other:?}"),
    }
}

fn error_kind(result: Result<impl std::fmt::Debug, CallError>) -> ErrorKind {
    result
        .expect_err("expected an error")
        .rpc()
        .and_then(ErrorObject::kind)
        .expect("a Burrow error kind")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_muse_prompt_streams_its_echo_back() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let Some((_daemon, client)) = Daemon::start(dir.path(), "muse-echo", "echo", false).await
    else {
        return;
    };

    let session = create(&client, workspace.path()).await;
    assert_eq!(session.agent, AgentId::Muse);
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.model, None, "echo has no model");
    assert!(session.models.is_empty(), "echo can't switch models");

    let mut events = client
        .subscribe(StreamName::session(&session.id), Some(0))
        .await
        .unwrap();
    let text = "Hello from Burrow";
    let turn_id = prompt(&client, &session.id, text).await;
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(states(&turn), ["starting", "idle", "working"]);
    assert_eq!(message_text(&turn), format!("echo: {text}"));
    for event in &turn {
        if let Event::AgentMessageChunk(chunk) = &event.event {
            assert_eq!(chunk.turn_id, turn_id);
        }
    }
    assert!(
        turn.iter()
            .any(|event| matches!(&event.event, Event::Usage(usage)
            if usage.turn_id.as_deref() == Some(turn_id.as_str()))),
        "{:?}",
        types(&turn)
    );
    assert_eq!(end_reason(&turn), TurnEndReason::Completed);

    // Muse refuses a model on an echo session; the session carries on.
    let refused = client
        .call::<SessionSetModel>(&SessionSetModelParams {
            session_id: session.id.clone(),
            model: "muse-spark-1.3".into(),
        })
        .await;
    assert_eq!(error_kind(refused), ErrorKind::InvalidState);
    prompt(&client, &session.id, "again").await;
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(message_text(&turn), "echo: again");

    // usage/get: Muse was asked, and an empty reading isn't a limit.
    let usage = client.call::<UsageGet>(&Empty {}).await.unwrap();
    let muse = usage
        .iter()
        .find(|usage| usage.provider == "muse")
        .expect("Muse's usage");
    assert_eq!(muse.agent, Some(AgentId::Muse));
    assert!(!muse.limited);
    assert_eq!(muse.source, UsageSource::Reported);
    assert!(
        muse.message
            .as_deref()
            .unwrap()
            .contains("no subscription usage"),
        "{muse:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_ends_a_muse_turn() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let Some((_daemon, client)) = Daemon::start(dir.path(), "muse-cancel", "echo", false).await
    else {
        return;
    };
    let session = create(&client, workspace.path()).await;
    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();
    let cancel = SessionIdParams {
        session_id: session.id.clone(),
    };

    // An echo turn lasts tens of milliseconds, most of it Muse's end-of-turn reminders, so the
    // cancel usually lands inside it. When echo wins the race the turn completes; try again.
    let mut cancelled = false;
    for _ in 0..10 {
        prompt(&client, &session.id, "interrupt me").await;
        client.call::<SessionCancel>(&cancel).await.unwrap();
        let turn = until(&mut events, turn_ended).await;
        match end_reason(&turn) {
            TurnEndReason::Cancelled => {
                cancelled = true;
                break;
            }
            TurnEndReason::Completed => {}
            other => panic!("the turn ended {other}: {:?}", turn.last()),
        }
    }
    assert!(cancelled, "no turn ended cancelled in ten tries");

    // Cancelling with no turn running does nothing, and the session still works.
    client.call::<SessionCancel>(&cancel).await.unwrap();
    prompt(&client, &session.id, "still there?").await;
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(message_text(&turn), "echo: still there?");
    assert_eq!(end_reason(&turn), TurnEndReason::Completed);
}

/// Stopping a Muse session leaves nothing running: `muse serve` and its console host are gone
/// by the time `session/stop` answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_a_muse_session_leaves_no_stray_processes() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let Some((daemon, client)) = Daemon::start(dir.path(), "muse-stop", "echo", false).await else {
        return;
    };
    assert!(
        descendants(daemon.pid()).is_empty(),
        "detection left processes"
    );
    let session = create(&client, workspace.path()).await;
    let tree = descendants(daemon.pid());
    assert!(!tree.is_empty(), "muse serve isn't running");

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
    let survivors: Vec<u32> = tree
        .iter()
        .copied()
        .filter(|&pid| is_running(pid))
        .collect();
    assert!(survivors.is_empty(), "still running: {survivors:?}");
    assert!(descendants(daemon.pid()).is_empty());
    let stopped = until(&mut events, |event| matches!(event, Event::StateChanged(_))).await;
    assert!(matches!(&stopped.last().unwrap().event,
        Event::StateChanged(c) if c.state == SessionState::Stopped && c.reason.as_deref() == Some("stopped by request")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detection_runs_the_versioned_binary() {
    let dir = tempfile::tempdir().unwrap();
    let Some((_daemon, client)) = Daemon::start(dir.path(), "muse-detect", "echo", false).await
    else {
        return;
    };
    let agents = client.call::<AgentList>(&Empty {}).await.unwrap();
    let [muse] = agents.as_slice() else {
        panic!("{agents:?}")
    };
    println!("{muse:#?}");
    assert_eq!(muse.id, AgentId::Muse);
    assert_eq!(muse.integration, Integration::Msp);
    assert!(muse.installed);
    assert!(
        muse.version
            .as_deref()
            .is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()))
    );
    let path = muse.path.as_deref().unwrap();
    assert!(
        path.contains("muse-bin-") || !path.ends_with(".cmd"),
        "not the versioned binary: {path}"
    );
    assert!(muse.available || muse.login.state == LoginState::LoggedOut);
}

/// Live, against Muse's default `meta` provider: about 25k input tokens per model call, three
/// or four calls in all. A prompt that streams back, an approval answered from the inbox, and a
/// clean stop.
///
/// Muse asks before a shell command only when it can't sandbox it, so this test turns the
/// sandbox off and approves exactly the one command it asked for. Reads, web fetches and
/// sandboxed commands run without a question; a write outside the workspace is refused without
/// one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "spends meta tokens; run by hand"]
async fn a_live_meta_session() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let Some((daemon, client)) = Daemon::start(dir.path(), "muse-live", "meta", true).await else {
        return;
    };
    let agents = client.call::<AgentList>(&Empty {}).await.unwrap();
    println!("detected: {:#?}", agents[0]);

    let session = create(&client, workspace.path()).await;
    println!(
        "session: model {:?}, {} models",
        session.model,
        session.models.len()
    );
    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();

    // 1. A prompt that streams back.
    prompt(&client, &session.id, "Reply with exactly: OK").await;
    let turn = until_within(&mut events, LIVE_TURN, turn_ended).await;
    println!("turn 1: {:?}", types(&turn));
    let reply = message_text(&turn);
    println!("turn 1 reply: {reply:?}");
    assert_eq!(
        end_reason(&turn),
        TurnEndReason::Completed,
        "{:?}",
        turn.last()
    );
    assert!(reply.contains("OK"), "{reply:?}");

    // 2. A shell command, which Muse asks about, approved from the inbox.
    prompt(
        &client,
        &session.id,
        "Use your shell tool to run exactly this command once: \
         Set-Content -Path burrow-live.txt -Value burrow. \
         If it fails, don't retry and don't try anything else. Then reply with one short sentence.",
    )
    .await;
    // Give up early, to save tokens, if the model runs tools without being asked about them.
    let calls = std::cell::Cell::new(0);
    let asked = until_within(&mut events, LIVE_TURN, |event| {
        if matches!(event, Event::ToolCall(_)) {
            calls.set(calls.get() + 1);
        }
        calls.get() > 3 || matches!(event, Event::PermissionRequested(_) | Event::TurnEnded(_))
    })
    .await;
    println!("turn 2 until the question:\n{}", describe(&asked));
    let Some(Event::PermissionRequested(request)) = asked.last().map(|event| &event.event) else {
        let _ = client
            .call::<SessionCancel>(&SessionIdParams {
                session_id: session.id.clone(),
            })
            .await;
        panic!("Muse didn't ask:\n{}", describe(&asked));
    };
    println!("asked: {} {:?}", request.title, request.options);
    // Approve only the command the test asked for: it runs unsandboxed.
    assert!(
        request.title.starts_with("Run `Set-Content")
            && request.title.contains("burrow-live.txt")
            && !request.title.contains(['&', ';', '|']),
        "{}",
        request.title
    );
    let open = client
        .call::<InboxList>(&InboxListParams {
            status: Some(InboxItemStatus::Open),
            ..InboxListParams::default()
        })
        .await
        .unwrap();
    let item = open
        .iter()
        .find(|item| item.id == request.inbox_item_id)
        .expect("the question is in the inbox");
    let allow = request
        .options
        .iter()
        .find(|option| option.kind == PermissionOptionKind::AllowOnce)
        .expect("an allow-once option");
    client
        .call::<InboxRespond>(&InboxRespondParams {
            item_id: item.id.clone(),
            action: allow.option_id.clone(),
            payload: None,
        })
        .await
        .unwrap();
    let turn = until_within(&mut events, LIVE_TURN, turn_ended).await;
    println!("turn 2 after the answer:\n{}", describe(&turn));
    assert_eq!(
        end_reason(&turn),
        TurnEndReason::Completed,
        "{:?}",
        turn.last()
    );
    // The approved command ran and wrote its file.
    let ran = turn.iter().find_map(|event| match &event.event {
        Event::ToolCallUpdate(update) if update.tool_call_id == request.tool_call_id => update
            .status
            .clone()
            .filter(|status| *status != ToolCallStatus::InProgress),
        _ => None,
    });
    assert_eq!(ran, Some(ToolCallStatus::Completed));
    let written = std::fs::read_to_string(workspace.path().join("burrow-live.txt"))
        .expect("the command wrote the file");
    assert!(written.contains("burrow"), "{written:?}");

    // 3. Stop: nothing left running.
    let tree = descendants(daemon.pid());
    client
        .call::<SessionStop>(&SessionIdParams {
            session_id: session.id.clone(),
        })
        .await
        .unwrap();
    assert!(tree.iter().all(|&pid| !is_running(pid)), "{tree:?}");
    assert!(descendants(daemon.pid()).is_empty());
    let usage = client.call::<UsageGet>(&Empty {}).await.unwrap();
    println!("usage/get: {usage:#?}");
}
