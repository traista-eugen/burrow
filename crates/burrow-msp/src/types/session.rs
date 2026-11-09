//! Sessions: starting one, its state, and the session notifications.

use serde::{Deserialize, Serialize};

use super::{CommandStatus, new_command_id};

open_enum! {
    /// How tool approvals are enforced in a session. Muse defines the modes; a client selects
    /// one and never creates one.
    pub enum ApprovalMode {
        /// Approve everything.
        AllowAll = "allowAll",
        /// Ask a person for anything the policy doesn't already allow: Muse's default.
        PromptUnmatched = "promptUnmatched",
        /// Ask only when the model requests it.
        OnRequest = "onRequest",
        /// Deny anything the policy doesn't already allow.
        DenyUnmatched = "denyUnmatched",
    }
}

open_enum! {
    /// A session's load state.
    pub enum SessionStatus {
        /// Not loaded on this host.
        NotLoaded = "notLoaded",
        /// Loaded, no turn running.
        Idle = "idle",
        /// A turn is running.
        Running = "running",
    }
}

open_enum! {
    /// What a loaded session is waiting on.
    pub enum AttentionFlag {
        /// An approval is waiting for a decision.
        ApprovalPending = "approvalPending",
        /// A user-input prompt is waiting for an answer.
        InputPending = "inputPending",
    }
}

open_enum! {
    /// Why a session was unloaded.
    pub enum SessionClosedReason {
        /// It sat idle.
        Idle = "idle",
        /// The host is shutting down, for example because its stdin closed.
        HostShutdown = "hostShutdown",
        /// A client called `session/leave`.
        Leave = "leave",
    }
}

/// `session/start` params: create a session, load it, and subscribe this connection to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionStartParams {
    /// The starting approval mode; Muse's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<ApprovalMode>,
    /// Idempotency handle (UUIDv7).
    pub command_id: String,
    /// The starting model, for example `muse-spark-1.3`; the provider's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// The starting provider routing; the host's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// The session id to use; Muse mints a UUIDv7 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The workspace, an absolute path. Tools work relative to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
}

impl SessionStartParams {
    /// A session in `workspace_root`, with a fresh `commandId` and every other member defaulted.
    pub fn new(workspace_root: impl Into<String>) -> Self {
        Self {
            approval_mode: None,
            command_id: new_command_id(),
            model_id: None,
            provider_id: None,
            session_id: None,
            workspace_root: Some(workspace_root.into()),
        }
    }
}

/// The approval mode in force, and how it got there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct EffectiveApprovalModeState {
    /// The `commandId` that last changed it; `None` when it was never changed.
    #[serde(default)]
    pub last_command_id: Option<String>,
    /// The mode.
    pub mode: ApprovalMode,
    /// How it took effect: `startup`, `replay` or `approvalReconfigure`.
    pub source: String,
}

/// A session, as every session method returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// The running turn; `None` when idle.
    #[serde(default)]
    pub active_turn_id: Option<String>,
    /// The approval mode in force.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<EffectiveApprovalModeState>,
    /// What the session is waiting on; empty when nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<AttentionFlag>,
    /// RFC 3339.
    pub created_at: String,
    /// The model; `None` when the provider has none (`echo`).
    #[serde(default)]
    pub model_id: Option<String>,
    /// The durable session name, when one was allocated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The session log's path; empty for memory-only sessions.
    pub path: String,
    /// The provider: `meta`, `echo` or `local`.
    #[serde(default)]
    pub provider_id: Option<String>,
    /// The session id.
    pub session_id: String,
    /// Load state.
    pub status: SessionStatus,
    /// A derived display title, when Muse has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Completed turns.
    pub turn_count: u64,
    /// RFC 3339.
    pub updated_at: String,
    /// The workspace.
    #[serde(default)]
    pub workspace_root: Option<String>,
}

/// `session/start` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionStartResult {
    /// The new session.
    pub session: Session,
    /// The session's view position after the start. This connection gets every view event
    /// after it.
    pub view_cursor: String,
}

/// `session/setApprovalMode` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionSetApprovalModeParams {
    /// Idempotency handle (UUIDv7).
    pub command_id: String,
    /// The mode to select.
    pub mode: ApprovalMode,
    /// The session.
    pub session_id: String,
}

impl SessionSetApprovalModeParams {
    /// Selects `mode` for `session_id`, with a fresh `commandId`.
    pub fn new(session_id: impl Into<String>, mode: ApprovalMode) -> Self {
        Self {
            command_id: new_command_id(),
            mode,
            session_id: session_id.into(),
        }
    }
}

/// `session/setApprovalMode` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionSetApprovalModeResult {
    /// Echo of the `commandId`.
    pub command_id: String,
    /// The mode in force afterwards.
    pub effective_mode: EffectiveApprovalModeState,
    /// Admission status.
    pub status: CommandStatus,
}

/// `session/started`: a session was loaded on this host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionStartedParams {
    /// The session.
    pub session: Session,
}

/// `session/statusChanged`: a session's status or attention flags changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusChangedParams {
    /// What the session waits on; empty when nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<AttentionFlag>,
    /// The session.
    pub session_id: String,
    /// The new status.
    pub status: SessionStatus,
    /// The view position, when the session has one.
    #[serde(default)]
    pub view_cursor: Option<String>,
}

/// `session/closed`: a session was unloaded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionClosedParams {
    /// Why.
    pub reason: SessionClosedReason,
    /// The session.
    pub session_id: String,
    /// The final view position.
    #[serde(default)]
    pub view_cursor: Option<String>,
}

/// `session/modelChanged`: a model selection took effect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionModelChangedParams {
    /// The model now in effect.
    pub model_id: String,
    /// Its provider.
    #[serde(default)]
    pub provider_id: Option<String>,
    /// The session.
    pub session_id: String,
    /// Who changed it: `user`, `default` or `policy`.
    pub source: String,
    /// The view position.
    pub view_cursor: String,
}

/// `view/gap`: Muse dropped view events between `after` and `next`; delivery goes on from
/// `next`. Whatever folds the session's view should re-read it (`view/page`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ViewGapParams {
    /// The last cursor delivered before the gap.
    pub after: String,
    /// The first cursor delivered after it.
    pub next: String,
    /// The session.
    pub session_id: String,
}
