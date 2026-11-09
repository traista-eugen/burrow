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
//! | `/withdraw` | A permission request that the fake withdraws after 200 ms, as if it timed out; the turn then carries on without the answer |
//! | `/limit` | A simulated usage limit that resets in an hour; the turn ends with `limit` |
//! | `/slow` | 100 chunks, 100 ms apart, so there is time to cancel |
//! | `/plan` | A three-step plan |
//! | `/steps` | A four-step plan that it updates in place as it reads, edits and runs the tests |
//! | `/markdown` | A thought, then headings, lists, a table, a quote and code in five languages |
//! | `/diff` | Three edits: old and new text as ACP sends it, Muse's fragment plus unified diff, and a new file |
//! | `/tools` | A run of reads and searches, then `cargo test` with its output |
//! | `/long` | A long answer, about 1,800 words, streamed over about six seconds |
//! | `/flood [rounds]` | As fast as it can, `rounds` (default 250) rounds of a thought, a read, a paragraph, and now and then a command or an edit: about five events a round, for measuring |
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

use burrow_protocol::{
    ContentBlock, PermissionOptionKind, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus,
    Timestamp, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate,
    ToolKind, TurnEndReason,
};

use super::content;
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
/// How long `/withdraw` waits for an answer before it withdraws its question.
const WITHDRAW_AFTER: Duration = Duration::from_millis(200);

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
            Some("/withdraw") => self.withdraw(),
            Some("/limit") => self.limit(),
            Some("/slow") => self.slow(),
            Some("/plan") => self.plan(),
            Some("/steps") => self.steps(),
            Some("/markdown") => self.markdown(),
            Some("/diff") => self.diff(),
            Some("/tools") => self.tools(),
            Some("/long") => self.long(),
            Some("/flood") => {
                let rounds = text
                    .split_whitespace()
                    .nth(1)
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(250);
                self.flood(rounds)
            }
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

    /// Asks, waits [`WITHDRAW_AFTER`] for an answer, then withdraws the question and carries
    /// on, as an agent whose question timed out does.
    fn withdraw(&mut self) -> Result<(), Interrupt> {
        let call = self.id("call");
        let request = self.id("perm");
        let title = "Run cleanup.ps1".to_owned();
        self.send(&FromAgent::ToolCall {
            id: call.clone(),
            title: title.clone(),
            kind: ToolKind::Execute,
            status: ToolCallStatus::Pending,
        });
        self.send(&FromAgent::Permission {
            id: request.clone(),
            tool_call_id: call.clone(),
            title,
            options: vec![
                option("allow", "Allow", PermissionOptionKind::AllowOnce),
                option("reject", "Reject", PermissionOptionKind::RejectOnce),
            ],
        });
        let deadline = std::time::Instant::now() + WITHDRAW_AFTER;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            let command = match self.inbox.recv_timeout(left) {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => std::process::exit(0),
            };
            match command {
                ToAgent::Cancel => {
                    self.send(&FromAgent::ToolCallUpdate {
                        id: call.clone(),
                        status: ToolCallStatus::Failed,
                        text: Some("Cancelled".into()),
                    });
                    return Err(Interrupt::Cancelled);
                }
                // An answer that beat the timeout: say what it was and carry on alike.
                ToAgent::Permission { id, option_id } if id == request => {
                    eprintln!("burrow-fake-agent: answered {option_id:?} before the withdrawal");
                }
                ToAgent::SetModel { model } => self.set_model(model),
                ToAgent::Prompt { .. } | ToAgent::Permission { .. } => {}
            }
        }
        self.send(&FromAgent::Withdraw { id: request });
        self.send(&FromAgent::ToolCallUpdate {
            id: call,
            status: ToolCallStatus::Failed,
            text: Some("Nobody answered in time".into()),
        });
        self.stream("Nobody answered, so I skipped the cleanup.")?;
        self.end(TurnEndReason::Completed, None);
        Ok(())
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

/// The content scripts: realistic markdown, edits and tool output (see [`content`]).
impl Agent {
    /// Streams `text` `words` words at a time.
    fn stream_by(&mut self, text: &str, words: usize) -> Result<(), Interrupt> {
        let mut piece = String::new();
        for (ix, word) in text.split_inclusive(' ').enumerate() {
            piece.push_str(word);
            if (ix + 1) % words == 0 {
                self.poll(Duration::ZERO)?;
                self.send(&FromAgent::Chunk {
                    text: std::mem::take(&mut piece),
                });
                thread::sleep(WORD_DELAY);
            }
        }
        if !piece.is_empty() {
            self.send(&FromAgent::Chunk { text: piece });
        }
        Ok(())
    }

    /// `relative` made absolute against the session's folder, which is the fake's own.
    fn path(&self, relative: &str) -> String {
        let cwd = std::env::current_dir().unwrap_or_default();
        cwd.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
            .display()
            .to_string()
    }

    /// Announces a tool call and returns its id.
    fn call(
        &mut self,
        kind: ToolKind,
        title: &str,
        locations: Vec<ToolCallLocation>,
        content: Vec<ToolCallContent>,
        status: ToolCallStatus,
    ) -> String {
        let id = self.id("call");
        self.send(&FromAgent::Call {
            call: ToolCall {
                turn_id: None,
                tool_call_id: id.clone(),
                title: title.to_owned(),
                kind,
                status,
                locations,
                content,
                raw_input: None,
            },
        });
        id
    }

    /// Moves a tool call to `status`, replacing its content when `content` is given.
    fn settle(&self, id: &str, status: ToolCallStatus, content: Option<Vec<ToolCallContent>>) {
        self.send(&FromAgent::CallUpdate {
            update: ToolCallUpdate {
                tool_call_id: id.to_owned(),
                status: Some(status),
                title: None,
                kind: None,
                locations: None,
                content,
            },
        });
    }

    fn location(&self, relative: &str, line: Option<u32>) -> Vec<ToolCallLocation> {
        vec![ToolCallLocation {
            path: self.path(relative),
            line,
        }]
    }

    fn finish(&mut self, input: u64, output: u64) -> Result<(), Interrupt> {
        self.send(&FromAgent::Usage {
            input_tokens: input,
            output_tokens: output,
            cost_usd: Some(0.0042),
        });
        self.end(TurnEndReason::Completed, None);
        Ok(())
    }

    fn markdown(&mut self) -> Result<(), Interrupt> {
        self.send(&FromAgent::Thought {
            text: content::THOUGHT.into(),
        });
        self.stream_by(content::MARKDOWN, 4)?;
        self.finish(1_204, 386)
    }

    fn long(&mut self) -> Result<(), Interrupt> {
        self.stream_by(&content::long_answer(), 3)?;
        self.finish(2_310, 2_450)
    }

    fn diff(&mut self) -> Result<(), Interrupt> {
        use content::{CONFIG_EDIT, LIB_EDIT, NEW_FILE};
        self.stream(
            "I'll make the arithmetic saturate, add `sub`, and give the config more retries.",
        )?;

        // As an ACP agent sends an edit: the old and new text, and where it starts.
        let lib = self.path(LIB_EDIT.path);
        let edit = self.call(
            ToolKind::Edit,
            &format!("Edit `{}`", LIB_EDIT.path),
            self.location(LIB_EDIT.path, Some(LIB_EDIT.line)),
            vec![ToolCallContent::Diff {
                path: lib,
                old_text: Some(LIB_EDIT.old.into()),
                new_text: LIB_EDIT.new.into(),
            }],
            ToolCallStatus::Pending,
        );
        self.poll(Duration::from_millis(150))?;
        self.settle(&edit, ToolCallStatus::InProgress, None);
        self.poll(Duration::from_millis(150))?;
        self.settle(&edit, ToolCallStatus::Completed, None);

        // As Muse reports one: the replaced fragment, then a unified diff in the output.
        let config = self.path(CONFIG_EDIT.path);
        let fragment = ToolCallContent::Diff {
            path: config,
            old_text: Some(CONFIG_EDIT.old.into()),
            new_text: CONFIG_EDIT.new.into(),
        };
        let edit = self.call(
            ToolKind::Edit,
            &format!("Edit {}", CONFIG_EDIT.path),
            self.location(CONFIG_EDIT.path, None),
            vec![fragment.clone()],
            ToolCallStatus::InProgress,
        );
        self.poll(Duration::from_millis(150))?;
        self.settle(
            &edit,
            ToolCallStatus::Completed,
            Some(vec![fragment, text_content(CONFIG_EDIT.unified)]),
        );

        // A new file.
        let notes = self.path(NEW_FILE.0);
        self.call(
            ToolKind::Edit,
            &format!("Write {}", NEW_FILE.0),
            self.location(NEW_FILE.0, None),
            vec![ToolCallContent::Diff {
                path: notes,
                old_text: None,
                new_text: NEW_FILE.1.into(),
            }],
            ToolCallStatus::Completed,
        );
        self.stream("Done: `add` saturates, `sub` is new, the config retries three times, and NOTES.md says so.")?;
        self.finish(3_920, 512)
    }

    fn tools(&mut self) -> Result<(), Interrupt> {
        self.send(&FromAgent::Thought {
            text: "**Exploring the project**\n\nI'll read the README and the library, look for \
                   `add`, then run the tests."
                .into(),
        });
        let mut searches = content::SEARCHES.iter();
        for (title, path, text) in content::READS {
            let read = self.call(
                ToolKind::Read,
                title,
                self.location(path, None),
                Vec::new(),
                ToolCallStatus::Pending,
            );
            self.poll(Duration::from_millis(80))?;
            self.settle(
                &read,
                ToolCallStatus::Completed,
                Some(vec![text_content(&content::fenced(path, text))]),
            );
            if let Some((title, result)) = searches.next() {
                let search = self.call(
                    ToolKind::Search,
                    title,
                    Vec::new(),
                    Vec::new(),
                    ToolCallStatus::InProgress,
                );
                self.poll(Duration::from_millis(80))?;
                self.settle(
                    &search,
                    ToolCallStatus::Completed,
                    Some(vec![text_content(result)]),
                );
            }
        }
        let run = self.call(
            ToolKind::Execute,
            content::TEST_COMMAND,
            Vec::new(),
            Vec::new(),
            ToolCallStatus::InProgress,
        );
        self.poll(Duration::from_millis(400))?;
        self.settle(
            &run,
            ToolCallStatus::Completed,
            Some(vec![text_content(content::TEST_OUTPUT)]),
        );
        self.stream("All four tests pass. `add` is covered by `tests/add.rs`, but nothing tests overflow yet.")?;
        self.finish(8_754, 640)
    }

    fn steps(&mut self) -> Result<(), Interrupt> {
        use PlanEntryStatus::{Completed, InProgress, Pending};
        const STEPS: [(&str, PlanEntryPriority); 4] = [
            ("Read the library", PlanEntryPriority::High),
            ("Make `add` saturate", PlanEntryPriority::High),
            ("Add `sub`", PlanEntryPriority::Medium),
            ("Run the tests", PlanEntryPriority::Low),
        ];
        let plan = |agent: &Self, done: usize| {
            let entries = STEPS
                .iter()
                .enumerate()
                .map(|(ix, (step, priority))| PlanEntry {
                    content: (*step).to_owned(),
                    status: match ix.cmp(&done) {
                        std::cmp::Ordering::Less => Completed,
                        std::cmp::Ordering::Equal => InProgress,
                        std::cmp::Ordering::Greater => Pending,
                    },
                    priority: priority.clone(),
                })
                .collect();
            agent.send(&FromAgent::FullPlan {
                plan: Plan { entries },
            });
        };
        plan(self, 0);
        let (title, path, text) = content::READS[1];
        let read = self.call(
            ToolKind::Read,
            title,
            self.location(path, None),
            Vec::new(),
            ToolCallStatus::Pending,
        );
        self.poll(Duration::from_millis(300))?;
        self.settle(
            &read,
            ToolCallStatus::Completed,
            Some(vec![text_content(&content::fenced(path, text))]),
        );
        for step in 1..=2 {
            plan(self, step);
            self.poll(Duration::from_millis(400))?;
        }
        plan(self, 3);
        let run = self.call(
            ToolKind::Execute,
            content::TEST_COMMAND,
            Vec::new(),
            Vec::new(),
            ToolCallStatus::InProgress,
        );
        self.poll(Duration::from_millis(500))?;
        self.settle(
            &run,
            ToolCallStatus::Completed,
            Some(vec![text_content(content::TEST_OUTPUT)]),
        );
        plan(self, STEPS.len());
        self.stream("All four steps are done.")?;
        self.finish(5_120, 330)
    }

    /// Events as fast as the fake can send them, for measuring a long transcript.
    fn flood(&mut self, rounds: usize) -> Result<(), Interrupt> {
        for round in 0..rounds {
            self.poll(Duration::ZERO)?;
            self.send(&FromAgent::Thought {
                text: format!("Round {round}: checking the next module."),
            });
            let path = format!("src/module_{round}.rs");
            let read = self.call(
                ToolKind::Read,
                &format!("Read {path}"),
                self.location(&path, None),
                Vec::new(),
                ToolCallStatus::Pending,
            );
            self.settle(
                &read,
                ToolCallStatus::Completed,
                Some(vec![text_content(&content::fenced(
                    &path,
                    &format!("pub fn f{round}() -> u32 {{\n    {round}\n}}\n"),
                ))]),
            );
            self.send(&FromAgent::Chunk {
                text: format!(
                    "Round **{round}** looked at `module_{round}`: it returns {round}, which is \
                     what the caller expects, so nothing needs to change there.\n\n"
                ),
            });
            if round % 10 == 9 {
                let run = self.call(
                    ToolKind::Execute,
                    "cargo check",
                    Vec::new(),
                    Vec::new(),
                    ToolCallStatus::InProgress,
                );
                self.settle(
                    &run,
                    ToolCallStatus::Completed,
                    Some(vec![text_content(
                        "    Checking calc v0.1.0\n    Finished `dev` profile in 0.42s\n",
                    )]),
                );
            }
            if round % 25 == 24 {
                let path = self.path(&format!("src/module_{round}.rs"));
                self.call(
                    ToolKind::Edit,
                    &format!("Edit src/module_{round}.rs"),
                    self.location(&format!("src/module_{round}.rs"), Some(2)),
                    vec![ToolCallContent::Diff {
                        path,
                        old_text: Some(format!("    {round}\n")),
                        new_text: format!("    {round} + 1\n"),
                    }],
                    ToolCallStatus::Completed,
                );
            }
        }
        self.finish(rounds as u64 * 100, rounds as u64 * 40)
    }
}

fn text_content(text: &str) -> ToolCallContent {
    ToolCallContent::Content {
        content: ContentBlock::text(text),
    }
}

fn option(id: &str, name: &str, kind: PermissionOptionKind) -> FakeOption {
    FakeOption {
        id: id.into(),
        name: name.into(),
        kind,
    }
}
