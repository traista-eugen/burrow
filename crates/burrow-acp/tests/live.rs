//! Live tests against the real agents. They spend subscription quota, so
//! they are ignored by default and their prompts are tiny. Run them by hand:
//!
//! ```text
//! cargo test -p burrow-acp --test live -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! The npm adapters are looked up under `BURROW_ACP_AGENTS_DIR` (default
//! `%LOCALAPPDATA%\Burrow\agents`), installed with
//! [`NpmAdapter::install_command`](burrow_acp::launch::NpmAdapter::install_command).
//! Each agent uses the login of its own CLI.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow_acp::launch::{self, KnownAgent};
use burrow_acp::{
    AcpClient, AcpEvent, AcpEvents, ConnectOptions, Content, SessionEvent, StopReason, TurnEnd,
};
use tokio::io::AsyncBufReadExt;

const TURN_TIMEOUT: Duration = Duration::from_secs(240);

fn agents_dir() -> PathBuf {
    std::env::var_os("BURROW_ACP_AGENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default())
                .join("Burrow")
                .join("agents")
        })
}

struct Live {
    client: AcpClient,
    events: AcpEvents,
    child: tokio::process::Child,
    stderr: Arc<Mutex<Vec<String>>>,
    session_id: String,
}

impl Live {
    async fn start(agent: KnownAgent, cwd: &Path) -> Live {
        let command = launch::recommended_command(agent, &agents_dir()).unwrap_or_else(|| {
            panic!(
                "{agent:?} isn't installed (agents dir: {})",
                agents_dir().display()
            )
        });
        eprintln!("[{agent:?}] {command}");
        let spawned = launch::spawn(&command, cwd).expect("spawn");
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let lines_out = stderr.clone();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(spawned.stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                lines_out.lock().unwrap().push(line);
            }
        });
        let (client, events) =
            AcpClient::connect_tokio(spawned.stdin, spawned.stdout, ConnectOptions::default())
                .await
                .expect("initialize");
        let session = client.new_session(cwd).await.expect("session/new");
        eprintln!(
            "[{agent:?}] {} {} session {} mode {:?} model {:?}",
            client.agent().name.as_deref().unwrap_or("(no name)"),
            client.agent().version.as_deref().unwrap_or("(no version)"),
            session.session_id,
            session.modes.as_ref().map(|modes| &modes.current),
            session.models.as_ref().map(|models| &models.current),
        );
        Live {
            client,
            events,
            child: spawned.child,
            stderr,
            session_id: session.session_id,
        }
    }

    /// Prompts and drives the turn: approves every permission request once.
    /// Returns the reply text, the number of permission requests, and how
    /// the turn ended.
    async fn turn(&mut self, prompt: &str) -> (String, usize, TurnEnd) {
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        let prompt = prompt.to_owned();
        let prompting =
            tokio::spawn(async move { client.prompt(&session_id, [prompt.into()]).await });
        let mut text = String::new();
        let mut permissions = 0;
        let end = tokio::time::timeout(TURN_TIMEOUT, async {
            loop {
                match self.events.recv().await {
                    Some(AcpEvent::Session { event, .. }) => match event {
                        SessionEvent::AgentMessageChunk {
                            content: Content::Text(chunk),
                            ..
                        } => text.push_str(&chunk),
                        SessionEvent::ToolCall(call) => {
                            eprintln!(
                                "  tool call: {} ({:?}, {:?})",
                                call.title, call.kind, call.status
                            );
                        }
                        SessionEvent::LimitReached(info) => eprintln!("  LIMIT: {info}"),
                        SessionEvent::TurnEnded(end) => return end,
                        _ => {}
                    },
                    Some(AcpEvent::Permission(request)) => {
                        permissions += 1;
                        eprintln!(
                            "  permission: {:?} options {:?}",
                            request.tool_call.title,
                            request
                                .options
                                .iter()
                                .map(|option| &option.name)
                                .collect::<Vec<_>>()
                        );
                        request.allow_once().expect("allow");
                    }
                    Some(AcpEvent::Closed { error }) => {
                        panic!("the agent closed the connection: {error:?}")
                    }
                    None => panic!("the event stream ended"),
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            self.dump_stderr();
            panic!("the turn didn't end within {TURN_TIMEOUT:?}")
        });
        let result = prompting.await.unwrap();
        eprintln!(
            "  reply: {text:?}\n  end: {end:?}\n  prompt result: {:?}",
            result.as_ref().map(|r| &r.stop_reason)
        );
        (text, permissions, end)
    }

    fn dump_stderr(&self) {
        let lines = self.stderr.lock().unwrap();
        for line in lines.iter().rev().take(30).rev() {
            eprintln!("  [stderr] {line}");
        }
    }

    async fn stop(mut self) {
        self.client.stop().await;
        match tokio::time::timeout(Duration::from_secs(15), self.child.wait()).await {
            Ok(status) => eprintln!("  exited: {status:?}"),
            Err(_) => {
                eprintln!("  didn't exit after stdin closed; killing");
                let _ = self.child.kill().await;
            }
        }
    }
}

async fn reply_ok(agent: KnownAgent) {
    let dir = tempfile::tempdir().unwrap();
    let mut live = Live::start(agent, dir.path()).await;
    let (text, _, end) = live.turn("Reply with exactly: OK").await;
    if !text.contains("OK") {
        live.dump_stderr();
    }
    assert!(
        matches!(
            end,
            TurnEnd::Stopped {
                reason: StopReason::EndTurn,
                ..
            }
        ),
        "turn ended with {end:?}"
    );
    assert!(text.contains("OK"), "reply was {text:?}");
    live.stop().await;
}

const CREATE_FILE: &str = "Create a file named hello.txt in the current directory containing the word hi. Do nothing else.";

/// Asks the agent to create a file, in a mode where that needs permission,
/// approves, and checks the file exists.
async fn create_file_with_permission(agent: KnownAgent, asking_mode: Option<&str>, prompt: &str) {
    let dir = tempfile::tempdir().unwrap();
    let mut live = Live::start(agent, dir.path()).await;
    if let Some(mode) = asking_mode {
        live.client
            .set_mode(&live.session_id, mode)
            .await
            .expect("set_mode");
    }
    let (_, permissions, end) = live.turn(prompt).await;
    let file = dir.path().join("hello.txt");
    if !file.exists() || permissions == 0 {
        live.dump_stderr();
    }
    assert!(
        matches!(end, TurnEnd::Stopped { .. }),
        "turn ended with {end:?}"
    );
    assert!(permissions > 0, "the agent didn't ask for permission");
    assert!(file.exists(), "hello.txt wasn't created");
    eprintln!(
        "  hello.txt: {:?}",
        std::fs::read_to_string(&file).unwrap_or_default()
    );
    live.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to the real agent and spends quota"]
async fn claude_replies() {
    reply_ok(KnownAgent::ClaudeCode).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to the real agent and spends quota"]
async fn claude_permission_round_trip() {
    // "default" is Claude Code's "always ask before making changes" mode.
    create_file_with_permission(KnownAgent::ClaudeCode, Some("default"), CREATE_FILE).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to the real agent and spends quota"]
async fn codex_replies() {
    reply_ok(KnownAgent::Codex).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to the real agent and spends quota"]
async fn codex_permission_round_trip() {
    // "read-only" requires approval to edit files.
    create_file_with_permission(KnownAgent::Codex, Some("read-only"), CREATE_FILE).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to the real agent and spends quota"]
async fn cursor_replies() {
    reply_ok(KnownAgent::Cursor).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to the real agent and spends quota"]
async fn cursor_permission_round_trip() {
    // Cursor's modes (agent, plan, ask) never ask before file edits: in
    // "agent" mode it edits directly. It does ask before shell commands.
    create_file_with_permission(
        KnownAgent::Cursor,
        None,
        "Run a shell command that creates a file named hello.txt in the current directory          containing the word hi. Do nothing else.",
    )
    .await;
}
