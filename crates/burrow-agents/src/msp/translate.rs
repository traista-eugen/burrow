//! MSP events to [`AgentEvent`]s: one [`Translator`] per session (D-026).
//!
//! The translator holds what the mapping needs to remember: the Burrow turn's Muse turn id, the
//! text already sent per item, and the tool calls already announced. It does no I/O; the session
//! loop ([`super::session`]) sends what it returns, and performs the [`Output`]s that need Muse.
//!
//! | MSP | Burrow |
//! |---|---|
//! | `item/delta` of an `agentMessage` | `MessageChunk`; a new message item in the same turn starts a new paragraph |
//! | `item/delta` `summary.N` of a `reasoning` item | `ThoughtChunk` |
//! | a `toolCall` item, first seen | `ToolCall`, with its id as the tool call id |
//! | the same item, later | `ToolCallUpdate`: status; on completion its output, and the edit as a diff |
//! | `approval/request` | `ToolCall` (pending) if it wasn't seen yet, then `Permission` |
//! | `session/tokenUsage` | summed per turn, sent as one `Usage` before the turn ends |
//! | `turn/completed` | `TurnEnded`; a quota failure first sends `LimitReached` |
//! | `usage/changed` | `ProviderUsage` |
//! | `session/modelChanged` | `ModelChanged` |
//!
//! Items of other turns, `reminderChild` items and `turn/foregroundCompleted` map to nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use burrow_msp::{
    ApprovalChoice, ApprovalChoiceScope, ApprovalDecision, ApprovalRequestParams,
    ApprovalSubjectKind, DeltaField, Event, FailureClass, Item, ItemDeltaParams, ItemKind,
    ItemStatus, SessionClosedReason, SubscriptionUsage, TurnCompletedParams, TurnError,
    TurnErrorKind, TurnTerminal, UserInputRequestParams,
};
use burrow_protocol::{
    AgentId, ContentBlock, PermissionOption, PermissionOptionKind, ProviderUsage, Timestamp,
    ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolKind,
    TurnEndReason, UsageSource,
};
use serde_json::Value;
use tracing::{debug, warn};

use crate::adapter::AgentEvent;

/// What the session loop does with a translated event.
#[derive(Debug)]
pub(super) enum Output {
    /// Send this event.
    Event(AgentEvent),
    /// Ask the user: send [`AgentEvent::Permission`] with these parts, and decide `approval`
    /// with the answer.
    Ask {
        /// The tool call it is about: Muse's item id.
        tool_call_id: String,
        /// What Muse wants to do.
        title: String,
        /// Muse's choices, as permission options.
        options: Vec<PermissionOption>,
        /// The request, for `approval/decide`.
        approval: Box<ApprovalRequestParams>,
    },
    /// Muse asked the user a question, which Burrow can't show yet (D-027): decline it, so the
    /// turn doesn't wait for Muse's timeout.
    DeclineInput(Box<UserInputRequestParams>),
    /// Muse unloaded the session by itself; the session is over.
    Closed(String),
}

/// Remembers what the mapping needs between events. See the module docs.
#[derive(Debug)]
pub(super) struct Translator {
    /// Muse's session id: events of other sessions are ignored.
    session_id: String,
    /// The session's workspace, to make tool paths absolute.
    workspace: PathBuf,
    /// The running turn, while Burrow waits for its `turn/completed`.
    turn: Option<Turn>,
    /// Tool calls announced and not finished, by item id.
    tools: HashMap<String, ToolState>,
}

#[derive(Debug)]
struct Turn {
    id: String,
    /// Token counts of the turn's model calls, summed.
    tokens: Option<(u64, u64)>,
    /// Text sent so far, by `agentMessage` item.
    messages: HashMap<String, String>,
    /// The message item that sent text last, to start a new paragraph for the next one.
    last_message: Option<String>,
    /// Summary parts sent so far, by `reasoning` item.
    thoughts: HashMap<String, Vec<String>>,
    /// The reasoning part that sent text last.
    last_thought: Option<(String, usize)>,
}

#[derive(Debug)]
struct ToolState {
    status: ToolCallStatus,
    title: String,
}

impl Translator {
    /// A translator for Muse session `session_id`, working in `workspace`.
    pub(super) fn new(session_id: impl Into<String>, workspace: impl Into<PathBuf>) -> Self {
        Self {
            session_id: session_id.into(),
            workspace: workspace.into(),
            turn: None,
            tools: HashMap::new(),
        }
    }

    /// Burrow's turn is Muse turn `id` from now on. Called before `turn/start` is sent, with the
    /// `commandId` that becomes a fresh turn's id, so no event of the turn is missed.
    pub(super) fn begin_turn(&mut self, id: impl Into<String>) {
        let id = id.into();
        match &mut self.turn {
            Some(turn) if turn.id != id => turn.id = id,
            Some(_) => {}
            None => {
                self.turn = Some(Turn {
                    id,
                    tokens: None,
                    messages: HashMap::new(),
                    last_message: None,
                    thoughts: HashMap::new(),
                    last_thought: None,
                })
            }
        }
    }

    /// Forgets the turn, because `turn/start` failed and Burrow ends it itself.
    pub(super) fn abandon_turn(&mut self) {
        self.turn = None;
    }

    /// The running turn's Muse id.
    pub(super) fn turn_id(&self) -> Option<&str> {
        self.turn.as_ref().map(|turn| turn.id.as_str())
    }

    /// Translates one event.
    pub(super) fn on_event(&mut self, event: Event) -> Vec<Output> {
        if event
            .session_id()
            .is_some_and(|session| session != self.session_id)
        {
            return Vec::new();
        }
        match event {
            Event::ItemStarted(params)
            | Event::ItemUpdated(params)
            | Event::ItemCompleted(params) => self.item(&params.item),
            Event::ItemDelta { delta, kind } => self.delta(&delta, kind.as_ref()),
            Event::ApprovalRequested(request) => self.approval(request),
            Event::UserInputRequested(request) => vec![Output::DeclineInput(Box::new(request))],
            Event::TokenUsage(usage) => {
                if let Some(turn) = &mut self.turn
                    && turn.id == usage.turn_id
                {
                    let (input, output) = turn.tokens.get_or_insert((0, 0));
                    *input += usage.usage.input_tokens;
                    *output += usage.usage.output_tokens;
                }
                Vec::new()
            }
            Event::TurnCompleted(done) => self.turn_completed(done),
            Event::ModelChanged(changed) => {
                vec![Output::Event(AgentEvent::ModelChanged(changed.model_id))]
            }
            Event::UsageChanged(usage) => vec![Output::Event(AgentEvent::ProviderUsage(
                provider_usage(Some(&usage)),
            ))],
            Event::SessionClosed(closed) if closed.reason != SessionClosedReason::HostShutdown => {
                vec![Output::Closed(format!(
                    "Muse unloaded the session ({})",
                    closed.reason.as_str()
                ))]
            }
            Event::ViewGap(gap) => {
                warn!(
                    session = self.session_id,
                    after = gap.after,
                    next = gap.next,
                    "muse dropped events"
                );
                Vec::new()
            }
            Event::Malformed { method, error, .. } => {
                warn!(method, error, "muse sent an event burrow-msp can't decode");
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn in_turn(&self, item: &Item) -> bool {
        matches!((&self.turn, &item.turn_id), (Some(turn), Some(id)) if turn.id == *id)
    }

    fn item(&mut self, item: &Item) -> Vec<Output> {
        if !self.in_turn(item) && !self.tools.contains_key(&item.item_id) {
            return Vec::new();
        }
        let done = item.status.is_terminal();
        match item.kind {
            ItemKind::AgentMessage if done => {
                let text = item.text.as_deref().unwrap_or_default();
                let sent = self
                    .turn
                    .as_mut()
                    .and_then(|turn| turn.messages.remove(&item.item_id))
                    .unwrap_or_default();
                match text.strip_prefix(sent.as_str()) {
                    Some(rest) => self.message(&item.item_id, rest).into_iter().collect(),
                    None => {
                        debug!(
                            item = item.item_id,
                            "a final message differs from its deltas"
                        );
                        Vec::new()
                    }
                }
            }
            ItemKind::Reasoning if done => {
                let sent = self
                    .turn
                    .as_mut()
                    .and_then(|turn| turn.thoughts.remove(&item.item_id))
                    .unwrap_or_default();
                let mut out = Vec::new();
                for (index, part) in item.summary.iter().enumerate() {
                    let already = sent.get(index).map_or("", String::as_str);
                    if let Some(rest) = part.strip_prefix(already) {
                        out.extend(self.thought(&item.item_id, index, rest));
                    }
                }
                out
            }
            ItemKind::ToolCall => self.tool(item),
            _ => Vec::new(),
        }
    }

    fn delta(&mut self, delta: &ItemDeltaParams, kind: Option<&ItemKind>) -> Vec<Output> {
        if self.turn.is_none() {
            return Vec::new();
        }
        match (kind, delta.field()) {
            (Some(ItemKind::AgentMessage), DeltaField::Text) => self
                .message(&delta.item_id, &delta.delta)
                .into_iter()
                .collect(),
            (Some(ItemKind::Reasoning), DeltaField::Summary(index)) => self
                .thought(&delta.item_id, index, &delta.delta)
                .into_iter()
                .collect(),
            // Tool output arrives whole when the call completes.
            _ => Vec::new(),
        }
    }

    /// Sends `piece` of message item `item`, after a paragraph break if another message item of
    /// the turn sent text before.
    fn message(&mut self, item: &str, piece: &str) -> Option<Output> {
        let turn = self.turn.as_mut()?;
        if piece.is_empty() {
            return None;
        }
        let sent = turn.messages.entry(item.to_owned()).or_default();
        let mut text = String::new();
        if sent.is_empty()
            && turn
                .last_message
                .as_deref()
                .is_some_and(|last| last != item)
        {
            text.push_str("\n\n");
        }
        sent.push_str(piece);
        text.push_str(piece);
        turn.last_message = Some(item.to_owned());
        Some(Output::Event(AgentEvent::MessageChunk(text)))
    }

    /// Sends `piece` of summary part `index` of reasoning item `item`, after a paragraph break
    /// if another part sent text before.
    fn thought(&mut self, item: &str, index: usize, piece: &str) -> Option<Output> {
        let turn = self.turn.as_mut()?;
        if piece.is_empty() {
            return None;
        }
        let parts = turn.thoughts.entry(item.to_owned()).or_default();
        if parts.len() <= index {
            parts.resize(index + 1, String::new());
        }
        let mut text = String::new();
        let here = (item.to_owned(), index);
        if parts[index].is_empty() && turn.last_thought.as_ref().is_some_and(|last| *last != here) {
            text.push_str("\n\n");
        }
        parts[index].push_str(piece);
        text.push_str(piece);
        turn.last_thought = Some(here);
        Some(Output::Event(AgentEvent::ThoughtChunk(text)))
    }

    fn tool(&mut self, item: &Item) -> Vec<Output> {
        let tool = item.tool.as_deref().unwrap_or_default();
        let args = parse_args(item.args.as_deref());
        let status = tool_status(&item.status);
        let title = tool_title(item, tool, args.as_ref(), &self.workspace);
        let done = item.status.is_terminal();
        let content = || {
            let mut content = edit_diff(tool, args.as_ref(), &self.workspace)
                .into_iter()
                .collect::<Vec<_>>();
            if done {
                content.extend(tool_output(item));
            }
            content
        };
        let Some(state) = self.tools.get_mut(&item.item_id) else {
            let call = ToolCall {
                turn_id: None,
                tool_call_id: item.item_id.clone(),
                title: title.clone(),
                kind: tool_kind(tool),
                status: status.clone(),
                locations: locations(args.as_ref(), &self.workspace),
                content: content(),
                raw_input: args.clone(),
            };
            if !done {
                self.tools
                    .insert(item.item_id.clone(), ToolState { status, title });
            }
            return vec![Output::Event(AgentEvent::ToolCall(call))];
        };
        if !done && state.status == status && state.title == title {
            return Vec::new();
        }
        let update = ToolCallUpdate {
            tool_call_id: item.item_id.clone(),
            status: (state.status != status || done).then(|| status.clone()),
            title: (state.title != title).then(|| title.clone()),
            kind: None,
            locations: None,
            content: done.then(content),
        };
        state.status = status;
        state.title = title;
        if done {
            self.tools.remove(&item.item_id);
        }
        vec![Output::Event(AgentEvent::ToolCallUpdate(update))]
    }

    fn approval(&mut self, request: ApprovalRequestParams) -> Vec<Output> {
        let title = approval_title(&request);
        let mut out = Vec::new();
        if !self.tools.contains_key(&request.item_id) {
            let args = parse_args(Some(&request.raw_args));
            out.push(Output::Event(AgentEvent::ToolCall(ToolCall {
                turn_id: None,
                tool_call_id: request.item_id.clone(),
                title: title.clone(),
                kind: tool_kind(&request.tool_name),
                status: ToolCallStatus::Pending,
                locations: locations(args.as_ref(), &self.workspace),
                content: edit_diff(&request.tool_name, args.as_ref(), &self.workspace)
                    .into_iter()
                    .collect(),
                raw_input: args,
            })));
            self.tools.insert(
                request.item_id.clone(),
                ToolState {
                    status: ToolCallStatus::Pending,
                    title: title.clone(),
                },
            );
        }
        out.push(Output::Ask {
            tool_call_id: request.item_id.clone(),
            title,
            options: request.available_choices.iter().map(option).collect(),
            approval: Box::new(request),
        });
        out
    }

    fn turn_completed(&mut self, done: TurnCompletedParams) -> Vec<Output> {
        if self
            .turn
            .as_ref()
            .is_none_or(|turn| turn.id != done.turn_id)
        {
            return Vec::new();
        }
        let turn = self.turn.take().expect("checked above");
        let mut out = Vec::new();
        // Muse completes a turn's items before the turn. A tool call still open now never ran
        // to the end, for example one whose approval the cancel cut short.
        let mut open: Vec<String> = self.tools.drain().map(|(id, _)| id).collect();
        open.sort();
        out.extend(open.into_iter().map(|tool_call_id| {
            Output::Event(AgentEvent::ToolCallUpdate(ToolCallUpdate {
                tool_call_id,
                status: Some(ToolCallStatus::Failed),
                title: None,
                kind: None,
                locations: None,
                content: None,
            }))
        }));
        let tokens = done
            .usage
            .as_ref()
            .map(|usage| (usage.input_tokens, usage.output_tokens))
            .or(turn.tokens);
        if let Some((input_tokens, output_tokens)) = tokens {
            out.push(Output::Event(AgentEvent::Usage {
                input_tokens,
                output_tokens,
                cost_usd: None,
            }));
        }
        let (reason, message) =
            match (&done.terminal, done.failure(), &done.error) {
                (TurnTerminal::Completed, _, _) => (TurnEndReason::Completed, None),
                (TurnTerminal::Cancelled, _, _) => (TurnEndReason::Cancelled, done.reason.clone()),
                (_, Some(FailureClass::Limit { resets_at_ms, .. }), Some(error)) => {
                    let message = error.message.clone();
                    out.push(Output::Event(AgentEvent::LimitReached {
                        provider: None,
                        resets_at: i64::try_from(resets_at_ms)
                            .ok()
                            .and_then(|ms| Timestamp::from_unix_millis(ms).ok()),
                        message: message.clone(),
                    }));
                    (TurnEndReason::Limit, Some(message))
                }
                (_, Some(FailureClass::Unavailable), Some(error)) => {
                    (TurnEndReason::Error, Some(unavailable_message(error)))
                }
                (_, _, Some(error)) => (TurnEndReason::Error, Some(error.message.clone())),
                (terminal, _, None) => (
                    TurnEndReason::Error,
                    Some(done.reason.clone().unwrap_or_else(|| {
                        format!("Muse ended the turn as {}", terminal.as_str())
                    })),
                ),
            };
        out.push(Output::Event(AgentEvent::TurnEnded { reason, message }));
        out
    }
}

/// What to tell the user when Muse's provider can't serve the session.
fn unavailable_message(error: &TurnError) -> String {
    if error.kind == TurnErrorKind::AuthRequired {
        format!(
            "Muse Code needs you to sign in: run `muse` in a terminal and log in. ({})",
            error.message
        )
    } else {
        format!("Muse's provider refused the request: {}", error.message)
    }
}

/// A Muse approval choice as a permission option. Its id is the `choiceId`.
fn option(choice: &ApprovalChoice) -> PermissionOption {
    let once = choice.scope == ApprovalChoiceScope::Once;
    let kind = match &choice.decision {
        decision if decision.allows() && once => PermissionOptionKind::AllowOnce,
        decision if decision.allows() => PermissionOptionKind::AllowAlways,
        ApprovalDecision::Denied | ApprovalDecision::Abort if once => {
            PermissionOptionKind::RejectOnce
        }
        ApprovalDecision::Denied
        | ApprovalDecision::Abort
        | ApprovalDecision::DeniedPolicyAmendment => PermissionOptionKind::RejectAlways,
        other => PermissionOptionKind::Other(other.as_str().to_owned()),
    };
    PermissionOption {
        option_id: choice.choice_id.clone(),
        name: choice.label.clone(),
        kind,
    }
}

/// The choice to decide with, given the option the user picked from `asked` and the approval as
/// it is now (`approval/updated` may have replaced the choices): the same id, else the same
/// decision and scope.
pub(super) fn pick_choice(
    asked: &ApprovalRequestParams,
    current: &ApprovalRequestParams,
    choice_id: &str,
) -> Option<ApprovalChoice> {
    if let Some(choice) = current
        .available_choices
        .iter()
        .find(|choice| choice.choice_id == choice_id)
    {
        return Some(choice.clone());
    }
    let picked = asked
        .available_choices
        .iter()
        .find(|choice| choice.choice_id == choice_id)?;
    current
        .available_choices
        .iter()
        .find(|choice| choice.decision == picked.decision && choice.scope == picked.scope)
        .cloned()
}

/// A one-line description of what an approval is for.
fn approval_title(request: &ApprovalRequestParams) -> String {
    let subject = &request.subject;
    let text = |value: &Option<String>| value.as_deref().map(display_path);
    match &subject.kind {
        ApprovalSubjectKind::Shell => {
            let command = subject.command.as_deref().unwrap_or(&request.tool_name);
            let stage = subject
                .stages
                .iter()
                .find(|stage| stage.requirement_id == request.current_requirement_id)
                .filter(|stage| stage.total_stages > 1);
            match stage {
                Some(stage) => format!(
                    "Run `{command}` (step {} of {}: `{}`)",
                    stage.position,
                    stage.total_stages,
                    stage.argv.join(" ")
                ),
                None => format!("Run `{command}`"),
            }
        }
        ApprovalSubjectKind::FileAccess => {
            let verb = match subject.access.as_deref() {
                Some("read") => "Read",
                Some("write") => "Write",
                _ => "Access",
            };
            format!("{verb} {}", text(&subject.path).unwrap_or_default())
        }
        ApprovalSubjectKind::Network => {
            let host = subject.host.as_deref().unwrap_or("the network");
            match subject.port {
                Some(port) => format!("Connect to {host}:{port}"),
                None => format!("Connect to {host}"),
            }
        }
        ApprovalSubjectKind::UnixSocket => {
            format!("Use the socket {}", text(&subject.path).unwrap_or_default())
        }
        ApprovalSubjectKind::Process => format!(
            "Control the process {}",
            subject.target.as_deref().unwrap_or("?")
        ),
        ApprovalSubjectKind::Tool => format!(
            "Use {}",
            subject.tool_name.as_deref().unwrap_or(&request.tool_name)
        ),
        other => format!("{} ({})", request.tool_name, other.as_str()),
    }
}

/// A tool call's arguments, as JSON when they parse, else as a string.
fn parse_args(args: Option<&str>) -> Option<Value> {
    let args = args?;
    Some(serde_json::from_str(args).unwrap_or_else(|_| Value::String(args.to_owned())))
}

fn arg<'a>(args: Option<&'a Value>, names: &[&str]) -> Option<&'a str> {
    let args = args?;
    names
        .iter()
        .find_map(|name| args.get(*name).and_then(Value::as_str))
}

/// The kind of a tool, by its name. Muse 1.4.14's tools include `powershell`, `read_file`,
/// `write_file` and `edit_file`.
fn tool_kind(tool: &str) -> ToolKind {
    let tool = tool.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| tool.contains(word));
    if has(&["web", "fetch", "http", "url", "browse"]) {
        ToolKind::Fetch
    } else if has(&[
        "powershell",
        "shell",
        "bash",
        "terminal",
        "command",
        "exec",
        "run",
    ]) {
        ToolKind::Execute
    } else if has(&["delete", "remove"]) {
        ToolKind::Delete
    } else if has(&["move", "rename"]) {
        ToolKind::Move
    } else if has(&["edit", "write", "patch", "create", "replace"]) {
        ToolKind::Edit
    } else if has(&["grep", "glob", "search", "find"]) {
        ToolKind::Search
    } else if has(&["read", "view", "list", "ls"]) {
        ToolKind::Read
    } else if has(&["think", "plan", "todo"]) {
        ToolKind::Think
    } else {
        ToolKind::Misc
    }
}

fn tool_status(status: &ItemStatus) -> ToolCallStatus {
    match status {
        ItemStatus::InProgress => ToolCallStatus::InProgress,
        ItemStatus::Failed
        | ItemStatus::Rejected
        | ItemStatus::Cancelled
        | ItemStatus::TimedOut => ToolCallStatus::Failed,
        _ => ToolCallStatus::Completed,
    }
}

/// A one-line title: the tool's own label, else one made from its arguments.
fn tool_title(item: &Item, tool: &str, args: Option<&Value>, workspace: &Path) -> String {
    if let Some(label) = &item.display_label {
        return label.neutral.clone();
    }
    if let Some(command) = arg(args, &["command", "cmd"]) {
        return format!("Run `{command}`");
    }
    let path = arg(args, &["path", "file_path", "file"]).map(|path| absolute(path, workspace));
    let target = path.as_deref().map(display_path);
    let verb = match tool_kind(tool) {
        ToolKind::Read => "Read",
        ToolKind::Edit if tool.contains("write") || tool.contains("create") => "Write",
        ToolKind::Edit => "Edit",
        ToolKind::Delete => "Delete",
        ToolKind::Move => "Move",
        ToolKind::Search => {
            return match arg(args, &["pattern", "query", "glob"]) {
                Some(pattern) => format!("Search for `{pattern}`"),
                None => tool.to_owned(),
            };
        }
        ToolKind::Fetch => {
            return match arg(args, &["url", "query"]) {
                Some(url) => format!("Fetch {url}"),
                None => tool.to_owned(),
            };
        }
        _ => return tool.to_owned(),
    };
    match target {
        Some(target) => format!("{verb} {target}"),
        None => tool.to_owned(),
    }
}

fn locations(args: Option<&Value>, workspace: &Path) -> Vec<ToolCallLocation> {
    arg(args, &["path", "file_path", "file"])
        .map(|path| ToolCallLocation {
            path: absolute(path, workspace),
            line: None,
        })
        .into_iter()
        .collect()
}

/// The edit an edit-family call makes, from its arguments, as a diff of the replaced text: ACP's
/// shape (`oldText` absent for a written file). Muse 1.4.14's `edit_file` takes
/// `{path, find, replace}` and `write_file` `{path, content}`; neither sends a stored patch.
fn edit_diff(tool: &str, args: Option<&Value>, workspace: &Path) -> Option<ToolCallContent> {
    if tool_kind(tool) != ToolKind::Edit {
        return None;
    }
    let path = absolute(arg(args, &["path", "file_path", "file"])?, workspace);
    let old = arg(
        args,
        &["find", "old_string", "old_str", "old_text", "search"],
    );
    let new = arg(
        args,
        &[
            "replace",
            "new_string",
            "new_str",
            "new_text",
            "replacement",
        ],
    );
    match (old, new, arg(args, &["content", "contents", "text"])) {
        (Some(old), Some(new), _) => Some(ToolCallContent::Diff {
            path,
            old_text: Some(old.to_owned()),
            new_text: new.to_owned(),
        }),
        (None, None, Some(content)) => Some(ToolCallContent::Diff {
            path,
            old_text: None,
            new_text: content.to_owned(),
        }),
        _ => None,
    }
}

/// What a finished call shows: its visible output, or why it failed, plus the size of a stored
/// patch when there is one.
fn tool_output(item: &Item) -> Vec<ToolCallContent> {
    let mut content = Vec::new();
    let output = item
        .visible_output
        .as_deref()
        .filter(|output| !output.trim().is_empty())
        .or(item.failure_reason.as_deref());
    if let Some(output) = output {
        content.push(text_content(output));
    }
    if let Some(patch) = &item.patch_summary {
        content.push(text_content(&format!(
            "Changed {} file{}: +{} −{}",
            patch.files,
            if patch.files == 1 { "" } else { "s" },
            patch.added,
            patch.removed
        )));
    }
    content
}

fn text_content(text: &str) -> ToolCallContent {
    ToolCallContent::Content {
        content: ContentBlock::text(text),
    }
}

/// `path` made absolute against the workspace, for display and locations.
fn absolute(path: &str, workspace: &Path) -> String {
    let path = Path::new(path);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    display_path(&joined.display().to_string())
}

/// A path without Windows' `\\?\` prefix, which Muse puts on some paths.
pub(super) fn display_path(path: &str) -> String {
    match path.strip_prefix(r"\\?\") {
        Some(rest) if rest.starts_with("UNC\\") => format!(r"\\{}", &rest[4..]),
        Some(rest) => rest.to_owned(),
        None => path.to_owned(),
    }
}

/// A limit a turn ran into, remembered for the whole `burrowd`: an account without an
/// allowance reports no subscription usage, so a later empty `usage/read` must not clear it.
#[derive(Clone, Debug)]
struct KnownLimit {
    resets_at: Option<Timestamp>,
    message: String,
}

static KNOWN_LIMIT: Mutex<Option<KnownLimit>> = Mutex::new(None);

/// Remembers that a turn hit a limit.
pub(super) fn remember_limit(resets_at: Option<Timestamp>, message: &str) {
    *KNOWN_LIMIT.lock().unwrap_or_else(PoisonError::into_inner) = Some(KnownLimit {
        resets_at,
        message: message.to_owned(),
    });
}

/// Forgets a remembered limit, because a turn completed. Whether there was one.
pub(super) fn forget_limit() -> bool {
    KNOWN_LIMIT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .is_some()
}

fn known_limit() -> Option<KnownLimit> {
    let mut known = KNOWN_LIMIT.lock().unwrap_or_else(PoisonError::into_inner);
    if known
        .as_ref()
        .and_then(|limit| limit.resets_at)
        .is_some_and(|resets_at| resets_at <= Timestamp::now())
    {
        *known = None;
    }
    known.clone()
}

/// Muse's subscription usage as Burrow's provider usage, for `usage/get`. `None` is an empty
/// `usage/read`: Muse has observed nothing, which is all an account without an allowance ever
/// gets. That isn't a limit; only a remembered quota failure makes it one.
pub(super) fn provider_usage(reading: Option<&SubscriptionUsage>) -> ProviderUsage {
    let base = ProviderUsage {
        provider: AgentId::Muse.to_string(),
        agent: Some(AgentId::Muse),
        used: None,
        limit: None,
        unit: None,
        limited: false,
        resets_at: None,
        message: None,
        source: UsageSource::Reported,
        updated_at: Timestamp::now(),
    };
    let millis = |ms: u64| {
        i64::try_from(ms)
            .ok()
            .and_then(|ms| Timestamp::from_unix_millis(ms).ok())
    };
    let Some(usage) = reading else {
        return match known_limit() {
            Some(limit) => ProviderUsage {
                limited: true,
                resets_at: limit.resets_at,
                message: Some(limit.message),
                ..base
            },
            None => ProviderUsage {
                message: Some(
                    "Muse has observed no subscription usage; accounts without an allowance never report any"
                        .into(),
                ),
                ..base
            },
        };
    };
    let exhausted = usage.exhausted_until_ms();
    ProviderUsage {
        used: Some(usage.window.used_percent as f64),
        limit: Some(100.0),
        unit: Some("percent".into()),
        limited: exhausted.is_some(),
        resets_at: millis(exhausted.unwrap_or(usage.window.resets_at_ms)),
        message: Some(format!(
            "{}: {}% of the {}-minute window and {}% of the week used",
            usage.tier,
            usage.window.used_percent,
            usage.window.window_duration_mins,
            usage.weekly.used_percent
        )),
        updated_at: millis(usage.observed_at_ms).unwrap_or(base.updated_at),
        ..base
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const SESSION: &str = "s1";
    const TURN: &str = "t1";

    fn translator() -> Translator {
        let mut translator = Translator::new(SESSION, r"C:\work\ws");
        translator.begin_turn(TURN);
        translator
    }

    /// Decodes a notification the way `MspClient` does, for the kinds these tests use.
    fn event(method: &str, params: Value) -> Event {
        match method {
            "item/started" => Event::ItemStarted(serde_json::from_value(params).unwrap()),
            "item/updated" => Event::ItemUpdated(serde_json::from_value(params).unwrap()),
            "item/completed" => Event::ItemCompleted(serde_json::from_value(params).unwrap()),
            "turn/completed" => Event::TurnCompleted(serde_json::from_value(params).unwrap()),
            "session/tokenUsage" => Event::TokenUsage(serde_json::from_value(params).unwrap()),
            "approval/request" => Event::ApprovalRequested(serde_json::from_value(params).unwrap()),
            other => panic!("no decoder for {other}"),
        }
    }

    fn delta(item: &str, kind: ItemKind, field: Option<&str>, text: &str) -> Event {
        Event::ItemDelta {
            delta: ItemDeltaParams {
                delta: text.into(),
                field: field.map(str::to_owned),
                item_id: item.into(),
                session_id: SESSION.into(),
                view_cursor: "v".into(),
            },
            kind: Some(kind),
        }
    }

    fn item(value: Value) -> Event {
        let status = value["status"].as_str().unwrap_or("inProgress").to_owned();
        let method = if status == "inProgress" {
            "item/started"
        } else {
            "item/completed"
        };
        let mut item = value;
        item["turnId"] = json!(TURN);
        item["revision"] = json!(1);
        item["status"] = json!(status);
        event(
            method,
            json!({"sessionId": SESSION, "viewCursor": "v", "item": item}),
        )
    }

    fn events(outputs: Vec<Output>) -> Vec<AgentEvent> {
        outputs
            .into_iter()
            .map(|output| match output {
                Output::Event(event) => event,
                other => panic!("expected an event, got {other:?}"),
            })
            .collect()
    }

    fn text(events: &[AgentEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::MessageChunk(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn messages_stream_and_a_second_message_starts_a_paragraph() {
        let mut t = translator();
        let mut all = Vec::new();
        // A commentary message that arrives whole, then a streamed answer.
        all.extend(events(t.on_event(item(json!({
            "itemId": "m1", "kind": "agentMessage", "status": "completed",
            "text": "I'll do it.", "messagePhase": "commentary"
        })))));
        all.extend(events(t.on_event(item(json!({
            "itemId": "m2", "kind": "agentMessage", "text": ""
        })))));
        for piece in ["Done", ", all", " good."] {
            all.extend(events(t.on_event(delta(
                "m2",
                ItemKind::AgentMessage,
                Some("text"),
                piece,
            ))));
        }
        all.extend(events(t.on_event(item(json!({
            "itemId": "m2", "kind": "agentMessage", "status": "completed",
            "text": "Done, all good."
        })))));
        assert_eq!(text(&all), "I'll do it.\n\nDone, all good.");
        assert_eq!(all.len(), 4, "{all:?}");
    }

    #[test]
    fn reasoning_summaries_become_thoughts() {
        let mut t = translator();
        let mut all = Vec::new();
        for (field, piece) in [
            ("summary.0", "Think"),
            ("summary.0", "ing"),
            ("summary.1", "More"),
        ] {
            all.extend(events(t.on_event(delta(
                "r1",
                ItemKind::Reasoning,
                Some(field),
                piece,
            ))));
        }
        all.extend(events(t.on_event(item(json!({
            "itemId": "r1", "kind": "reasoning", "status": "completed",
            "summary": ["Thinking", "More"]
        })))));
        // A reasoning item that arrives whole.
        all.extend(events(t.on_event(item(json!({
            "itemId": "r2", "kind": "reasoning", "status": "completed", "summary": ["Done"]
        })))));
        let thoughts: String = all
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ThoughtChunk(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(thoughts, "Thinking\n\nMore\n\nDone");
    }

    #[test]
    fn items_of_other_turns_and_sessions_are_ignored() {
        let mut t = translator();
        let mut other = item(
            json!({"itemId": "m", "kind": "agentMessage", "status": "completed", "text": "x"}),
        );
        if let Event::ItemCompleted(params) = &mut other {
            params.item.turn_id = Some("t0".into());
        }
        assert!(t.on_event(other).is_empty());
        let mut elsewhere = item(
            json!({"itemId": "m", "kind": "agentMessage", "status": "completed", "text": "x"}),
        );
        if let Event::ItemCompleted(params) = &mut elsewhere {
            params.session_id = "s2".into();
        }
        assert!(t.on_event(elsewhere).is_empty());
        let reminder = item(json!({"itemId": "c", "kind": "reminderChild"}));
        assert!(t.on_event(reminder).is_empty());
    }

    #[test]
    fn an_edit_becomes_a_tool_call_with_a_diff() {
        let mut t = translator();
        let args = r#"{"find":"one","path":"notes.txt","replace":"two"}"#;
        let started = events(t.on_event(item(json!({
            "itemId": "e1", "kind": "toolCall", "tool": "edit_file", "args": args
        }))));
        let [AgentEvent::ToolCall(call)] = started.as_slice() else {
            panic!("{started:?}")
        };
        assert_eq!(call.tool_call_id, "e1");
        assert_eq!(call.kind, ToolKind::Edit);
        assert_eq!(call.status, ToolCallStatus::InProgress);
        assert_eq!(call.title, r"Edit C:\work\ws\notes.txt");
        assert_eq!(call.locations[0].path, r"C:\work\ws\notes.txt");
        let diff = ToolCallContent::Diff {
            path: r"C:\work\ws\notes.txt".into(),
            old_text: Some("one".into()),
            new_text: "two".into(),
        };
        assert_eq!(call.content, std::slice::from_ref(&diff));

        let done = events(t.on_event(item(json!({
            "itemId": "e1", "kind": "toolCall", "tool": "edit_file", "args": args,
            "status": "completed", "visibleOutput": "edited\n-one\n+two\n"
        }))));
        let [AgentEvent::ToolCallUpdate(update)] = done.as_slice() else {
            panic!("{done:?}")
        };
        assert_eq!(update.status, Some(ToolCallStatus::Completed));
        assert_eq!(update.title, None);
        assert_eq!(
            update.content.as_deref(),
            Some([diff, text_content("edited\n-one\n+two\n")].as_slice())
        );
    }

    #[test]
    fn a_failed_command_reports_why() {
        let mut t = translator();
        t.on_event(item(json!({
            "itemId": "c1", "kind": "toolCall", "tool": "powershell",
            "args": r#"{"command":"echo hi"}"#
        })));
        let done = events(t.on_event(item(json!({
            "itemId": "c1", "kind": "toolCall", "tool": "powershell", "status": "failed",
            "args": r#"{"command":"echo hi"}"#, "failureReason": "sandbox setup_required"
        }))));
        let [AgentEvent::ToolCallUpdate(update)] = done.as_slice() else {
            panic!("{done:?}")
        };
        assert_eq!(update.status, Some(ToolCallStatus::Failed));
        assert_eq!(
            update.content.as_deref(),
            Some([text_content("sandbox setup_required")].as_slice())
        );
    }

    fn approval(choices: Value) -> Event {
        event(
            "approval/request",
            json!({
                "sessionId": SESSION, "approvalId": "a1", "turnId": TURN, "taskId": "a1",
                "itemId": "c1", "toolCallId": "call_1", "toolName": "powershell",
                "rawArgs": r#"{"command":"echo hi"}"#, "viewCursor": "v",
                "judgeEscalated": false, "protectedWrite": false,
                "currentRequirementId": {"approvalId": "a1", "sourceIndex": 0},
                "subject": {"kind": "shell", "command": "echo hi", "stages": []},
                "availableChoices": choices
            }),
        )
    }

    fn recorded_choices() -> Value {
        json!([
            {"choiceId": "allow_once", "label": "Allow once", "decision": "approved", "scope": "once"},
            {"choiceId": "allow_local_prefix", "label": "Always allow in this workspace: echo ...",
             "decision": "approvedPolicyAmendment", "scope": "localPersistent"},
            {"choiceId": "abort", "label": "Reject", "decision": "abort", "scope": "once"}
        ])
    }

    #[test]
    fn an_approval_announces_its_tool_call_then_asks() {
        let mut t = translator();
        let mut outputs = t.on_event(approval(recorded_choices()));
        assert_eq!(outputs.len(), 2, "{outputs:?}");
        let Output::Ask {
            tool_call_id,
            title,
            options,
            approval,
        } = outputs.pop().unwrap()
        else {
            panic!("expected a question")
        };
        let [Output::Event(AgentEvent::ToolCall(call))] = outputs.as_slice() else {
            panic!("{outputs:?}")
        };
        assert_eq!(call.status, ToolCallStatus::Pending);
        assert_eq!(call.kind, ToolKind::Execute);
        assert_eq!(
            (tool_call_id.as_str(), title.as_str()),
            ("c1", "Run `echo hi`")
        );
        assert_eq!(approval.approval_id, "a1");
        let kinds: Vec<_> = options
            .iter()
            .map(|o| (o.option_id.as_str(), o.kind.clone()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("allow_once", PermissionOptionKind::AllowOnce),
                ("allow_local_prefix", PermissionOptionKind::AllowAlways),
                ("abort", PermissionOptionKind::RejectOnce),
            ]
        );

        // The tool call starts once approved: an update, not a second announcement.
        let started = events(t.on_event(item(json!({
            "itemId": "c1", "kind": "toolCall", "tool": "powershell",
            "args": r#"{"command":"echo hi"}"#
        }))));
        assert!(
            matches!(started.as_slice(), [AgentEvent::ToolCallUpdate(u)] if u.status == Some(ToolCallStatus::InProgress)),
            "{started:?}"
        );
    }

    #[test]
    fn a_tool_call_left_open_fails_when_its_turn_ends() {
        let mut t = translator();
        t.on_event(approval(recorded_choices()));
        // Cancelled while Muse waited for the answer: the call never started.
        let end = events(t.on_event(completed("cancelled", None)));
        assert!(
            matches!(
                end.as_slice(),
                [
                    AgentEvent::ToolCallUpdate(update),
                    AgentEvent::TurnEnded { reason: TurnEndReason::Cancelled, .. },
                ] if update.tool_call_id == "c1" && update.status == Some(ToolCallStatus::Failed)
            ),
            "{end:?}"
        );
        // Nothing is left over for the next turn.
        t.begin_turn(TURN);
        let end = events(t.on_event(completed("completed", None)));
        assert_eq!(end.len(), 1, "{end:?}");
    }

    #[test]
    fn a_choice_is_found_again_after_the_choices_change() {
        let Event::ApprovalRequested(asked) = approval(recorded_choices()) else {
            unreachable!()
        };
        let mut current = asked.clone();
        current.available_choices[0].choice_id = "allow_once_v2".into();
        let picked = pick_choice(&asked, &current, "allow_once").unwrap();
        assert_eq!(picked.choice_id, "allow_once_v2");
        assert_eq!(
            pick_choice(&asked, &current, "abort").unwrap().choice_id,
            "abort"
        );
        assert!(pick_choice(&asked, &current, "nope").is_none());
    }

    fn completed(terminal: &str, error: Option<Value>) -> Event {
        let mut params = json!({
            "sessionId": SESSION, "viewCursor": "v", "turnId": TURN, "terminal": terminal
        });
        if let Some(error) = error {
            params["error"] = error;
        }
        event("turn/completed", params)
    }

    fn tokens(input: u64, output: u64) -> Event {
        event(
            "session/tokenUsage",
            json!({
                "sessionId": SESSION, "viewCursor": "v", "turnId": TURN, "modelId": null,
                "usage": {"inputTokens": input, "outputTokens": output, "cachedTokens": 0, "reasoningTokens": 0},
                "promptTokens": input, "totalTokens": input + output,
                "cumulative": {"promptTokens": input, "outputTokens": output, "totalTokens": input + output}
            }),
        )
    }

    #[test]
    fn a_turn_ends_with_its_summed_usage() {
        let mut t = translator();
        assert!(t.on_event(tokens(100, 10)).is_empty());
        assert!(t.on_event(tokens(200, 20)).is_empty());
        let end = events(t.on_event(completed("completed", None)));
        assert!(
            matches!(
                end.as_slice(),
                [
                    AgentEvent::Usage {
                        input_tokens: 300,
                        output_tokens: 30,
                        cost_usd: None
                    },
                    AgentEvent::TurnEnded {
                        reason: TurnEndReason::Completed,
                        message: None
                    },
                ]
            ),
            "{end:?}"
        );
        assert_eq!(t.turn_id(), None);
        // A second turn/completed for the same turn is ignored.
        assert!(t.on_event(completed("completed", None)).is_empty());
    }

    #[test]
    fn a_quota_failure_is_a_limit_and_auth_is_a_clear_error() {
        let mut t = translator();
        let end = events(t.on_event(completed(
            "failed",
            Some(json!({
                "kind": "modelError", "message": "quota exhausted", "retryable": false,
                "quota": {"limitKind": "tierLimit", "resetsAtMs": 1_794_210_000_000u64}
            })),
        )));
        let [
            AgentEvent::LimitReached {
                provider: None,
                resets_at: Some(resets_at),
                message,
            },
            AgentEvent::TurnEnded {
                reason: TurnEndReason::Limit,
                ..
            },
        ] = end.as_slice()
        else {
            panic!("{end:?}")
        };
        assert_eq!(resets_at.as_unix_millis(), 1_794_210_000_000);
        assert_eq!(message, "quota exhausted");

        t.begin_turn(TURN);
        let end = events(t.on_event(completed(
            "failed",
            Some(json!({"kind": "authRequired", "message": "log in", "retryable": false})),
        )));
        let [
            AgentEvent::TurnEnded {
                reason: TurnEndReason::Error,
                message: Some(message),
            },
        ] = end.as_slice()
        else {
            panic!("{end:?}")
        };
        assert!(message.contains("sign in"), "{message}");

        t.begin_turn(TURN);
        let end = events(t.on_event(completed("cancelled", None)));
        assert!(matches!(
            end.as_slice(),
            [AgentEvent::TurnEnded {
                reason: TurnEndReason::Cancelled,
                ..
            }]
        ));
    }

    #[test]
    fn subscription_usage_is_a_percentage_and_an_empty_reading_is_not_a_limit() {
        let empty = provider_usage(None);
        assert_eq!(empty.provider, "muse");
        assert!(!empty.limited);
        assert!(empty.message.unwrap().contains("no subscription usage"));

        let reading: SubscriptionUsage = serde_json::from_value(json!({
            "observedAtMs": 1_794_000_000_000u64, "tier": "pro",
            "window": {"resetsAtMs": 1_794_010_000_000u64, "usedPercent": 100, "windowDurationMins": 300},
            "weekly": {"resetsAtMs": 1_794_500_000_000u64, "usedPercent": 40}
        }))
        .unwrap();
        let usage = provider_usage(Some(&reading));
        assert!(usage.limited);
        assert_eq!((usage.used, usage.limit), (Some(100.0), Some(100.0)));
        assert_eq!(usage.unit.as_deref(), Some("percent"));
        assert_eq!(usage.resets_at.unwrap().as_unix_millis(), 1_794_010_000_000);
    }

    #[test]
    fn titles_kinds_and_paths() {
        assert_eq!(tool_kind("powershell"), ToolKind::Execute);
        assert_eq!(tool_kind("read_file"), ToolKind::Read);
        assert_eq!(tool_kind("write_file"), ToolKind::Edit);
        assert_eq!(tool_kind("web_search"), ToolKind::Fetch);
        assert_eq!(tool_kind("grep"), ToolKind::Search);
        assert_eq!(tool_kind("mystery"), ToolKind::Misc);
        assert_eq!(display_path(r"\\?\C:\work\ws"), r"C:\work\ws");
        assert_eq!(display_path(r"\\?\UNC\server\share"), r"\\server\share");
        let write = edit_diff(
            "write_file",
            Some(&json!({"path": r"D:\other\hello.txt", "content": "hi\n"})),
            Path::new(r"C:\work\ws"),
        );
        assert_eq!(
            write,
            Some(ToolCallContent::Diff {
                path: r"D:\other\hello.txt".into(),
                old_text: None,
                new_text: "hi\n".into()
            })
        );
    }
}
