//! The fake ACP agent, run as `burrow-fake-acp-agent`: a scripted agent that speaks real ACP
//! (newline-delimited JSON-RPC on stdin and stdout), so `burrowd`'s tests drive the whole ACP
//! adapter, `burrow-acp` included, without spending quota. `burrowd --dev` offers it as agent
//! `fake-acp`. It uses only blocking standard library I/O.
//!
//! On start it launches a child of its own (itself with `--sleeper`), so every session has a
//! real process tree. It offers three modes, `auto` (its default, which never asks), `ask` and
//! `plan`; three models as a `model` config option: `fake-fast`, `fake-smart`, and
//! `fake-param[context=1k,effort=high]`, named `fake-param` the way Cursor names its
//! parameterized models; and `session/resume` for any session id. Prompts are scripted:
//!
//! | Prompt | What happens |
//! |---|---|
//! | anything else | A thought, `Echo: <prompt>` word by word, the session's cost, usage |
//! | `/permission` | A tool call, then, unless the mode is `auto`, a permission request; the turn waits for the answer |
//! | `/sudden-permission` | A permission request about a tool call it never announced |
//! | `/withdraw` | A permission request it withdraws with `$/cancel_request` after 200 ms, as if it timed out; the turn then carries on |
//! | `/limit` | Claude Code's 5-hour limit notice, then the error it sends |
//! | `/slow` | 50 chunks, 100 ms apart; a cancel ends the turn `cancelled` |
//! | `/hang` | Never ends the turn, and ignores `session/cancel` (like codex-acp#641) |
//! | `/plan` | A three-step plan |
//! | `/mode` | Switches itself to mode `plan` |
//! | `/model` | Switches itself to model `fake-smart` |
//! | `/rate` | Claude Code's rate-limit status on the side: 90% of the 5-hour window |
//! | `/pids` | Replies `pids <agent pid> <child pid>` |
//! | `/crash` | Exits at once with code 3 |
//!
//! When stdin closes it exits; its child is left to the Job Object.

use std::io::{self, BufRead, Write};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// The mode that asks before every change: the one Burrow starts the fake in.
pub const ASK_MODE: &str = "ask";
/// The modes, the first being the fake's own default.
pub const MODES: &[&str] = &["auto", ASK_MODE, "plan"];
/// The models, the first being the default. The last has parameters, like Cursor's, and is
/// named after its base: `fake-param`.
pub const MODELS: &[&str] = &[
    "fake-fast",
    "fake-smart",
    "fake-param[context=1k,effort=high]",
];
/// The limit notice `/limit` sends, as Claude Code words it.
pub const LIMIT_NOTICE: &str = "You've hit your 5-hour limit · resets 3pm (Europe/Bucharest)";

/// How long `--sleeper` sleeps before giving up, so a fake started by hand outside a Job Object
/// doesn't leave its child behind forever.
const SLEEPER_LIFETIME: Duration = Duration::from_secs(600);
/// The pause between streamed words.
const WORD_DELAY: Duration = Duration::from_millis(5);
/// How long `/withdraw` waits for an answer before it withdraws its request.
const WITHDRAW_AFTER: Duration = Duration::from_millis(200);

/// A message from the client.
enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
    },
    Response {
        id: Value,
        result: Result<Value, Value>,
    },
}

/// The fake ACP agent's `main`.
pub fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--sleeper") {
        thread::sleep(SLEEPER_LIFETIME);
        return ExitCode::SUCCESS;
    }
    let child = spawn_sleeper();

    let (sender, inbox) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match parse(&line) {
                Some(message) => {
                    if sender.send(message).is_err() {
                        break;
                    }
                }
                None => eprintln!("burrow-fake-acp-agent: bad line {line:?}"),
            }
        }
        // Dropping `sender` tells the main loop that stdin closed.
    });

    let mut agent = Agent {
        inbox,
        session: None,
        mode: MODES[0].to_owned(),
        model: MODELS[0].to_owned(),
        next_id: 1,
        cost: 0.0,
        child_pid: child.as_ref().map(Child::id),
    };
    eprintln!("burrow-fake-acp-agent: ready");
    while let Ok(message) = agent.inbox.recv() {
        match message {
            Incoming::Request { id, method, params } => {
                if method == "session/prompt" {
                    agent.prompt(id, &params);
                } else {
                    agent.request(id, &method, &params);
                }
            }
            // A cancel with no turn running, or a late answer: nothing to do.
            Incoming::Notification { .. } | Incoming::Response { .. } => {}
        }
    }
    eprintln!("burrow-fake-acp-agent: stdin closed, exiting");
    ExitCode::SUCCESS
}

fn parse(line: &str) -> Option<Incoming> {
    let mut value: Value = serde_json::from_str(line).ok()?;
    let object = value.as_object_mut()?;
    let params = object.remove("params").unwrap_or(Value::Null);
    match (object.remove("id"), object.remove("method")) {
        (Some(id), Some(Value::String(method))) => Some(Incoming::Request { id, method, params }),
        (None, Some(Value::String(method))) => Some(Incoming::Notification { method }),
        (Some(id), None) => Some(Incoming::Response {
            id,
            result: match object.remove("error") {
                Some(error) => Err(error),
                None => Ok(object.remove("result").unwrap_or(Value::Null)),
            },
        }),
        _ => None,
    }
}

/// Starts this program again as `--sleeper`, so the session has a process tree. It is
/// deliberately not cleaned up here; the Job Object must do that.
fn spawn_sleeper() -> Option<Child> {
    let exe = std::env::current_exe().ok()?;
    Command::new(exe)
        .arg("--sleeper")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .inspect_err(|error| eprintln!("burrow-fake-acp-agent: no sleeper: {error}"))
        .ok()
}

/// Why a turn stopped early.
enum Interrupt {
    /// `session/cancel`.
    Cancelled,
    /// stdin closed.
    Gone,
}

struct Agent {
    inbox: Receiver<Incoming>,
    session: Option<String>,
    mode: String,
    model: String,
    next_id: u64,
    cost: f64,
    child_pid: Option<u32>,
}

impl Agent {
    fn write(&self, message: &Value) {
        let mut stdout = io::stdout().lock();
        // If the client is gone, the reader thread notices and the agent exits.
        let _ = writeln!(stdout, "{message}");
        let _ = stdout.flush();
    }

    fn respond(&self, id: &Value, result: Value) {
        self.write(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn fail(&self, id: &Value, code: i64, message: &str, data: Option<Value>) {
        let mut error = json!({"code": code, "message": message});
        if let Some(data) = data {
            error["data"] = data;
        }
        self.write(&json!({"jsonrpc": "2.0", "id": id, "error": error}));
    }

    fn update(&self, update: Value) {
        self.write(&json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {"sessionId": self.session.clone().unwrap_or_default(), "update": update},
        }));
    }

    fn chunk(&self, kind: &str, text: &str) {
        self.update(json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}));
    }

    fn modes(&self) -> Value {
        let describe = |mode: &str| match mode {
            "auto" => "Edits without asking",
            "ask" => "Asks before every change",
            _ => "Plans only",
        };
        json!({
            "currentModeId": self.mode,
            "availableModes": MODES.iter().map(|mode| json!({
                "id": mode,
                "name": capitalize(mode),
                "description": describe(mode),
            })).collect::<Vec<_>>(),
        })
    }

    fn config_options(&self) -> Value {
        json!([{
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": self.model,
            "options": MODELS.iter().map(|model| json!({"value": model, "name": model_name(model)})).collect::<Vec<_>>(),
        }])
    }

    /// Every request except `session/prompt`.
    fn request(&mut self, id: Value, method: &str, params: &Value) {
        let text = |key: &str| params[key].as_str().unwrap_or_default().to_owned();
        match method {
            "initialize" => self.respond(
                &id,
                json!({
                    "protocolVersion": 1,
                    "agentInfo": {"name": "burrow-fake-acp-agent", "title": "Fake ACP agent", "version": env!("CARGO_PKG_VERSION")},
                    "agentCapabilities": {"loadSession": false, "sessionCapabilities": {"resume": {}}},
                    "authMethods": [],
                }),
            ),
            "session/new" => {
                let session = format!("fake-acp-{}-{}", std::process::id(), self.next_id);
                self.next_id += 1;
                self.session = Some(session.clone());
                self.respond(
                    &id,
                    json!({"sessionId": session, "modes": self.modes(), "configOptions": self.config_options()}),
                );
            }
            "session/resume" => {
                self.session = Some(text("sessionId"));
                self.respond(
                    &id,
                    json!({"modes": self.modes(), "configOptions": self.config_options()}),
                );
            }
            "session/set_mode" => {
                let mode = text("modeId");
                if MODES.contains(&mode.as_str()) {
                    self.mode = mode;
                    self.respond(&id, json!({}));
                } else {
                    self.fail(&id, -32602, &format!("no mode {mode}"), None);
                }
            }
            "session/set_config_option" => {
                let value = text("value");
                if text("configId") == "model" && MODELS.contains(&value.as_str()) {
                    self.model = value;
                    self.respond(&id, json!({"configOptions": self.config_options()}));
                } else {
                    self.fail(&id, -32602, &format!("can't set that to {value}"), None);
                }
            }
            _ => self.fail(&id, -32601, &format!("method not found: {method}"), None),
        }
    }

    /// Waits up to `timeout` for a message, handling what can be handled during a turn.
    fn poll(&mut self, timeout: Duration) -> Result<Option<Incoming>, Interrupt> {
        match self.inbox.recv_timeout(timeout) {
            Ok(Incoming::Notification { method, .. }) if method == "session/cancel" => {
                Err(Interrupt::Cancelled)
            }
            Ok(Incoming::Request { id, method, params }) if method != "session/prompt" => {
                self.request(id, &method, &params);
                Ok(None)
            }
            Ok(Incoming::Request { id, .. }) => {
                self.fail(&id, -32600, "a prompt is already running", None);
                Ok(None)
            }
            Ok(other) => Ok(Some(other)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(Interrupt::Gone),
        }
    }

    /// Sleeps `duration`, handling messages meanwhile.
    fn pause(&mut self, duration: Duration) -> Result<(), Interrupt> {
        let until = Instant::now() + duration;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            self.poll(left)?;
        }
        Ok(())
    }

    fn stream(&mut self, text: &str) -> Result<(), Interrupt> {
        for (i, word) in text.split(' ').enumerate() {
            let piece = if i == 0 {
                word.to_owned()
            } else {
                format!(" {word}")
            };
            self.chunk("agent_message_chunk", &piece);
            self.pause(WORD_DELAY)?;
        }
        Ok(())
    }

    fn prompt(&mut self, id: Value, params: &Value) {
        let text = params["prompt"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let result = match text.trim() {
            "/permission" => self.permission(true),
            "/sudden-permission" => self.permission(false),
            "/withdraw" => self.withdraw(),
            "/limit" => {
                self.chunk("agent_message_chunk", LIMIT_NOTICE);
                self.fail(
                    &id,
                    -32603,
                    &format!("Internal error: {LIMIT_NOTICE}"),
                    Some(json!({"errorKind": "rate_limit"})),
                );
                return;
            }
            "/slow" => (0..50).try_for_each(|i| {
                self.chunk("agent_message_chunk", &format!("tick {i} "));
                self.pause(Duration::from_millis(100))
            }),
            "/hang" => {
                // Like codex-acp#641: the cancel is lost and the turn never ends.
                loop {
                    match self.poll(Duration::from_secs(3600)) {
                        Err(Interrupt::Gone) => return,
                        Ok(_) | Err(Interrupt::Cancelled) => {}
                    }
                }
            }
            "/plan" => {
                self.update(json!({"sessionUpdate": "plan", "entries": [
                    {"content": "Read the code", "priority": "high", "status": "completed"},
                    {"content": "Change it", "priority": "medium", "status": "in_progress"},
                    {"content": "Run the tests", "priority": "low", "status": "pending"},
                ]}));
                self.stream("Planned.")
            }
            "/mode" => {
                self.mode = "plan".into();
                self.update(
                    json!({"sessionUpdate": "current_mode_update", "currentModeId": "plan"}),
                );
                self.stream("Switched to plan mode.")
            }
            "/model" => {
                self.model = MODELS[1].into();
                self.update(json!({"sessionUpdate": "config_option_update", "configOptions": self.config_options()}));
                self.stream("Switched models.")
            }
            "/rate" => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                self.update(json!({
                    "sessionUpdate": "usage_update", "used": 1000, "size": 200_000,
                    "_meta": {"_claude/rateLimit": {
                        "status": "allowed_warning", "resetsAt": now + 3600,
                        "rateLimitType": "five_hour", "utilization": 0.9,
                    }},
                }));
                self.stream("Rate reported.")
            }
            "/pids" => {
                let reply = format!(
                    "pids {} {}",
                    std::process::id(),
                    self.child_pid.unwrap_or_default()
                );
                self.stream(&reply)
            }
            "/crash" => std::process::exit(3),
            _ => self.echo(&text),
        };
        match result {
            Ok(()) => {
                let words = text.split_whitespace().count() as u64;
                self.respond(
                    &id,
                    json!({"stopReason": "end_turn", "usage": {
                        "inputTokens": words, "outputTokens": words + 1, "totalTokens": 2 * words + 1,
                    }}),
                );
            }
            Err(Interrupt::Cancelled) => self.respond(&id, json!({"stopReason": "cancelled"})),
            Err(Interrupt::Gone) => {}
        }
    }

    fn echo(&mut self, text: &str) -> Result<(), Interrupt> {
        self.chunk("agent_thought_chunk", "Thinking about it.");
        self.stream(&format!("Echo: {text}"))?;
        self.cost += 0.0125;
        self.update(json!({
            "sessionUpdate": "usage_update", "used": 1000, "size": 200_000,
            "cost": {"amount": self.cost, "currency": "USD"},
        }));
        Ok(())
    }

    fn permission(&mut self, announce: bool) -> Result<(), Interrupt> {
        let call = format!("call_{}", self.next_id);
        self.next_id += 1;
        let (title, kind) = if announce {
            ("Write notes.txt", "edit")
        } else {
            ("Run echo hi", "execute")
        };
        if announce {
            self.update(json!({
                "sessionUpdate": "tool_call", "toolCallId": call, "title": title,
                "kind": kind, "status": "pending", "locations": [{"path": "notes.txt"}],
            }));
            if self.mode == "auto" {
                self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": call, "status": "completed"}));
                return self.stream("Wrote notes.txt without asking (auto mode).");
            }
        }
        let request_id = format!("perm-{}", self.next_id);
        self.next_id += 1;
        let mut tool_call = json!({"toolCallId": call});
        if !announce {
            tool_call =
                json!({"toolCallId": call, "title": title, "kind": kind, "status": "pending"});
        }
        self.write(&json!({
            "jsonrpc": "2.0", "id": request_id, "method": "session/request_permission",
            "params": {
                "sessionId": self.session.clone().unwrap_or_default(),
                "toolCall": tool_call,
                "options": [
                    {"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
                    {"optionId": "allow-always", "name": "Always allow", "kind": "allow_always"},
                    {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"},
                ],
            },
        }));
        let mut cancelled = false;
        let outcome = loop {
            match self.poll(Duration::from_secs(3600)) {
                Ok(Some(Incoming::Response { id, result })) if id == json!(request_id) => {
                    break result.unwrap_or(Value::Null);
                }
                Ok(_) => {}
                // The client answers the request as cancelled; wait for that answer.
                Err(Interrupt::Cancelled) => cancelled = true,
                Err(Interrupt::Gone) => return Err(Interrupt::Gone),
            }
        };
        let outcome = &outcome["outcome"];
        if cancelled || outcome["outcome"] == "cancelled" {
            return Err(Interrupt::Cancelled);
        }
        let allowed = outcome["optionId"]
            .as_str()
            .is_some_and(|option| option.starts_with("allow"));
        let (status, reply) = if allowed {
            ("completed", "Allowed.")
        } else {
            ("failed", "Rejected.")
        };
        self.update(json!({
            "sessionUpdate": "tool_call_update", "toolCallId": call, "status": status,
            "content": [{"type": "content", "content": {"type": "text", "text": reply}}],
        }));
        self.stream(reply)
    }
}

impl Agent {
    /// Asks permission, waits [`WITHDRAW_AFTER`] for an answer, then withdraws the request with
    /// `$/cancel_request` and carries on, as an agent whose question timed out does. ACP still
    /// expects the client to answer the withdrawn request (as cancelled); the fake waits for
    /// that answer.
    fn withdraw(&mut self) -> Result<(), Interrupt> {
        let call = format!("call_{}", self.next_id);
        let request_id = format!("perm-{}", self.next_id + 1);
        self.next_id += 2;
        self.update(json!({
            "sessionUpdate": "tool_call", "toolCallId": call, "title": "Run cleanup.ps1",
            "kind": "execute", "status": "pending",
        }));
        self.write(&json!({
            "jsonrpc": "2.0", "id": request_id, "method": "session/request_permission",
            "params": {
                "sessionId": self.session.clone().unwrap_or_default(),
                "toolCall": {"toolCallId": call},
                "options": [
                    {"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
                    {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"},
                ],
            },
        }));
        let mut cancelled = false;
        let mut withdrawn = false;
        let deadline = Instant::now() + WITHDRAW_AFTER;
        loop {
            let wait = match deadline.checked_duration_since(Instant::now()) {
                Some(left) if !withdrawn => left,
                _ => {
                    if !withdrawn {
                        withdrawn = true;
                        self.write(&json!({
                            "jsonrpc": "2.0", "method": "$/cancel_request",
                            "params": {"requestId": request_id},
                        }));
                    }
                    Duration::from_secs(3600)
                }
            };
            match self.poll(wait) {
                Ok(Some(Incoming::Response { id, .. })) if id == json!(request_id) => break,
                Ok(_) => {}
                Err(Interrupt::Cancelled) => cancelled = true,
                Err(Interrupt::Gone) => return Err(Interrupt::Gone),
            }
        }
        if cancelled {
            return Err(Interrupt::Cancelled);
        }
        if !withdrawn {
            return self.stream("You answered before the timeout.");
        }
        self.update(json!({
            "sessionUpdate": "tool_call_update", "toolCallId": call, "status": "failed",
            "content": [{"type": "content", "content": {"type": "text", "text": "Nobody answered in time"}}],
        }));
        self.stream("Nobody answered, so I skipped the cleanup.")
    }
}

/// A model's display name: the base of a parameterized id, as Cursor names them, or else the
/// id capitalized.
fn model_name(model: &str) -> String {
    match model.split_once('[') {
        Some((base, _)) => base.to_owned(),
        None => capitalize(model),
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
