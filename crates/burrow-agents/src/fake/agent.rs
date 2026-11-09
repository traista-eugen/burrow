//! The fake agent program, run as `burrow-fake-agent`. It speaks the [`wire`](super::wire)
//! protocol on stdin and stdout, using only blocking standard library I/O.
//!
//! On start it launches a child of its own (itself with `--sleeper`), so every session has a
//! real process tree, then announces `ready`. Prompts are scripted:
//!
//! | Prompt | What happens |
//! |---|---|
//! | anything else | A thought, then `Echo: <prompt>` streamed word by word, then usage |
//! | `/permission` | A tool call and a permission request; the turn waits for the answer |
//! | `/limit` | A simulated usage limit that resets in an hour; the turn ends with `limit` |
//! | `/slow` | 100 chunks, 100 ms apart, so there is time to cancel |
//! | `/plan` | A three-step plan |
//! | `/pids` | Replies `pids <agent pid> <child pid>` |
//! | `/crash` | Exits at once with code 3 |
//!
//! With `--model fake-slow-start` it waits a minute before announcing `ready`, for testing a
//! cancelled `session/create`. When stdin closes it exits; its child is left to the Job Object.

use std::io::{self, BufRead, Write};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use burrow_protocol::{PermissionOptionKind, Timestamp, ToolCallStatus, ToolKind, TurnEndReason};

use super::wire::{FakeOption, FromAgent, ToAgent};

/// The models the fake offers. The first is the default.
pub const MODELS: &[&str] = &["fake", "fake-alt"];
/// A model that makes the fake take a minute to start.
pub const SLOW_START_MODEL: &str = "fake-slow-start";

/// How long `--sleeper` sleeps before giving up, so a fake started by hand outside a Job Object
/// doesn't leave its child behind forever.
const SLEEPER_LIFETIME: Duration = Duration::from_secs(600);
/// How long a slow start takes.
const SLOW_START: Duration = Duration::from_secs(60);
/// The pause between streamed words.
const WORD_DELAY: Duration = Duration::from_millis(10);

/// The fake agent's `main`.
pub fn main() -> ExitCode {
    let mut model = MODELS[0].to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sleeper" => {
                thread::sleep(SLEEPER_LIFETIME);
                return ExitCode::SUCCESS;
            }
            "--model" => {
                if let Some(value) = args.next() {
                    model = value;
                }
            }
            other => eprintln!("burrow-fake-agent: ignoring argument {other:?}"),
        }
    }

    let child = spawn_sleeper();
    if model == SLOW_START_MODEL {
        thread::sleep(SLOW_START);
    }

    let (commands, inbox) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ToAgent>(&line) {
                Ok(command) => {
                    if commands.send(command).is_err() {
                        break;
                    }
                }
                Err(error) => eprintln!("burrow-fake-agent: bad line {line:?}: {error}"),
            }
        }
        // Dropping `commands` tells the main loop that stdin closed.
    });

    let mut agent = Agent {
        inbox,
        model,
        next_id: 1,
        child_pid: child.as_ref().map(Child::id),
    };
    agent.send(&FromAgent::Ready {
        model: agent.model.clone(),
        models: MODELS.iter().map(|m| (*m).to_owned()).collect(),
        pid: std::process::id(),
        child_pid: agent.child_pid,
    });
    eprintln!("burrow-fake-agent: ready");
    while let Ok(command) = agent.inbox.recv() {
        match command {
            ToAgent::Prompt { text } => agent.turn(&text),
            ToAgent::SetModel { model } => agent.set_model(model),
            // Nothing is running, or the request was already settled.
            ToAgent::Cancel | ToAgent::Permission { .. } => {}
        }
    }
    eprintln!("burrow-fake-agent: stdin closed, exiting");
    ExitCode::SUCCESS
}

/// Starts this program again as `--sleeper`: a child that just waits, so the session has a
/// process tree. It is deliberately not cleaned up here; the Job Object must do that.
fn spawn_sleeper() -> Option<Child> {
    let exe = std::env::current_exe().ok()?;
    let mut command = Command::new(exe);
    command
        .arg("--sleeper")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// No console window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    match command.spawn() {
        Ok(child) => Some(child),
        Err(error) => {
            eprintln!("burrow-fake-agent: can't start the sleeper: {error}");
            None
        }
    }
}

struct Agent {
    inbox: Receiver<ToAgent>,
    model: String,
    next_id: u32,
    child_pid: Option<u32>,
}

/// What interrupted a turn.
enum Interrupt {
    Cancelled,
}

impl Agent {
    fn send(&self, message: &FromAgent) {
        let line = serde_json::to_string(message).expect("messages serialize");
        let mut stdout = io::stdout().lock();
        if writeln!(stdout, "{line}")
            .and_then(|()| stdout.flush())
            .is_err()
        {
            // Nobody listens any more.
            std::process::exit(0);
        }
    }

    fn id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}_{}", self.next_id);
        self.next_id += 1;
        id
    }

    fn set_model(&mut self, model: String) {
        self.model = model;
        self.send(&FromAgent::ModelChanged {
            model: self.model.clone(),
        });
    }

    fn end(&self, reason: TurnEndReason, message: Option<&str>) {
        self.send(&FromAgent::TurnEnded {
            reason,
            message: message.map(str::to_owned),
        });
    }

    /// Handles commands that arrived during a turn, waiting up to `timeout` for one.
    fn poll(&mut self, timeout: Duration) -> Result<(), Interrupt> {
        let command = if timeout.is_zero() {
            match self.inbox.try_recv() {
                Ok(command) => command,
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => std::process::exit(0),
            }
        } else {
            match self.inbox.recv_timeout(timeout) {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => return Ok(()),
                Err(RecvTimeoutError::Disconnected) => std::process::exit(0),
            }
        };
        match command {
            ToAgent::Cancel => Err(Interrupt::Cancelled),
            ToAgent::SetModel { model } => {
                self.set_model(model);
                Ok(())
            }
            ToAgent::Prompt { .. } => {
                eprintln!("burrow-fake-agent: a prompt arrived during a turn; ignoring it");
                Ok(())
            }
            ToAgent::Permission { .. } => Ok(()),
        }
    }

    /// Streams `text` word by word. Returns early if the turn is cancelled.
    fn stream(&mut self, text: &str) -> Result<(), Interrupt> {
        for word in text.split_inclusive(' ') {
            self.poll(Duration::ZERO)?;
            self.send(&FromAgent::Chunk {
                text: word.to_owned(),
            });
            thread::sleep(WORD_DELAY);
        }
        Ok(())
    }

    fn turn(&mut self, text: &str) {
        let text = text.trim();
        let result = match text.split_whitespace().next() {
            Some("/permission") => self.permission(),
            Some("/limit") => self.limit(),
            Some("/slow") => self.slow(),
            Some("/plan") => self.plan(),
            Some("/pids") => {
                let reply = format!(
                    "pids {} {}",
                    std::process::id(),
                    self.child_pid.unwrap_or_default()
                );
                self.stream(&reply)
                    .map(|()| self.end(TurnEndReason::Completed, None))
            }
            Some("/crash") => std::process::exit(3),
            _ => self.echo(text),
        };
        if let Err(Interrupt::Cancelled) = result {
            self.end(TurnEndReason::Cancelled, None);
        }
    }

    fn echo(&mut self, text: &str) -> Result<(), Interrupt> {
        self.send(&FromAgent::Thought {
            text: "Thinking about your prompt.".into(),
        });
        let reply = format!("Echo: {text}");
        self.stream(&reply)?;
        self.send(&FromAgent::Usage {
            input_tokens: text.split_whitespace().count() as u64,
            output_tokens: reply.split_whitespace().count() as u64,
            cost_usd: None,
        });
        self.end(TurnEndReason::Completed, None);
        Ok(())
    }

    fn permission(&mut self) -> Result<(), Interrupt> {
        let call = self.id("call");
        let request = self.id("perm");
        let title = "Edit README.md".to_owned();
        self.send(&FromAgent::ToolCall {
            id: call.clone(),
            title: title.clone(),
            kind: ToolKind::Edit,
            status: ToolCallStatus::Pending,
        });
        self.send(&FromAgent::Permission {
            id: request.clone(),
            tool_call_id: call.clone(),
            title,
            options: vec![
                option("allow", "Allow", PermissionOptionKind::AllowOnce),
                option(
                    "allow-always",
                    "Always allow",
                    PermissionOptionKind::AllowAlways,
                ),
                option("reject", "Reject", PermissionOptionKind::RejectOnce),
            ],
        });
        let failed = |agent: &Self, text: &str| {
            agent.send(&FromAgent::ToolCallUpdate {
                id: call.clone(),
                status: ToolCallStatus::Failed,
                text: Some(text.into()),
            });
        };
        loop {
            let command = match self.inbox.recv() {
                Ok(command) => command,
                Err(_) => std::process::exit(0),
            };
            match command {
                ToAgent::Permission { id, option_id } if id == request => match option_id {
                    Some(option) if option.starts_with("allow") => {
                        self.send(&FromAgent::ToolCallUpdate {
                            id: call.clone(),
                            status: ToolCallStatus::InProgress,
                            text: None,
                        });
                        self.send(&FromAgent::ToolCallUpdate {
                            id: call.clone(),
                            status: ToolCallStatus::Completed,
                            text: Some("Edited README.md".into()),
                        });
                        self.stream("Done: I edited README.md.")?;
                        self.end(TurnEndReason::Completed, None);
                        return Ok(());
                    }
                    Some(_) => {
                        failed(self, "Permission denied");
                        self.stream("Okay, I left README.md alone.")?;
                        self.end(TurnEndReason::Completed, None);
                        return Ok(());
                    }
                    None => {
                        failed(self, "Cancelled");
                        return Err(Interrupt::Cancelled);
                    }
                },
                ToAgent::Cancel => {
                    failed(self, "Cancelled");
                    return Err(Interrupt::Cancelled);
                }
                ToAgent::SetModel { model } => self.set_model(model),
                ToAgent::Prompt { .. } | ToAgent::Permission { .. } => {}
            }
        }
    }

    fn limit(&mut self) -> Result<(), Interrupt> {
        self.stream("Working on it… ")?;
        let in_an_hour = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|now| now.as_millis() as i64 + 3_600_000)
            .ok()
            .and_then(|millis| Timestamp::from_unix_millis(millis).ok());
        self.send(&FromAgent::Limit {
            resets_at: in_an_hour,
            message: "Simulated usage limit. It resets in an hour.".into(),
        });
        self.end(TurnEndReason::Limit, Some("usage limit reached"));
        Ok(())
    }

    fn slow(&mut self) -> Result<(), Interrupt> {
        for tick in 1..=100 {
            self.poll(Duration::from_millis(100))?;
            self.send(&FromAgent::Chunk {
                text: format!("tick {tick} "),
            });
        }
        self.end(TurnEndReason::Completed, None);
        Ok(())
    }

    fn plan(&mut self) -> Result<(), Interrupt> {
        self.send(&FromAgent::Plan {
            entries: vec![
                "Read the code".into(),
                "Write a test".into(),
                "Make it pass".into(),
            ],
        });
        self.stream("Here is my plan.")?;
        self.end(TurnEndReason::Completed, None);
        Ok(())
    }
}

fn option(id: &str, name: &str, kind: PermissionOptionKind) -> FakeOption {
    FakeOption {
        id: id.into(),
        name: name.into(),
        kind,
    }
}
