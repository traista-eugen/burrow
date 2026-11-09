//! The crate's own typed view of ACP: what an agent reports about itself and
//! its sessions, and the events a session streams.
//!
//! These types are deliberately simpler than the `agent-client-protocol`
//! schema, and independent of it, so that Burrow's adapter keeps compiling when
//! the SDK changes (it had five major releases in 2026). Anything an agent
//! sends that doesn't fit is kept as raw JSON instead of being dropped.

use std::path::PathBuf;

use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::v1::schema as acp;
use serde_json::Value;

use crate::limit::{LimitInfo, RateLimitStatus};

/// What the agent said about itself in `initialize`.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentInfo {
    /// `agentInfo.name`, for example `claude-agent-acp`.
    pub name: Option<String>,
    /// `agentInfo.title`, a display name.
    pub title: Option<String>,
    /// `agentInfo.version`.
    pub version: Option<String>,
    /// The ACP protocol version the agent chose.
    pub protocol_version: u16,
    /// The agent's capabilities.
    pub capabilities: AgentCapabilities,
    /// How to authenticate, if the agent needs it. Usually not needed when
    /// the user is already logged in to the agent's own CLI.
    pub auth_methods: Vec<AuthMethod>,
    /// The raw `initialize` result, for anything not modelled here.
    pub raw: Value,
}

/// The subset of `agentCapabilities` Burrow cares about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCapabilities {
    /// `session/load`: reopen a session and replay its history as updates.
    pub load_session: bool,
    /// `session/resume`: reopen a session without replaying history.
    pub resume_session: bool,
    /// `session/close`.
    pub close_session: bool,
    /// `session/list`.
    pub list_sessions: bool,
    /// Prompts may contain images.
    pub prompt_image: bool,
    /// Prompts may contain audio.
    pub prompt_audio: bool,
    /// Prompts may embed resources (file contents).
    pub prompt_embedded_context: bool,
    /// MCP servers over HTTP can be passed to new sessions.
    pub mcp_http: bool,
    /// MCP servers over SSE can be passed to new sessions.
    pub mcp_sse: bool,
}

/// One way the agent offers to authenticate.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthMethod {
    /// The id to pass to [`crate::AcpClient::authenticate`].
    pub id: String,
    /// Display name.
    pub name: String,
    /// Longer description.
    pub description: Option<String>,
    /// `true` when the method is not handled by `authenticate` but by running
    /// the agent's program interactively in a terminal with [`Self::args`].
    pub terminal: bool,
    /// Arguments for a terminal method.
    pub args: Vec<String>,
}

/// A session the agent created, loaded or resumed.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    /// The agent's session id.
    pub session_id: String,
    /// The current mode and the modes to choose from, if the agent has modes.
    pub modes: Option<ModeState>,
    /// The current model and the models to choose from, if the agent lets
    /// the client pick one.
    pub models: Option<ModelState>,
    /// Every session config option the agent offers (model, mode, effort, …).
    pub config_options: Vec<ConfigOption>,
    /// The raw result, for anything not modelled here.
    pub raw: Value,
}

/// Session modes, such as "ask", "code" or "plan".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeState {
    /// The current mode's id.
    pub current: String,
    /// Every mode the session offers.
    pub available: Vec<Choice>,
}

/// The session's model and the models to choose from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelState {
    /// The current model's id.
    pub current: String,
    /// Every model the session offers.
    pub available: Vec<Choice>,
    /// How the model is changed: through a config option with this id, or
    /// (when `None`) through the older `session/set_model` method.
    pub config_id: Option<String>,
}

/// One choice in a list of modes, models or config values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The id to send back.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Longer description.
    pub description: Option<String>,
}

/// A session config option: a selector the agent offers, such as the model,
/// the mode or the reasoning effort.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigOption {
    /// The id to pass to [`crate::AcpClient::set_config_option`].
    pub id: String,
    /// Display name.
    pub name: String,
    /// Longer description.
    pub description: Option<String>,
    /// What the option controls.
    pub category: ConfigCategory,
    /// The current value and the values to choose from.
    pub value: ConfigValue,
}

/// What a [`ConfigOption`] controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigCategory {
    /// The session mode.
    Mode,
    /// The model.
    Model,
    /// A model setting other than the model itself.
    ModelConfig,
    /// Reasoning effort.
    ThoughtLevel,
    /// Anything else, or no category.
    Other(String),
}

/// A [`ConfigOption`]'s value.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    /// One of a list of values.
    Select {
        /// The current value's id.
        current: String,
        /// Every value, flattened across groups.
        options: Vec<Choice>,
    },
    /// On or off.
    Boolean(bool),
    /// A kind of option this crate doesn't know.
    Other(Value),
}

/// Content in a prompt: what the caller sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptContent {
    /// Plain text.
    Text(String),
    /// An image, base64-encoded. The agent must support images.
    Image {
        /// For example `image/png`.
        mime_type: String,
        /// The image bytes, base64-encoded.
        data: String,
    },
    /// A link to a resource the agent can read itself, such as a file URI.
    ResourceLink {
        /// The resource's URI, for example `file:///C:/repo/src/main.rs`.
        uri: String,
        /// Display name.
        name: String,
    },
    /// A resource's text, embedded. The agent must support embedded context.
    Resource {
        /// The resource's URI.
        uri: String,
        /// Its text.
        text: String,
        /// Its MIME type, if known.
        mime_type: Option<String>,
    },
}

impl From<&str> for PromptContent {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for PromptContent {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

/// Content the agent sends: message chunks and tool output.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// Text, usually markdown.
    Text(String),
    /// An image.
    Image {
        /// For example `image/png`.
        mime_type: String,
        /// The image bytes, base64-encoded.
        data: String,
        /// Where the image came from, if known.
        uri: Option<String>,
    },
    /// A link to a resource.
    ResourceLink {
        /// The resource's URI.
        uri: String,
        /// Display name.
        name: String,
    },
    /// An embedded resource.
    Resource {
        /// The resource's URI.
        uri: String,
        /// Its text, if it's a text resource.
        text: Option<String>,
        /// Its MIME type, if known.
        mime_type: Option<String>,
    },
    /// Any other kind of content (audio, or a kind this crate doesn't know).
    Other(Value),
}

impl Content {
    /// The text, if this is text.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }
}

/// Something that happened in a session, from a `session/update`
/// notification.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// A chunk of the user's message. Agents send these when replaying a
    /// loaded session's history.
    UserMessageChunk {
        /// The chunk.
        content: Content,
    },
    /// A chunk of the agent's reply.
    AgentMessageChunk {
        /// The chunk.
        content: Content,
        /// Groups chunks into messages, if the agent says.
        message_id: Option<String>,
    },
    /// A chunk of the agent's reasoning.
    AgentThoughtChunk {
        /// The chunk.
        content: Content,
        /// Groups chunks into messages, if the agent says.
        message_id: Option<String>,
    },
    /// The agent started a tool call.
    ToolCall(ToolCall),
    /// A tool call changed. Only the fields that changed are set.
    ToolCallUpdate(ToolCallUpdate),
    /// The agent's plan, complete each time.
    Plan(Vec<PlanEntry>),
    /// The slash commands the agent offers.
    AvailableCommands(Vec<Command>),
    /// The session's mode changed.
    ModeChanged {
        /// The new mode's id.
        mode_id: String,
    },
    /// The session's config options changed, for example the model. Complete
    /// each time.
    ConfigOptionsChanged(Vec<ConfigOption>),
    /// The session's title or last-updated time changed.
    SessionInfoChanged {
        /// The new title, if it changed.
        title: Option<String>,
        /// The new last-updated time (an ISO 8601 string), if it changed.
        updated_at: Option<String>,
    },
    /// How much of the context window is used, and what the session has cost.
    ContextUsage {
        /// Tokens in the context window.
        used: u64,
        /// The context window's size in tokens.
        size: u64,
        /// What the session has cost so far, if the agent knows.
        cost: Option<Cost>,
    },
    /// A notice for the user, such as a warning.
    Notice {
        /// `info`, `warning` or `error`.
        severity: String,
        /// One line.
        title: String,
        /// More detail.
        description: Option<String>,
    },
    /// The agent hit a usage limit, so the turn is ending. Detected by this
    /// crate from what the agent sent (see [`crate::limit`]); not an ACP
    /// update kind of its own.
    LimitReached(LimitInfo),
    /// A rate-limit status the agent reported on the side (Claude Code, in
    /// some `usage_update`s). Follows the [`SessionEvent::ContextUsage`] it
    /// came with.
    RateLimit(RateLimitStatus),
    /// The prompt turn ended. Sent after every update of the turn; the same
    /// result is also returned by [`crate::AcpClient::prompt`].
    TurnEnded(TurnEnd),
    /// An update this crate doesn't model, or one that didn't parse.
    Other {
        /// The `sessionUpdate` kind, if there was one.
        kind: String,
        /// The whole `update` object.
        raw: Value,
    },
}

/// How a prompt turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEnd {
    /// The agent answered with a stop reason.
    Stopped {
        /// Why the agent stopped.
        reason: StopReason,
        /// Tokens used in the turn, if the agent reports them.
        usage: Option<TokenUsage>,
    },
    /// The turn failed because of a usage limit.
    Limit(LimitInfo),
    /// The turn failed with an error.
    Error {
        /// The JSON-RPC error code, if the agent sent one.
        code: Option<i32>,
        /// What went wrong.
        message: String,
    },
}

/// Why a prompt turn stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The agent finished.
    EndTurn,
    /// The token limit was reached.
    MaxTokens,
    /// The agent made too many model requests in one turn.
    MaxTurnRequests,
    /// The model refused.
    Refusal,
    /// The client cancelled the turn.
    Cancelled,
    /// A stop reason this crate doesn't know.
    Other(String),
}

/// Tokens used in one turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Reasoning tokens, if reported.
    pub thought_tokens: Option<u64>,
    /// Tokens read from the prompt cache, if reported.
    pub cached_read_tokens: Option<u64>,
    /// Tokens written to the prompt cache, if reported.
    pub cached_write_tokens: Option<u64>,
}

/// A cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Cost {
    /// The amount.
    pub amount: f64,
    /// An ISO 4217 currency code, for example `USD`.
    pub currency: String,
}

/// A tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// The tool call's id, unique within the session.
    pub id: String,
    /// What the tool call does, for display.
    pub title: String,
    /// What kind of tool it is.
    pub kind: ToolKind,
    /// Where it is in its lifecycle.
    pub status: ToolCallStatus,
    /// Output so far: text, diffs or terminals.
    pub content: Vec<ToolCallContent>,
    /// Files the tool call touches.
    pub locations: Vec<Location>,
    /// The tool's raw input.
    pub raw_input: Option<Value>,
    /// The tool's raw output.
    pub raw_output: Option<Value>,
}

/// A change to a tool call. Fields that didn't change are `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCallUpdate {
    /// The tool call's id.
    pub id: String,
    /// The new title.
    pub title: Option<String>,
    /// The new kind.
    pub kind: Option<ToolKind>,
    /// The new status.
    pub status: Option<ToolCallStatus>,
    /// The complete new content.
    pub content: Option<Vec<ToolCallContent>>,
    /// The complete new locations.
    pub locations: Option<Vec<Location>>,
    /// The new raw input.
    pub raw_input: Option<Value>,
    /// The new raw output.
    pub raw_output: Option<Value>,
}

/// What kind of tool a tool call uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Reads files or data.
    Read,
    /// Edits files.
    Edit,
    /// Deletes files.
    Delete,
    /// Moves or renames files.
    Move,
    /// Searches.
    Search,
    /// Runs a command.
    Execute,
    /// Internal reasoning.
    Think,
    /// Fetches from the network.
    Fetch,
    /// Switches the session mode.
    SwitchMode,
    /// Anything else.
    Other,
}

/// Where a tool call is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    /// Not started; possibly waiting for permission.
    Pending,
    /// Running.
    InProgress,
    /// Finished.
    Completed,
    /// Failed.
    Failed,
}

/// Output of a tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCallContent {
    /// Ordinary content.
    Content(Content),
    /// A file change.
    Diff {
        /// The file.
        path: PathBuf,
        /// The text before; `None` for a new file.
        old_text: Option<String>,
        /// The text after.
        new_text: String,
    },
    /// A terminal the client created for the agent. Burrow offers no
    /// terminals in M2 (D-021), so agents shouldn't send this.
    Terminal {
        /// The terminal's id.
        terminal_id: String,
    },
    /// A kind this crate doesn't know.
    Other(Value),
}

/// A file a tool call touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// The file.
    pub path: PathBuf,
    /// A line in it, if known.
    pub line: Option<u32>,
}

/// One entry in the agent's plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    /// What the step is.
    pub content: String,
    /// `high`, `medium` or `low`.
    pub priority: String,
    /// `pending`, `in_progress` or `completed`.
    pub status: String,
}

/// A slash command the agent offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The command's name, without the slash.
    pub name: String,
    /// What it does.
    pub description: String,
    /// A hint for its input, if it takes any.
    pub input_hint: Option<String>,
}

/// One answer the agent offers to a permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    /// The id to answer with.
    pub id: String,
    /// Display name, for example "Allow once".
    pub name: String,
    /// What the answer means.
    pub kind: PermissionKind,
}

/// What a [`PermissionOption`] means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionKind {
    /// Allow this once.
    AllowOnce,
    /// Allow this and remember it.
    AllowAlways,
    /// Reject this once.
    RejectOnce,
    /// Reject this and remember it.
    RejectAlways,
    /// A kind this crate doesn't know.
    Other(String),
}

// ---------------------------------------------------------------------------
// Conversions from the ACP schema. Each one is total: what doesn't fit is kept
// as raw JSON.

/// Converts the `update` object of a `session/update` notification. Never
/// fails: an update that doesn't parse becomes [`SessionEvent::Other`].
pub(crate) fn session_event_from_json(update: Value) -> SessionEvent {
    let kind = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    match serde_json::from_value::<acp::SessionUpdate>(update.clone()) {
        Ok(parsed) => session_event_from_acp(parsed, kind, update),
        Err(_) => SessionEvent::Other { kind, raw: update },
    }
}

fn session_event_from_acp(update: acp::SessionUpdate, kind: String, raw: Value) -> SessionEvent {
    use acp::SessionUpdate as U;
    match update {
        U::UserMessageChunk(chunk) => SessionEvent::UserMessageChunk {
            content: content_from_acp(chunk.content),
        },
        U::AgentMessageChunk(chunk) => SessionEvent::AgentMessageChunk {
            content: content_from_acp(chunk.content),
            message_id: chunk.message_id.map(|id| id.0.to_string()),
        },
        U::AgentThoughtChunk(chunk) => SessionEvent::AgentThoughtChunk {
            content: content_from_acp(chunk.content),
            message_id: chunk.message_id.map(|id| id.0.to_string()),
        },
        U::ToolCall(call) => SessionEvent::ToolCall(tool_call_from_acp(call)),
        U::ToolCallUpdate(update) => {
            SessionEvent::ToolCallUpdate(tool_call_update_from_acp(update))
        }
        U::Plan(plan) => {
            SessionEvent::Plan(plan.entries.into_iter().map(plan_entry_from_acp).collect())
        }
        U::AvailableCommandsUpdate(update) => SessionEvent::AvailableCommands(
            update
                .available_commands
                .into_iter()
                .map(|command| Command {
                    name: command.name,
                    description: command.description,
                    input_hint: command.input.and_then(|input| match input {
                        acp::AvailableCommandInput::Unstructured(input) => Some(input.hint),
                        _ => None,
                    }),
                })
                .collect(),
        ),
        U::CurrentModeUpdate(update) => SessionEvent::ModeChanged {
            mode_id: update.current_mode_id.0.to_string(),
        },
        U::ConfigOptionUpdate(update) => SessionEvent::ConfigOptionsChanged(
            update
                .config_options
                .into_iter()
                .map(config_option_from_acp)
                .collect(),
        ),
        U::SessionInfoUpdate(update) => SessionEvent::SessionInfoChanged {
            title: maybe_undefined(update.title),
            updated_at: maybe_undefined(update.updated_at),
        },
        U::UsageUpdate(update) => SessionEvent::ContextUsage {
            used: update.used,
            size: update.size,
            cost: update.cost.map(|cost| Cost {
                amount: cost.amount,
                currency: cost.currency,
            }),
        },
        U::Notice(notice) => SessionEvent::Notice {
            severity: enum_str(&notice.severity),
            title: notice.title,
            description: notice.description,
        },
        _ => SessionEvent::Other { kind, raw },
    }
}

fn maybe_undefined<T>(value: MaybeUndefined<T>) -> Option<T> {
    match value {
        MaybeUndefined::Value(value) => Some(value),
        _ => None,
    }
}

/// The serde name of a unit enum value, such as `"in_progress"`.
fn enum_str<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

pub(crate) fn content_from_acp(block: acp::ContentBlock) -> Content {
    match block {
        acp::ContentBlock::Text(text) => Content::Text(text.text),
        acp::ContentBlock::Image(image) => Content::Image {
            mime_type: image.mime_type,
            data: image.data,
            uri: image.uri,
        },
        acp::ContentBlock::ResourceLink(link) => Content::ResourceLink {
            uri: link.uri,
            name: link.name,
        },
        acp::ContentBlock::Resource(resource) => match resource.resource {
            acp::EmbeddedResourceResource::TextResourceContents(text) => Content::Resource {
                uri: text.uri,
                text: Some(text.text),
                mime_type: text.mime_type,
            },
            acp::EmbeddedResourceResource::BlobResourceContents(blob) => Content::Resource {
                uri: blob.uri,
                text: None,
                mime_type: blob.mime_type,
            },
            other => Content::Other(serde_json::to_value(other).unwrap_or_default()),
        },
        other => Content::Other(serde_json::to_value(other).unwrap_or_default()),
    }
}

pub(crate) fn prompt_content_to_acp(content: PromptContent) -> acp::ContentBlock {
    match content {
        PromptContent::Text(text) => acp::ContentBlock::Text(acp::TextContent::new(text)),
        PromptContent::Image { mime_type, data } => {
            acp::ContentBlock::Image(acp::ImageContent::new(data, mime_type))
        }
        PromptContent::ResourceLink { uri, name } => {
            acp::ContentBlock::ResourceLink(acp::ResourceLink::new(name, uri))
        }
        PromptContent::Resource {
            uri,
            text,
            mime_type,
        } => acp::ContentBlock::Resource(acp::EmbeddedResource::new(
            acp::EmbeddedResourceResource::TextResourceContents(
                acp::TextResourceContents::new(text, uri).mime_type(mime_type),
            ),
        )),
    }
}

pub(crate) fn tool_kind_from_acp(kind: acp::ToolKind) -> ToolKind {
    match kind {
        acp::ToolKind::Read => ToolKind::Read,
        acp::ToolKind::Edit => ToolKind::Edit,
        acp::ToolKind::Delete => ToolKind::Delete,
        acp::ToolKind::Move => ToolKind::Move,
        acp::ToolKind::Search => ToolKind::Search,
        acp::ToolKind::Execute => ToolKind::Execute,
        acp::ToolKind::Think => ToolKind::Think,
        acp::ToolKind::Fetch => ToolKind::Fetch,
        acp::ToolKind::SwitchMode => ToolKind::SwitchMode,
        _ => ToolKind::Other,
    }
}

pub(crate) fn tool_status_from_acp(status: acp::ToolCallStatus) -> ToolCallStatus {
    match status {
        acp::ToolCallStatus::Pending => ToolCallStatus::Pending,
        acp::ToolCallStatus::InProgress => ToolCallStatus::InProgress,
        acp::ToolCallStatus::Completed => ToolCallStatus::Completed,
        acp::ToolCallStatus::Failed => ToolCallStatus::Failed,
        _ => ToolCallStatus::InProgress,
    }
}

fn tool_content_from_acp(content: acp::ToolCallContent) -> ToolCallContent {
    match content {
        acp::ToolCallContent::Content(content) => {
            ToolCallContent::Content(content_from_acp(content.content))
        }
        acp::ToolCallContent::Diff(diff) => ToolCallContent::Diff {
            path: diff.path,
            old_text: diff.old_text,
            new_text: diff.new_text,
        },
        acp::ToolCallContent::Terminal(terminal) => ToolCallContent::Terminal {
            terminal_id: terminal.terminal_id.0.to_string(),
        },
        other => ToolCallContent::Other(serde_json::to_value(other).unwrap_or_default()),
    }
}

fn location_from_acp(location: acp::ToolCallLocation) -> Location {
    Location {
        path: location.path,
        line: location.line,
    }
}

fn tool_call_from_acp(call: acp::ToolCall) -> ToolCall {
    ToolCall {
        id: call.tool_call_id.0.to_string(),
        title: call.title,
        kind: tool_kind_from_acp(call.kind),
        status: tool_status_from_acp(call.status),
        content: call
            .content
            .into_iter()
            .map(tool_content_from_acp)
            .collect(),
        locations: call.locations.into_iter().map(location_from_acp).collect(),
        raw_input: call.raw_input,
        raw_output: call.raw_output,
    }
}

pub(crate) fn tool_call_update_from_acp(update: acp::ToolCallUpdate) -> ToolCallUpdate {
    let fields = update.fields;
    ToolCallUpdate {
        id: update.tool_call_id.0.to_string(),
        title: fields.title,
        kind: fields.kind.map(tool_kind_from_acp),
        status: fields.status.map(tool_status_from_acp),
        content: fields
            .content
            .map(|content| content.into_iter().map(tool_content_from_acp).collect()),
        locations: fields
            .locations
            .map(|locations| locations.into_iter().map(location_from_acp).collect()),
        raw_input: fields.raw_input,
        raw_output: fields.raw_output,
    }
}

fn plan_entry_from_acp(entry: acp::PlanEntry) -> PlanEntry {
    PlanEntry {
        content: entry.content,
        priority: enum_str(&entry.priority),
        status: enum_str(&entry.status),
    }
}

pub(crate) fn permission_option_from_acp(option: acp::PermissionOption) -> PermissionOption {
    PermissionOption {
        id: option.option_id.0.to_string(),
        name: option.name,
        kind: match option.kind {
            acp::PermissionOptionKind::AllowOnce => PermissionKind::AllowOnce,
            acp::PermissionOptionKind::AllowAlways => PermissionKind::AllowAlways,
            acp::PermissionOptionKind::RejectOnce => PermissionKind::RejectOnce,
            acp::PermissionOptionKind::RejectAlways => PermissionKind::RejectAlways,
            other => PermissionKind::Other(enum_str(&other)),
        },
    }
}

pub(crate) fn config_option_from_acp(option: acp::SessionConfigOption) -> ConfigOption {
    ConfigOption {
        id: option.id.0.to_string(),
        name: option.name,
        description: option.description,
        category: match option.category {
            Some(acp::SessionConfigOptionCategory::Mode) => ConfigCategory::Mode,
            Some(acp::SessionConfigOptionCategory::Model) => ConfigCategory::Model,
            Some(acp::SessionConfigOptionCategory::ModelConfig) => ConfigCategory::ModelConfig,
            Some(acp::SessionConfigOptionCategory::ThoughtLevel) => ConfigCategory::ThoughtLevel,
            Some(other) => ConfigCategory::Other(enum_str(&other)),
            None => ConfigCategory::Other(String::new()),
        },
        value: match option.kind {
            acp::SessionConfigKind::Select(select) => ConfigValue::Select {
                current: select.current_value.0.to_string(),
                options: match select.options {
                    acp::SessionConfigSelectOptions::Ungrouped(options) => {
                        options.into_iter().map(choice_from_select).collect()
                    }
                    acp::SessionConfigSelectOptions::Grouped(groups) => groups
                        .into_iter()
                        .flat_map(|group| group.options)
                        .map(choice_from_select)
                        .collect(),
                    other => {
                        tracing::debug!(?other, "unknown config select options");
                        Vec::new()
                    }
                },
            },
            acp::SessionConfigKind::Boolean(boolean) => ConfigValue::Boolean(boolean.current_value),
            other => ConfigValue::Other(serde_json::to_value(other).unwrap_or_default()),
        },
    }
}

fn choice_from_select(option: acp::SessionConfigSelectOption) -> Choice {
    Choice {
        id: option.value.0.to_string(),
        name: option.name,
        description: option.description,
    }
}

pub(crate) fn stop_reason_from_json(value: &Value) -> StopReason {
    match value.as_str() {
        Some("end_turn") => StopReason::EndTurn,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("max_turn_requests") => StopReason::MaxTurnRequests,
        Some("refusal") => StopReason::Refusal,
        Some("cancelled") => StopReason::Cancelled,
        Some(other) => StopReason::Other(other.to_owned()),
        None => StopReason::Other(value.to_string()),
    }
}

/// Reads `usage` from a `session/prompt` result. Lenient: the field is
/// still unstable in ACP, so each number is read on its own.
pub(crate) fn token_usage_from_json(value: &Value) -> Option<TokenUsage> {
    let usage = value.as_object()?;
    let number = |key: &str| usage.get(key).and_then(Value::as_u64);
    Some(TokenUsage {
        input_tokens: number("inputTokens")?,
        output_tokens: number("outputTokens")?,
        thought_tokens: number("thoughtTokens"),
        cached_read_tokens: number("cachedReadTokens"),
        cached_write_tokens: number("cachedWriteTokens"),
    })
}

/// Builds [`AgentInfo`] from the raw `initialize` result.
pub(crate) fn agent_info_from_json(raw: Value) -> AgentInfo {
    let info = raw.get("agentInfo");
    let text = |value: Option<&Value>, key: &str| {
        value
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let caps = raw.get("agentCapabilities");
    let flag = |path: &[&str]| {
        let mut value = caps;
        for key in path {
            value = value.and_then(|value| value.get(*key));
        }
        match value {
            Some(Value::Bool(flag)) => *flag,
            // Session capabilities are objects: present means supported.
            Some(Value::Object(_)) => true,
            _ => false,
        }
    };
    let capabilities = AgentCapabilities {
        load_session: flag(&["loadSession"]),
        resume_session: flag(&["sessionCapabilities", "resume"]),
        close_session: flag(&["sessionCapabilities", "close"]),
        list_sessions: flag(&["sessionCapabilities", "list"]),
        prompt_image: flag(&["promptCapabilities", "image"]),
        prompt_audio: flag(&["promptCapabilities", "audio"]),
        prompt_embedded_context: flag(&["promptCapabilities", "embeddedContext"]),
        mcp_http: flag(&["mcpCapabilities", "http"]),
        mcp_sse: flag(&["mcpCapabilities", "sse"]),
    };
    let auth_methods = raw
        .get("authMethods")
        .and_then(Value::as_array)
        .map(|methods| methods.iter().filter_map(auth_method_from_json).collect())
        .unwrap_or_default();
    AgentInfo {
        name: text(info, "name"),
        title: text(info, "title"),
        version: text(info, "version"),
        protocol_version: raw
            .get("protocolVersion")
            .and_then(Value::as_u64)
            .and_then(|version| u16::try_from(version).ok())
            .unwrap_or(0),
        capabilities,
        auth_methods,
        raw,
    }
}

fn auth_method_from_json(value: &Value) -> Option<AuthMethod> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(AuthMethod {
        id: text("id")?,
        name: text("name").unwrap_or_default(),
        description: text("description"),
        terminal: value.get("type").and_then(Value::as_str) == Some("terminal"),
        args: value
            .get("args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Builds [`SessionInfo`] from the raw result of `session/new`,
/// `session/load` or `session/resume`. `session_id` is the id the caller
/// asked for, used when the result doesn't repeat it.
pub(crate) fn session_info_from_json(session_id: Option<&str>, raw: Value) -> SessionInfo {
    let session_id = raw
        .get("sessionId")
        .and_then(Value::as_str)
        .or(session_id)
        .unwrap_or_default()
        .to_owned();
    let config_options = config_options_from_json(raw.get("configOptions"));
    let modes = raw.get("modes").and_then(|modes| {
        let modes: acp::SessionModeState = serde_json::from_value(modes.clone()).ok()?;
        Some(ModeState {
            current: modes.current_mode_id.0.to_string(),
            available: modes
                .available_modes
                .into_iter()
                .map(|mode| Choice {
                    id: mode.id.0.to_string(),
                    name: mode.name,
                    description: mode.description,
                })
                .collect(),
        })
    });
    let models = model_state(&config_options).or_else(|| legacy_models(raw.get("models")));
    SessionInfo {
        session_id,
        modes,
        models,
        config_options,
        raw,
    }
}

/// Parses a `configOptions` array one option at a time, so one option this
/// crate can't read doesn't hide the others.
pub(crate) fn config_options_from_json(value: Option<&Value>) -> Vec<ConfigOption> {
    value
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    serde_json::from_value::<acp::SessionConfigOption>(option.clone()).ok()
                })
                .map(config_option_from_acp)
                .collect()
        })
        .unwrap_or_default()
}

/// The model selector among config options, if there is one.
pub(crate) fn model_state(options: &[ConfigOption]) -> Option<ModelState> {
    options
        .iter()
        .find_map(|option| match (&option.category, &option.value) {
            (ConfigCategory::Model, ConfigValue::Select { current, options }) => Some(ModelState {
                current: current.clone(),
                available: options.clone(),
                config_id: Some(option.id.clone()),
            }),
            _ => None,
        })
}

/// The older, unstable `models` field: `{currentModelId, availableModels:
/// [{modelId, name, description?}]}`, changed with `session/set_model`.
fn legacy_models(value: Option<&Value>) -> Option<ModelState> {
    let value = value?;
    let current = value.get("currentModelId")?.as_str()?.to_owned();
    let available = value
        .get("availableModels")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    Some(Choice {
                        id: model.get("modelId")?.as_str()?.to_owned(),
                        name: model
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        description: model
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ModelState {
        current,
        available,
        config_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_update_kinds_are_kept_raw() {
        let raw = json!({"sessionUpdate": "brand_new_thing", "x": 1});
        assert_eq!(
            session_event_from_json(raw.clone()),
            SessionEvent::Other {
                kind: "brand_new_thing".into(),
                raw
            }
        );
    }

    #[test]
    fn a_known_update_that_does_not_parse_is_kept_raw() {
        let raw = json!({"sessionUpdate": "tool_call", "title": 5});
        assert!(matches!(
            session_event_from_json(raw),
            SessionEvent::Other { kind, .. } if kind == "tool_call"
        ));
    }

    #[test]
    fn tool_calls_convert() {
        let event = session_event_from_json(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "title": "Write hello.txt",
            "kind": "edit",
            "status": "pending",
            "locations": [{"path": "C:/tmp/hello.txt"}],
            "content": [{"type": "diff", "path": "C:/tmp/hello.txt", "newText": "hi"}]
        }));
        let SessionEvent::ToolCall(call) = event else {
            panic!("not a tool call: {event:?}");
        };
        assert_eq!(call.id, "t1");
        assert_eq!(call.kind, ToolKind::Edit);
        assert_eq!(call.status, ToolCallStatus::Pending);
        assert_eq!(call.locations[0].path, PathBuf::from("C:/tmp/hello.txt"));
        assert!(matches!(
            &call.content[0],
            ToolCallContent::Diff { old_text: None, new_text, .. } if new_text == "hi"
        ));
    }

    #[test]
    fn models_come_from_the_model_config_option_first() {
        let info = session_info_from_json(
            None,
            json!({
                "sessionId": "s1",
                "configOptions": [{
                    "id": "model", "name": "Model", "category": "model", "type": "select",
                    "currentValue": "fast",
                    "options": [{"value": "fast", "name": "Fast"}, {"value": "smart", "name": "Smart"}]
                }],
                "models": {"currentModelId": "legacy", "availableModels": []}
            }),
        );
        let models = info.models.expect("models");
        assert_eq!(models.current, "fast");
        assert_eq!(models.config_id.as_deref(), Some("model"));
        assert_eq!(models.available.len(), 2);
    }

    #[test]
    fn legacy_models_are_read_when_there_is_no_config_option() {
        let info = session_info_from_json(
            Some("s1"),
            json!({
                "models": {
                    "currentModelId": "gpt-5",
                    "availableModels": [{"modelId": "gpt-5", "name": "GPT-5"}]
                }
            }),
        );
        assert_eq!(info.session_id, "s1");
        let models = info.models.expect("models");
        assert_eq!(models.current, "gpt-5");
        assert_eq!(models.config_id, None);
    }

    #[test]
    fn agent_info_reads_capabilities_and_auth_methods() {
        let info = agent_info_from_json(json!({
            "protocolVersion": 1,
            "agentInfo": {"name": "fake", "version": "1.2.3"},
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {"image": true},
                "sessionCapabilities": {"resume": {}, "close": {}}
            },
            "authMethods": [
                {"id": "login", "name": "Log in"},
                {"id": "tui", "name": "Terminal", "type": "terminal", "args": ["login"]}
            ]
        }));
        assert_eq!(info.version.as_deref(), Some("1.2.3"));
        assert!(info.capabilities.load_session);
        assert!(info.capabilities.resume_session);
        assert!(info.capabilities.close_session);
        assert!(!info.capabilities.list_sessions);
        assert!(info.capabilities.prompt_image);
        assert_eq!(info.auth_methods.len(), 2);
        assert!(info.auth_methods[1].terminal);
        assert_eq!(info.auth_methods[1].args, ["login"]);
    }
}
