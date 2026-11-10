//! A scripted fake agent, for tests and for trying Burrow by hand without spending quota.
//!
//! The fake runs as a real child process, `burrow-fake-agent` ([`agent`]), in a Job Object like
//! any agent, and starts a child of its own. So the launcher and the "no stray processes" check
//! get real coverage. [`FakeAdapter`] drives it over a trivial line protocol ([`wire`]); the
//! script is described in [`agent`].
//!
//! `burrowd --dev` registers the fake as agent `fake`, looking for `burrow-fake-agent.exe` next
//! to its own executable. In a terminal session, agent `fake` runs the same program as a fake
//! TUI ([`tui`]).

pub mod agent;
pub mod content;
pub mod tui;
pub mod wire;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use burrow_protocol::{
    AgentId, AgentInfo, AgentLogin, ContentBlock, Integration, LoginState, ModelInfo,
    PermissionOption, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, Timestamp, ToolCall,
    ToolCallContent, ToolCallUpdate,
};
use futures::future::BoxFuture;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, warn};

use self::wire::{FromAgent, ToAgent};
use crate::adapter::{
    AgentAdapter, AgentError, AgentEvent, AgentSession, PermissionId, PermissionOutcome,
    PermissionReply, PermissionRequest, SessionConfig, StartedSession,
};
use crate::launcher::{self, LaunchSpec, ProcessHandle};

/// The fake agent's executable name, without the platform's extension.
pub const PROGRAM: &str = "burrow-fake-agent";
/// How long a stopping fake gets to exit by itself after stdin closes.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// Drives `burrow-fake-agent`.
#[derive(Clone, Debug)]
pub struct FakeAdapter {
    program: PathBuf,
}

impl FakeAdapter {
    /// Runs the fake agent at `program`.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// Runs the `burrow-fake-agent` that sits next to the running executable, as it does in
    /// Cargo's target directory and in a Burrow install.
    pub fn next_to_current_exe() -> io::Result<Self> {
        let exe = std::env::current_exe()?;
        Ok(Self::new(exe.with_file_name(format!(
            "{PROGRAM}{}",
            std::env::consts::EXE_SUFFIX
        ))))
    }

    /// Where the fake agent's executable is expected.
    pub fn program(&self) -> &PathBuf {
        &self.program
    }
}

impl AgentAdapter for FakeAdapter {
    fn id(&self) -> AgentId {
        AgentId::Fake
    }

    fn name(&self) -> &str {
        "Fake agent"
    }

    fn integration(&self) -> Integration {
        Integration::Fake
    }

    fn detect(&self) -> BoxFuture<'_, AgentInfo> {
        Box::pin(async move {
            let installed = self.program.is_file();
            AgentInfo {
                id: self.id(),
                name: self.name().to_owned(),
                integration: self.integration(),
                installed,
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                path: Some(self.program.display().to_string()),
                login: AgentLogin {
                    state: LoginState::LoggedIn,
                    account: None,
                    detail: Some("scripted; spends no quota".into()),
                },
                models: agent::MODELS
                    .iter()
                    .map(|id| ModelInfo {
                        id: (*id).to_owned(),
                        name: None,
                    })
                    .collect(),
                agent_modes: Vec::new(),
                available: installed,
                problem: (!installed).then(|| format!("{} doesn't exist", self.program.display())),
                adapter: None,
                default_agent_mode: None,
                detected_at: Timestamp::now(),
            }
        })
    }

    fn start(&self, config: SessionConfig) -> BoxFuture<'_, Result<StartedSession, AgentError>> {
        Box::pin(start(self.program.clone(), config))
    }

    /// The fake TUI: `burrow-fake-agent --tui`.
    fn terminal(&self, config: &SessionConfig) -> Result<LaunchSpec, AgentError> {
        if !self.program.is_file() {
            return Err(AgentError::Unavailable(format!(
                "{} doesn't exist",
                self.program.display()
            )));
        }
        let spec = LaunchSpec::new(&self.program, ["--tui"], "fake");
        Ok(crate::terminal::for_session(spec, config))
    }
}

async fn start(program: PathBuf, config: SessionConfig) -> Result<StartedSession, AgentError> {
    let mut args = Vec::new();
    if let Some(model) = &config.model {
        args.push("--model".to_owned());
        args.push(model.clone());
    }
    let spec =
        LaunchSpec::new(&program, args, format!("fake {}", config.session_id)).cwd(&config.cwd);
    let mut process = launcher::spawn(&spec).map_err(|error| {
        AgentError::Unavailable(format!("can't start {}: {error}", program.display()))
    })?;
    let handle = process.handle();
    let stdin = process.take_stdin().expect("stdin is piped");
    let stdout = process.take_stdout().expect("stdout is piped");
    // If this future is dropped from here on, `process` and `handle` go with it, the job's last
    // handle closes, and the whole tree dies.
    let mut lines = BufReader::new(stdout).lines();
    let (model, models) = loop {
        match lines.next_line().await {
            Ok(Some(line)) => match serde_json::from_str::<FromAgent>(&line) {
                Ok(FromAgent::Ready { model, models, .. }) => break (model, models),
                Ok(other) => debug!(?other, "fake agent spoke before it was ready"),
                Err(error) => warn!(%error, line, "fake agent sent a bad line"),
            },
            Ok(None) | Err(_) => {
                let exit = tokio::time::timeout(STOP_GRACE, handle.exited()).await;
                let exit = exit.map_or_else(|_| "still running".to_owned(), |e| e.description);
                return Err(AgentError::Unavailable(format!(
                    "the fake agent stopped before it was ready ({exit}): {}",
                    handle.stderr_tail().join(" | ")
                )));
            }
        }
    };
    drop(process);

    let writer = Arc::new(Writer(Mutex::new(Some(stdin))));
    let stopping = Arc::new(AtomicBool::new(false));
    let (events, receiver) = mpsc::unbounded_channel();
    tokio::spawn(read_loop(
        lines,
        events,
        writer.clone(),
        handle.clone(),
        stopping.clone(),
    ));
    Ok(StartedSession {
        session: Box::new(FakeSession {
            writer,
            handle,
            stopping,
        }),
        events: receiver,
        model: Some(model),
        models: models
            .into_iter()
            .map(|id| ModelInfo { id, name: None })
            .collect(),
        agent_session_id: None,
    })
}

/// The fake's stdin, shared by the session handle and the permission tasks.
struct Writer(Mutex<Option<ChildStdin>>);

impl Writer {
    async fn send(&self, message: &ToAgent) -> Result<(), AgentError> {
        let mut line = serde_json::to_string(message).expect("messages serialize");
        line.push('\n');
        let mut stdin = self.0.lock().await;
        let Some(pipe) = stdin.as_mut() else {
            return Err(AgentError::InvalidState(
                "the fake agent is stopping".into(),
            ));
        };
        let written = async {
            pipe.write_all(line.as_bytes()).await?;
            pipe.flush().await
        };
        written
            .await
            .map_err(|error| AgentError::Failed(format!("writing to the fake agent: {error}")))
    }

    async fn close(&self) {
        self.0.lock().await.take();
    }
}

async fn read_loop(
    mut lines: Lines<BufReader<ChildStdout>>,
    events: mpsc::UnboundedSender<AgentEvent>,
    writer: Arc<Writer>,
    handle: ProcessHandle,
    stopping: Arc<AtomicBool>,
) {
    // The fake's permission request ids, and the ids of the replies they became.
    let mut asked: HashMap<String, PermissionId> = HashMap::new();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                debug!(%error, "reading from the fake agent failed");
                break;
            }
        };
        let message = match serde_json::from_str::<FromAgent>(&line) {
            Ok(message) => message,
            Err(error) => {
                warn!(%error, line, "fake agent sent a bad line");
                continue;
            }
        };
        let event = match message {
            FromAgent::Ready { .. } => continue,
            FromAgent::Chunk { text } => AgentEvent::MessageChunk(text),
            FromAgent::Thought { text } => AgentEvent::ThoughtChunk(text),
            FromAgent::ToolCall {
                id,
                title,
                kind,
                status,
            } => AgentEvent::ToolCall(ToolCall {
                turn_id: None,
                tool_call_id: id,
                title,
                kind,
                status,
                locations: Vec::new(),
                content: Vec::new(),
                raw_input: None,
            }),
            FromAgent::ToolCallUpdate { id, status, text } => {
                AgentEvent::ToolCallUpdate(ToolCallUpdate {
                    tool_call_id: id,
                    status: Some(status),
                    title: None,
                    kind: None,
                    locations: None,
                    content: text.map(|text| {
                        vec![ToolCallContent::Content {
                            content: ContentBlock::text(text),
                        }]
                    }),
                })
            }
            FromAgent::Call { call } => AgentEvent::ToolCall(call),
            FromAgent::CallUpdate { update } => AgentEvent::ToolCallUpdate(update),
            FromAgent::FullPlan { plan } => AgentEvent::Plan(plan),
            FromAgent::Plan { entries } => AgentEvent::Plan(Plan {
                entries: entries
                    .into_iter()
                    .map(|content| PlanEntry {
                        content,
                        status: PlanEntryStatus::Pending,
                        priority: PlanEntryPriority::Medium,
                    })
                    .collect(),
            }),
            FromAgent::Permission {
                id,
                tool_call_id,
                title,
                options,
            } => {
                let (reply, answer) = PermissionReply::channel();
                asked.insert(id.clone(), reply.id());
                let writer = writer.clone();
                tokio::spawn(async move {
                    let option_id = match answer.await {
                        Ok(PermissionOutcome::Selected(option)) => Some(option),
                        Ok(PermissionOutcome::Cancelled) | Err(_) => None,
                    };
                    // The fake may be gone already; then nobody waits for the answer.
                    let _ = writer.send(&ToAgent::Permission { id, option_id }).await;
                });
                AgentEvent::Permission(PermissionRequest {
                    tool_call_id,
                    title,
                    options: options
                        .into_iter()
                        .map(|option| PermissionOption {
                            option_id: option.id,
                            name: option.name,
                            kind: option.kind,
                        })
                        .collect(),
                    reply,
                })
            }
            FromAgent::Withdraw { id } => match asked.remove(&id) {
                Some(permission) => AgentEvent::PermissionWithdrawn(permission),
                None => {
                    debug!(id, "the fake withdrew a request it never made");
                    continue;
                }
            },
            FromAgent::Usage {
                input_tokens,
                output_tokens,
                cost_usd,
            } => AgentEvent::Usage {
                input_tokens,
                output_tokens,
                cost_usd,
            },
            FromAgent::Limit { resets_at, message } => AgentEvent::LimitReached {
                provider: None,
                resets_at,
                message,
            },
            FromAgent::ModelChanged { model } => AgentEvent::ModelChanged(model),
            FromAgent::TurnEnded { reason, message } => AgentEvent::TurnEnded { reason, message },
        };
        // If nobody listens any more, keep draining until the process exits.
        let _ = events.send(event);
    }

    // Stdout closed: the fake is exiting, or was killed.
    writer.close().await;
    let exit = tokio::time::timeout(STOP_GRACE, handle.exited()).await;
    let error = if stopping.load(Ordering::SeqCst) {
        None
    } else {
        let exit = exit.map_or_else(|_| "stdout closed".to_owned(), |e| e.description);
        let stderr = handle.stderr_tail();
        Some(match stderr.last() {
            Some(last) => format!("the fake agent exited ({exit}): {last}"),
            None => format!("the fake agent exited ({exit})"),
        })
    };
    let _ = events.send(AgentEvent::Exited { error });
}

struct FakeSession {
    writer: Arc<Writer>,
    handle: ProcessHandle,
    stopping: Arc<AtomicBool>,
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        // Dropped without `stop`, for example because burrowd is shutting down: kill the tree.
        self.stopping.store(true, Ordering::SeqCst);
        self.handle.kill();
    }
}

impl AgentSession for FakeSession {
    fn prompt(&self, content: Vec<ContentBlock>) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            let text: Vec<&str> = content.iter().filter_map(ContentBlock::as_text).collect();
            self.writer
                .send(&ToAgent::Prompt {
                    text: text.join("\n"),
                })
                .await
        })
    }

    fn cancel(&self) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move { self.writer.send(&ToAgent::Cancel).await })
    }

    fn set_model(&self, model: String) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move { self.writer.send(&ToAgent::SetModel { model }).await })
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.stopping.store(true, Ordering::SeqCst);
            // Closing stdin asks the fake to exit; the job takes care of its child.
            self.writer.close().await;
            self.handle.stop(STOP_GRACE).await;
        })
    }
}
