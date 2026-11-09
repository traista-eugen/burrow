//! Agent sessions: the `session/*` methods, [`Session`], [`ContentBlock`], and the events of a
//! `session:<id>` stream.
//!
//! Session events deliberately mirror ACP's `session/update` kinds, so the ACP adapter is mostly
//! a mapping; the MSP and terminal adapters map into the same set. Enums that mirror ACP values
//! are open, so an adapter can pass on a value a newer agent adds.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{AgentId, ModelInfo};
use crate::artifact::ArtifactRef;
use crate::events::InboxItemStatus;
use crate::time::Timestamp;

/// Whether a session is structured or runs the agent's own TUI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SessionMode {
    /// Through ACP or MSP: Burrow sees messages, tool calls and permission requests.
    #[default]
    Structured,
    /// The agent's own TUI in a pseudo-terminal. Not available until M4.
    Terminal,
}

open_enum! {
    /// Where a session is in its life.
    pub enum SessionState {
        /// The agent process is starting.
        Starting = "starting",
        /// Ready for a prompt.
        Idle = "idle",
        /// A turn is running.
        Working = "working",
        /// A turn waits on a human: an inbox item is open.
        Waiting = "waiting",
        /// A provider limit was hit. `Session.limit` says when it resets, if known.
        Limited = "limited",
        /// The agent process was stopped: by `session/stop`, or because `burrowd` stopped.
        Stopped = "stopped",
        /// The agent couldn't start, or exited unexpectedly. `Session.reason` says why.
        Failed = "failed",
    }
}

impl SessionState {
    /// Whether the session has a running agent process: every state but `stopped` and `failed`.
    pub fn is_live(&self) -> bool {
        !matches!(self, Self::Stopped | Self::Failed)
    }
}

/// A piece of prompt or message content. Follows ACP, except that images are artifact
/// references instead of inline data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlock {
    /// Text, usually Markdown.
    Text {
        /// The text.
        text: String,
    },
    /// An image from the artifact store. Defined, but `session/prompt` doesn't accept it yet.
    Image {
        /// The image.
        artifact: ArtifactRef,
    },
    /// A resource, such as a file, by URI. Defined, but `session/prompt` doesn't accept it yet.
    Resource {
        /// The resource's URI, for example `file:///D:/repo/README.md`.
        uri: String,
    },
}

impl ContentBlock {
    /// A text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// The text, if this is a text block.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }
}

/// One conversation with one agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// The session's id (`ses_…`).
    pub id: String,
    /// The project it belongs to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The agent.
    pub agent: AgentId,
    /// The model, when chosen or reported by the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Structured or terminal.
    pub mode: SessionMode,
    /// The folder the agent works in.
    pub cwd: String,
    /// The display name. Set from the first prompt when `session/create` didn't give one;
    /// `session/rename` changes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The current state.
    pub state: SessionState,
    /// Why the session stopped or failed. Human-readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The running turn, while `working` or `waiting`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Models the agent offers in this session, when it says.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelInfo>,
    /// The agent's own mode, for example Claude Code's `default` or Codex's `read-only`. It
    /// decides what the agent asks permission for. Absent when the agent has no modes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_mode: Option<String>,
    /// The modes `session/setAgentMode` accepts, when the agent has modes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_modes: Vec<AgentMode>,
    /// Tokens and cost so far, when the agent reports them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SessionUsage>,
    /// The limit that was hit, while `limited`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<LimitInfo>,
    /// Archived sessions leave the default lists.
    #[serde(default, skip_serializing_if = "is_false")]
    pub archived: bool,
    /// When the session was created.
    pub created_at: Timestamp,
    /// When its state, title, model or usage last changed.
    pub updated_at: Timestamp,
    /// When a turn last started or ended; `createdAt` until the first prompt. `session/list`
    /// sorts by it, most recent first. `burrowd` always sets it; it is optional only so that
    /// sessions stored before it existed still decode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<Timestamp>,
}

impl Session {
    /// When a turn last started or ended, or when the session was created if none has: the key
    /// `session/list` sorts by.
    pub fn activity(&self) -> Timestamp {
        self.last_activity_at.unwrap_or(self.created_at)
    }
}

/// One of an agent's own modes, such as "ask before edits" or "plan only".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentMode {
    /// The id to pass as `agentMode`, for example `read-only`.
    pub id: String,
    /// A display name, for example `Read Only`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What the mode does, as the agent describes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A session's totals so far.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    /// Input tokens, summed over every `usage` event.
    pub input_tokens: u64,
    /// Output tokens, summed over every `usage` event.
    pub output_tokens: u64,
    /// Cost in US dollars, when the agent reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// A provider limit that was hit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LimitInfo {
    /// The provider whose limit it is, for example `claude`.
    pub provider: String,
    /// When it resets, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
    /// The provider's message.
    pub message: String,
}

/// A session in a list: enough for a sidebar entry. `session/get` has the rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// The session's id.
    pub id: String,
    /// The project it belongs to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The agent.
    pub agent: AgentId,
    /// The model, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The current state.
    pub state: SessionState,
    /// Whether it is archived.
    #[serde(default, skip_serializing_if = "is_false")]
    pub archived: bool,
    /// When the session was created.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// When a turn last started or ended; `createdAt` until the first prompt. Lists are sorted
    /// by it, most recent first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<Timestamp>,
}

impl From<&Session> for SessionSummary {
    fn from(session: &Session) -> Self {
        Self {
            id: session.id.clone(),
            project_id: session.project_id.clone(),
            agent: session.agent.clone(),
            model: session.model.clone(),
            title: session.title.clone(),
            state: session.state.clone(),
            archived: session.archived,
            created_at: session.created_at,
            updated_at: session.updated_at,
            last_activity_at: Some(session.activity()),
        }
    }
}

/// `session/list` params. Every filter is optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionListParams {
    /// Only sessions of this project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Only sessions in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SessionState>,
    /// Include archived sessions, which are left out by default.
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_archived: bool,
    /// At most this many. Default 100, at most 1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Only sessions that come after this one in the list's order (most recent activity
    /// first): pass the last id of the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// `session/create` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreateParams {
    /// The project to work in. Its folder is the default `cwd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The agent to start.
    pub agent: AgentId,
    /// The model to use; the agent's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Structured (the default) or terminal. Terminal sessions are `unavailable` until M4.
    #[serde(default)]
    pub mode: SessionMode,
    /// The agent's own mode to start in, for example `read-only`. When absent, the agent's
    /// `defaultAgentMode` from `agent/list`, or else the agent's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_mode: Option<String>,
    /// The folder the agent works in, as an absolute path. Required without `projectId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Params naming one session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionIdParams {
    /// The session's id.
    pub session_id: String,
}

/// `session/prompt` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptParams {
    /// The session.
    pub session_id: String,
    /// The prompt. Only text blocks are accepted for now.
    pub content: Vec<ContentBlock>,
}

/// `session/prompt` result. The turn's output arrives as events on the session's stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptResult {
    /// The new turn's id (`trn_…`). Its events carry it, and `turnEnded` closes it.
    pub turn_id: String,
}

/// `session/setModel` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionSetModelParams {
    /// The session.
    pub session_id: String,
    /// The model's id, as the agent knows it.
    pub model: String,
}

/// `session/setAgentMode` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionSetAgentModeParams {
    /// The session.
    pub session_id: String,
    /// The mode's id, one of `Session.agentModes`.
    pub agent_mode: String,
}

/// `session/rename` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionRenameParams {
    /// The session.
    pub session_id: String,
    /// The new display name: not empty, at most 200 characters. Surrounding whitespace is
    /// trimmed.
    pub title: String,
}

// ---- Session events ----------------------------------------------------------------------------

/// `userMessage`: the prompt that started a turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    /// The turn it started.
    pub turn_id: String,
    /// The prompt.
    pub content: Vec<ContentBlock>,
}

/// `agentMessageChunk`: a piece of the agent's answer. Stored in chunks of about 100 ms; a
/// client appends consecutive chunks of a turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageChunk {
    /// The turn.
    pub turn_id: String,
    /// The text, usually Markdown, to append to the answer so far.
    pub text: String,
}

/// `agentThoughtChunk`: a piece of the agent's visible reasoning.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentThoughtChunk {
    /// The turn.
    pub turn_id: String,
    /// The text to append.
    pub text: String,
}

open_enum! {
    /// What a tool call does, so a client can pick an icon. Mirrors ACP's tool kinds.
    pub enum ToolKind {
        /// Reads files or data.
        Read = "read",
        /// Changes files.
        Edit = "edit",
        /// Deletes files.
        Delete = "delete",
        /// Moves or renames files.
        Move = "move",
        /// Searches.
        Search = "search",
        /// Runs a command.
        Execute = "execute",
        /// Thinks.
        Think = "think",
        /// Fetches from the network.
        Fetch = "fetch",
        /// Switches the agent's mode, for example out of planning.
        SwitchMode = "switchMode",
        /// Anything else. (Named `Misc` in Rust because `Other` holds unknown values.)
        Misc = "other",
    }
}

open_enum! {
    /// How far a tool call has got. Mirrors ACP's statuses.
    pub enum ToolCallStatus {
        /// Not started, for example waiting for permission.
        Pending = "pending",
        /// Running.
        InProgress = "inProgress",
        /// Finished.
        Completed = "completed",
        /// Failed or was refused.
        Failed = "failed",
    }
}

/// A file a tool call touches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallLocation {
    /// The file's absolute path.
    pub path: String,
    /// A 1-based line, if relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// What a tool call produced or proposes. Mirrors ACP's tool call content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ToolCallContent {
    /// Ordinary content, such as text output.
    Content {
        /// The content.
        content: ContentBlock,
    },
    /// A change to a file.
    Diff {
        /// The file's absolute path.
        path: String,
        /// The text before the change; absent for a new file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_text: Option<String>,
        /// The text after the change.
        new_text: String,
    },
    /// A terminal the tool runs in (M4).
    Terminal {
        /// The terminal's id.
        terminal_id: String,
    },
}

/// `toolCall`: the agent started, or asks to start, a tool call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// The turn it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// The tool call's id, unique within the session. `toolCallUpdate` refers to it.
    pub tool_call_id: String,
    /// A one-line description, for example `Edit README.md`.
    pub title: String,
    /// What it does.
    pub kind: ToolKind,
    /// How far it has got.
    pub status: ToolCallStatus,
    /// Files it touches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<ToolCallLocation>,
    /// What it produced or proposes so far.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<ToolCallContent>,
    /// The tool's raw input, as the agent sent it, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<Value>,
}

/// `toolCallUpdate`: a tool call changed. Absent fields are unchanged; present lists replace the
/// old ones.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallUpdate {
    /// The tool call.
    pub tool_call_id: String,
    /// The new status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolCallStatus>,
    /// The new title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The new kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ToolKind>,
    /// The new locations, replacing the old ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<ToolCallLocation>>,
    /// The new content, replacing the old: text, a diff or a terminal reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ToolCallContent>>,
}

open_enum! {
    /// A plan entry's progress.
    pub enum PlanEntryStatus {
        /// Not started.
        Pending = "pending",
        /// Being worked on.
        InProgress = "inProgress",
        /// Done.
        Completed = "completed",
    }
}

open_enum! {
    /// A plan entry's priority.
    pub enum PlanEntryPriority {
        /// High.
        High = "high",
        /// Medium.
        Medium = "medium",
        /// Low.
        Low = "low",
    }
}

/// One step of an agent's plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    /// What the step is.
    pub content: String,
    /// Its progress.
    pub status: PlanEntryStatus,
    /// Its priority.
    pub priority: PlanEntryPriority,
}

/// `plan`: the agent's current plan, replacing any earlier one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// Every entry, in order.
    pub entries: Vec<PlanEntry>,
}

open_enum! {
    /// What a permission option does. Mirrors ACP's option kinds.
    pub enum PermissionOptionKind {
        /// Allow this once.
        AllowOnce = "allowOnce",
        /// Allow this and similar calls from now on.
        AllowAlways = "allowAlways",
        /// Refuse this once.
        RejectOnce = "rejectOnce",
        /// Refuse this and similar calls from now on.
        RejectAlways = "rejectAlways",
    }
}

/// One answer the agent offers to a permission request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    /// The option's id. It is also the inbox item's action id.
    pub option_id: String,
    /// Its label, for example `Allow once`.
    pub name: String,
    /// What it does.
    pub kind: PermissionOptionKind,
}

/// `permissionRequested`: the agent asks before a tool call. The question is inbox item
/// `inboxItemId`, answered with `inbox/respond`; the session is `waiting` until then.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequested {
    /// The inbox item that holds the question.
    pub inbox_item_id: String,
    /// The turn it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// The tool call it is about.
    pub tool_call_id: String,
    /// What the agent wants to do, for example `Edit README.md`.
    pub title: String,
    /// The answers the agent offers.
    pub options: Vec<PermissionOption>,
}

/// `permissionResolved`: a permission request was answered, or expired because the turn was
/// cancelled, the session stopped or `burrowd` restarted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PermissionResolved {
    /// The inbox item.
    pub inbox_item_id: String,
    /// The tool call it was about.
    pub tool_call_id: String,
    /// `resolved` or `expired`.
    pub status: InboxItemStatus,
    /// The option chosen. Absent when it expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    /// Who answered: a device id, or `local`. Absent when it expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

open_enum! {
    /// Why a turn ended.
    pub enum TurnEndReason {
        /// The agent finished.
        Completed = "completed",
        /// `session/cancel`, or the session was stopped.
        Cancelled = "cancelled",
        /// A provider limit was hit; see `limitReached`.
        Limit = "limit",
        /// Something went wrong; `message` says what.
        Error = "error",
    }
}

/// `turnEnded`: a turn is over; the session is `idle` again, or `limited`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnEnded {
    /// The turn.
    pub turn_id: String,
    /// Why it ended.
    pub reason: TurnEndReason,
    /// More detail, for example the error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `usage`: tokens and cost of one turn, as the agent reports them. `Session.usage` holds the
/// totals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// The turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cost in US dollars, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// `limitReached`: a provider limit was hit. The session becomes `limited`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LimitReached {
    /// The provider, for example `claude`.
    pub provider: String,
    /// When the limit resets, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
    /// The provider's message.
    pub message: String,
}

/// `stateChanged`: the session moved to a new state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StateChanged {
    /// The new state.
    pub state: SessionState,
    /// Why, when it isn't obvious: for example why it stopped or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `modelChanged`: the session now uses another model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelChanged {
    /// The model's id.
    pub model: String,
}

/// `agentModeChanged`: the agent now runs in another of its modes, because
/// `session/setAgentMode` asked or the agent switched by itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentModeChanged {
    /// The mode's id.
    pub agent_mode: String,
}

/// `terminalOpened`: a terminal belongs to this session (M4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpened {
    /// The terminal's id, for `terminal/attach`.
    pub terminal_id: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}
