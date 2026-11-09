//! Turns: submitting input, stopping a turn, and how a turn ends.
//!
//! A turn ends with exactly one `turn/completed`. Its `terminal` is `completed`, `cancelled` or
//! `failed`. A failure carries a [`TurnError`]; mid-turn failures never arrive as JSON-RPC
//! errors. [`TurnError::class`] sorts failures into usage limits, an unavailable provider, and
//! everything else.

use serde::{Deserialize, Serialize};

use super::{CommandStatus, ReasoningEffort, new_command_id};

/// The kind of a [`TurnInputPart`]. Closed: Muse rejects unknown part types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum TurnInputPartType {
    /// Prompt text. Mention files as `@relative/path` inside the text.
    Text,
    /// An image: `mediaType` and `base64Data`.
    Image,
    /// A skill invocation: `selector`, optionally `arguments`.
    Skill,
    /// An uploaded video: `uploadId`.
    Video,
}

/// One part of a turn's input. A flat object whose `type` says which members apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TurnInputPart {
    /// Free-text skill arguments, on a `skill` part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    /// The image bytes as base64, on an `image` part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base64_data: Option<String>,
    /// Pixel height; only together with `width`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// The image's media type, on an `image` part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The skill to invoke, on a `skill` part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// The prompt text, on a `text` part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Which kind of part this is.
    #[serde(rename = "type")]
    pub kind: TurnInputPartType,
    /// Pixel width; only together with `height`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
}

impl TurnInputPart {
    /// A text part.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::empty(TurnInputPartType::Text)
        }
    }

    /// An image part.
    pub fn image(media_type: impl Into<String>, base64_data: impl Into<String>) -> Self {
        Self {
            media_type: Some(media_type.into()),
            base64_data: Some(base64_data.into()),
            ..Self::empty(TurnInputPartType::Image)
        }
    }

    fn empty(kind: TurnInputPartType) -> Self {
        Self {
            arguments: None,
            base64_data: None,
            height: None,
            media_type: None,
            selector: None,
            text: None,
            kind,
            width: None,
        }
    }
}

/// What `turn/start` does when a turn is already running. Muse's default is `queue`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum IfBusy {
    /// Run after the current turn.
    Queue,
    /// Inject into the current turn.
    Steer,
    /// Replace the current turn.
    Replace,
}

/// `turn/start` params: submit input to a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    /// Idempotency handle (UUIDv7). A fresh turn's id is this id.
    pub command_id: String,
    /// How transcripts show the prompt; never seen by the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_text: Option<String>,
    /// What to do if a turn is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_busy: Option<IfBusy>,
    /// The input, in order; at least one part.
    pub input: Vec<TurnInputPart>,
    /// Reasoning effort for this turn only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// The session.
    pub session_id: String,
}

impl TurnStartParams {
    /// A text prompt for `session_id`, with a fresh `commandId`.
    pub fn text(session_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            command_id: new_command_id(),
            display_text: None,
            if_busy: None,
            input: vec![TurnInputPart::text(text)],
            reasoning_effort: None,
            session_id: session_id.into(),
        }
    }
}

open_enum! {
    /// What `turn/start` did with the input.
    pub enum TurnStartDisposition {
        /// A new turn started.
        Started = "started",
        /// Queued behind the running turn.
        Queued = "queued",
        /// Injected into the running turn.
        Steered = "steered",
    }
}

/// `turn/start` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TurnStartResult {
    /// Echo of the `commandId`.
    pub command_id: String,
    /// What happened to the input.
    pub disposition: TurnStartDisposition,
    /// Whether `disposition` is `started`.
    pub started_new_turn: bool,
    /// Admission status.
    pub status: CommandStatus,
    /// The turn that carries the input. Always take it from here.
    pub turn_id: String,
}

/// `turn/interrupt` params: the "stop" button, on Muse's priority lane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    /// Idempotency handle (UUIDv7).
    pub command_id: String,
    /// Also retract the submission if no assistant output was committed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retract: Option<bool>,
    /// The session.
    pub session_id: String,
    /// The turn; the current foreground turn when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

impl TurnInterruptParams {
    /// Interrupts `turn_id` (or the current turn) in `session_id`, with a fresh `commandId`.
    pub fn new(session_id: impl Into<String>, turn_id: Option<String>) -> Self {
        Self {
            command_id: new_command_id(),
            retract: None,
            session_id: session_id.into(),
            turn_id,
        }
    }
}

/// `turn/cancel` params: a plain cancel on the normal command lane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TurnCancelParams {
    /// Idempotency handle (UUIDv7).
    pub command_id: String,
    /// The session.
    pub session_id: String,
    /// The turn; the current foreground turn when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

impl TurnCancelParams {
    /// Cancels `turn_id` (or the current turn) in `session_id`, with a fresh `commandId`.
    pub fn new(session_id: impl Into<String>, turn_id: Option<String>) -> Self {
        Self {
            command_id: new_command_id(),
            session_id: session_id.into(),
            turn_id,
        }
    }
}

/// `turn/interrupt` and `turn/cancel` result. Acceptance means admitted, not stopped: the turn
/// is over at its `turn/completed`. With no running turn Muse refuses the call instead:
/// `commandRejected`, reason `missing_run`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptResult {
    /// Echo of the `commandId`.
    pub command_id: String,
    /// Admission status.
    pub status: CommandStatus,
    /// The turn being stopped.
    pub turn_id: String,
}

/// `turn/cancel` result: the same shape as [`TurnInterruptResult`].
pub type TurnCancelResult = TurnInterruptResult;

/// `turn/started`: a foreground turn began running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedParams {
    /// The submitting command; equal to `turnId` for fresh turns.
    pub command_id: String,
    /// The session.
    pub session_id: String,
    /// The turn.
    pub turn_id: String,
    /// The view position.
    pub view_cursor: String,
}

/// `turn/foregroundCompleted`: the model is done, but reminder agents still hold the turn open.
/// Not the end of the turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TurnForegroundCompletedParams {
    /// The reminder agents still running, such as `goal-reminder`.
    pub blocking_agents: Vec<String>,
    /// The session.
    pub session_id: String,
    /// The turn.
    pub turn_id: String,
    /// The view position.
    pub view_cursor: String,
}

open_enum! {
    /// How a turn ended.
    pub enum TurnTerminal {
        /// It finished.
        Completed = "completed",
        /// It failed; see `error`.
        Failed = "failed",
        /// It was interrupted or cancelled.
        Cancelled = "cancelled",
    }
}

open_enum! {
    /// The class of a turn failure.
    pub enum TurnErrorKind {
        /// The step limit was reached.
        StepLimit = "stepLimit",
        /// Bad configuration.
        ConfigError = "configError",
        /// The view projection failed.
        ProjectionError = "projectionError",
        /// The session log failed.
        LogError = "logError",
        /// A workflow failed to launch.
        WorkflowLaunchError = "workflowLaunchError",
        /// The environment is broken.
        EnvironmentError = "environmentError",
        /// The model call failed. Quota exhaustion is this kind, with `quota` set.
        ModelError = "modelError",
        /// The run failed to launch.
        LaunchError = "launchError",
        /// The provider needs a login.
        AuthRequired = "authRequired",
        /// An MCP server link failed.
        McpLinkError = "mcpLinkError",
    }
}

open_enum! {
    /// Which quota ran out.
    pub enum LimitKind {
        /// The subscription tier's limit.
        TierLimit = "tierLimit",
    }
}

/// Typed quota exhaustion: present exactly when a turn stopped on a quota.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct QuotaDetail {
    /// Which quota.
    pub limit_kind: LimitKind,
    /// When the provider says it resets, in epoch milliseconds.
    pub resets_at_ms: u64,
}

/// Why a turn failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TurnError {
    /// The failure class.
    pub kind: TurnErrorKind,
    /// Human-readable text. Display only; never parse it.
    pub message: String,
    /// Set when the turn stopped on a quota.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaDetail>,
    /// `true` when the provider refused the request. Only on a live `turn/completed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_denied: Option<bool>,
    /// Whether resubmitting the same input may succeed.
    pub retryable: bool,
}

/// What a failed turn means for whoever drives the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailureClass {
    /// A usage limit. Don't send more turns to this provider before `resets_at_ms`.
    Limit {
        /// Which quota.
        limit_kind: LimitKind,
        /// When it resets, in epoch milliseconds.
        resets_at_ms: u64,
    },
    /// The provider can't serve the session: it needs a login (`authRequired`) or refused the
    /// request (`requestDenied`). Another agent may still work.
    Unavailable,
    /// Anything else.
    Other,
}

impl TurnError {
    /// Classifies the failure. Branches on typed members only, never on `message`.
    pub fn class(&self) -> FailureClass {
        if let Some(quota) = &self.quota {
            return FailureClass::Limit {
                limit_kind: quota.limit_kind.clone(),
                resets_at_ms: quota.resets_at_ms,
            };
        }
        if self.kind == TurnErrorKind::AuthRequired || self.request_denied == Some(true) {
            return FailureClass::Unavailable;
        }
        FailureClass::Other
    }
}

/// Raw token counters for one model call or one turn.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    /// Cache-read tokens, when the provider tells reads and writes apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Cache-write tokens, when the provider tells reads and writes apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// Cached tokens as the provider reports them.
    pub cached_tokens: u64,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Output tokens spent on reasoning.
    pub reasoning_tokens: u64,
}

/// `turn/completed`: the turn's one terminal event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedParams {
    /// How long the turn took, when measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Set when `terminal` is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TurnError>,
    /// Muse's free-text reason, such as "cancelled during end-of-turn reminder wait". Display
    /// only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The session.
    pub session_id: String,
    /// How the turn ended.
    pub terminal: TurnTerminal,
    /// Time to the first token, when measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_ms: Option<u64>,
    /// The turn.
    pub turn_id: String,
    /// The turn's total token usage, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// The view position.
    pub view_cursor: String,
}

impl TurnCompletedParams {
    /// The failure class, when the turn failed. A `failed` terminal without an error object
    /// (a synthesized host-death terminal) is [`FailureClass::Other`].
    pub fn failure(&self) -> Option<FailureClass> {
        match (&self.terminal, &self.error) {
            (_, Some(error)) => Some(error.class()),
            (TurnTerminal::Failed, None) => Some(FailureClass::Other),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn error(value: serde_json::Value) -> TurnError {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_quota_is_a_limit_whatever_the_kind() {
        let quota = error(json!({
            "kind": "modelError", "message": "quota", "retryable": false,
            "quota": {"limitKind": "tierLimit", "resetsAtMs": 1794210000000u64}
        }));
        assert_eq!(
            quota.class(),
            FailureClass::Limit {
                limit_kind: LimitKind::TierLimit,
                resets_at_ms: 1_794_210_000_000,
            }
        );
    }

    #[test]
    fn auth_and_denials_are_unavailable_and_the_rest_is_other() {
        let auth = error(json!({"kind": "authRequired", "message": "log in", "retryable": false}));
        assert_eq!(auth.class(), FailureClass::Unavailable);
        let denied = error(json!({
            "kind": "modelError", "message": "no", "retryable": false, "requestDenied": true
        }));
        assert_eq!(denied.class(), FailureClass::Unavailable);
        let other = error(json!({"kind": "stepLimit", "message": "steps", "retryable": false}));
        assert_eq!(other.class(), FailureClass::Other);
    }

    #[test]
    fn text_prompts_encode_as_one_text_part() {
        let params = TurnStartParams::text("s", "hello");
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value["input"], json!([{"type": "text", "text": "hello"}]));
        assert!(value.get("ifBusy").is_none());
        assert!(uuid::Uuid::parse_str(&params.command_id).is_ok());
    }
}
