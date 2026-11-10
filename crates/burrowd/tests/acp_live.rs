//! M2's "done when" for the ACP agents, against the real Claude Code, Codex and Cursor, through
//! `burrowd` and `burrow-client`. They spend subscription quota, so they are ignored by default
//! and their prompts are tiny. Run them by hand, one at a time:
//!
//! ```text
//! cargo test -p burrowd --test acp_live -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Each test starts `burrowd --agents=<agent>` with the default agents folder
//! (`%LOCALAPPDATA%\Burrow\agents`), installs the agent's ACP adapter there with `agent/install`
//! if it is missing, and then:
//! 1. `session/create`, in the mode Burrow picks;
//! 2. prompts "Reply with exactly: OK" and checks the answer streams back;
//! 3. asks for a file in a temporary folder, approves every permission request with
//!    `inbox/respond`, and checks the file exists;
//! 4. `session/stop`, and checks no process of the session is left.

mod common;

use std::time::{Duration, Instant};

use burrow_agents::launcher::descendants;
use burrow_client::{Client, Subscription};
use burrow_protocol::methods::{
    AgentInstall, AgentList, InboxRespond, SessionCreate, SessionPrompt, SessionStop,
};
use burrow_protocol::*;
use common::*;

/// How long a live turn may take.
const TURN_TIMEOUT: Duration = Duration::from_secs(300);
/// How long installing an adapter may take.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

const CREATE_FILE: &str = "Create a file named hello.txt in the current directory containing \
                           the word hi. Do nothing else.";
/// Cursor never asks before file edits, only before shell commands (D-024).
const CREATE_FILE_WITH_SHELL: &str = "Run a shell command that creates a file named hello.txt \
                                      in the current directory containing the word hi. Do \
                                      nothing else.";

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

/// Runs one turn to its end, printing it, and approves every permission request with its
/// first "allow once" option. Returns the reply, the number of requests, and the turn's end.
async fn turn(
    client: &Client,
    events: &mut Subscription,
    session: &str,
    text: &str,
) -> (String, usize, TurnEnded) {
    let started = Instant::now();
    let turn_id = prompt(client, session, text).await;
    eprintln!("> {text}");
    let mut reply = String::new();
    let mut permissions = 0;
    let end = tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            let event = events.next().await.expect("the subscription ended");
            match event.event {
                Event::AgentMessageChunk(chunk) => reply.push_str(&chunk.text),
                Event::ToolCall(call) => {
                    eprintln!(
                        "  tool call: {} [{}, {}]",
                        call.title, call.kind, call.status
                    )
                }
                Event::ToolCallUpdate(update) => {
                    if let Some(status) = update.status {
                        eprintln!("  tool call {}: {status}", update.tool_call_id);
                    }
                }
                Event::PermissionRequested(request) => {
                    permissions += 1;
                    let names: Vec<&str> =
                        request.options.iter().map(|o| o.name.as_str()).collect();
                    eprintln!("  permission: {:?}, options {names:?}", request.title);
                    let allow = request
                        .options
                        .iter()
                        .find(|o| o.kind == PermissionOptionKind::AllowOnce)
                        .or_else(|| request.options.first())
                        .expect("an option");
                    client
                        .call::<InboxRespond>(&InboxRespondParams {
                            item_id: request.inbox_item_id.clone(),
                            action: allow.option_id.clone(),
                            payload: None,
                        })
                        .await
                        .expect("inbox/respond");
                    eprintln!("  answered {:?} through inbox/respond", allow.name);
                }
                Event::Usage(usage) => eprintln!(
                    "  usage: {} in, {} out, cost {:?}",
                    usage.input_tokens, usage.output_tokens, usage.cost_usd
                ),
                Event::LimitReached(limit) => eprintln!("  LIMIT: {limit:?}"),
                Event::AgentModeChanged(changed) => eprintln!("  mode: {}", changed.agent_mode),
                Event::TurnEnded(ended) if ended.turn_id == turn_id => return ended,
                _ => {}
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the turn didn't end within {TURN_TIMEOUT:?}"));
    eprintln!(
        "  reply: {reply:?}\n  ended: {} {:?} after {:.1} s",
        end.reason,
        end.message,
        started.elapsed().as_secs_f64()
    );
    (reply, permissions, end)
}

async fn live(agent: AgentId, file_prompt: &str) {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let pipe = unique_pipe(&format!("live-{agent}"));
    let daemon = Daemon::start_with(dir.path(), &pipe, &[&format!("--agents={agent}")]).await;
    let client = connect(&pipe).await;

    let mut info = client.call::<AgentList>(&Empty {}).await.unwrap().remove(0);
    if info
        .adapter
        .as_ref()
        .is_some_and(|adapter| !adapter.installed)
    {
        eprintln!("installing {agent}'s adapter with agent/install...");
        let started = Instant::now();
        info = tokio::time::timeout(
            INSTALL_TIMEOUT,
            client.call::<AgentInstall>(&AgentInstallParams {
                agent: agent.clone(),
            }),
        )
        .await
        .expect("the install timed out")
        .expect("agent/install");
        eprintln!("  installed in {:.1} s", started.elapsed().as_secs_f64());
    }
    eprintln!(
        "[{agent}] {} {}: available {}, {:?}, adapter {:?}, default mode {:?}",
        info.name,
        info.version.as_deref().unwrap_or("-"),
        info.available,
        info.problem,
        info.adapter
            .as_ref()
            .map(|a| format!("{} {}", a.package, a.version)),
        info.default_agent_mode
    );
    assert!(info.available, "{:?}", info.problem);

    let started = Instant::now();
    let session = client
        .call::<SessionCreate>(&SessionCreateParams {
            project_id: None,
            agent: agent.clone(),
            model: None,
            mode: SessionMode::Structured,
            agent_mode: None,
            cwd: Some(work.display().to_string()),
            title: None,
            cols: None,
            rows: None,
        })
        .await
        .expect("session/create");
    eprintln!(
        "  session {} ready in {:.1} s: model {:?} ({} offered), mode {:?} of {:?}",
        session.id,
        started.elapsed().as_secs_f64(),
        session.model,
        session.models.len(),
        session.agent_mode,
        session
            .agent_modes
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(session.state, SessionState::Idle);
    let tree = descendants(daemon.pid());
    eprintln!("  process tree: {} processes", tree.len());
    assert!(!tree.is_empty());

    let mut events = client
        .subscribe(StreamName::session(&session.id), None)
        .await
        .unwrap();
    let (reply, _, end) = turn(&client, &mut events, &session.id, "Reply with exactly: OK").await;
    assert_eq!(end.reason, TurnEndReason::Completed, "{:?}", end.message);
    assert!(reply.contains("OK"), "the reply was {reply:?}");

    let (_, permissions, end) = turn(&client, &mut events, &session.id, file_prompt).await;
    assert_eq!(end.reason, TurnEndReason::Completed, "{:?}", end.message);
    assert!(permissions > 0, "the agent didn't ask for permission");
    let file = work.join("hello.txt");
    assert!(file.exists(), "hello.txt wasn't created");
    eprintln!("  hello.txt: {:?}", std::fs::read_to_string(&file).unwrap());

    let started = Instant::now();
    client
        .call::<SessionStop>(&SessionIdParams {
            session_id: session.id.clone(),
        })
        .await
        .unwrap();
    let left = descendants(daemon.pid());
    eprintln!(
        "  stopped in {:.1} s; processes left: {left:?}",
        started.elapsed().as_secs_f64()
    );
    assert!(left.is_empty(), "processes outlived the session: {left:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "talks to the real agent and spends quota"]
async fn claude_end_to_end() {
    live(AgentId::Claude, CREATE_FILE).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "talks to the real agent and spends quota"]
async fn codex_end_to_end() {
    live(AgentId::Codex, CREATE_FILE).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "talks to the real agent and spends quota"]
async fn cursor_end_to_end() {
    live(AgentId::Cursor, CREATE_FILE_WITH_SHELL).await;
}
