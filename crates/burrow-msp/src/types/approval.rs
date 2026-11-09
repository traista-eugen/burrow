//! Approvals: Muse asks before a tool call the policy doesn't already allow.
//!
//! The round trip, as observed with Muse 1.4.14:
//! 1. Muse sends the request `approval/request` to every connection subscribed to the session,
//!    and the notification `approval/requested` on the view stream. Both carry
//!    [`ApprovalRequestParams`].
//! 2. The client answers `approval/request` with an empty presentation receipt `{}`. That
//!    decides nothing; [`crate::MspClient`] sends it for you.
//! 3. Someone decides: `approval/decide` with one of `availableChoices` and the current
//!    `requirementId`.
//! 4. Muse reports `approval/updated` for each stage satisfied and, once final,
//!    `approval/resolved`. A shell command with several stages sends a fresh `approval/request`
//!    for each stage still open.

use serde::{Deserialize, Serialize};

use super::{CommandStatus, UserInputRequestParams, new_command_id};

open_enum! {
    /// What a choice applies, or what an approval was decided as.
    pub enum ApprovalDecision {
        /// Allowed, this once.
        Approved = "approved",
        /// Allowed for the rest of the session.
        ApprovedForSession = "approvedForSession",
        /// Allowed, and a policy rule was saved.
        ApprovedPolicyAmendment = "approvedPolicyAmendment",
        /// Denied.
        Denied = "denied",
        /// Denied, and a policy rule was saved.
        DeniedPolicyAmendment = "deniedPolicyAmendment",
        /// Nobody decided in time.
        TimedOut = "timedOut",
        /// Rejected: the tool call doesn't run.
        Abort = "abort",
    }
}

impl ApprovalDecision {
    /// Whether this decision lets the action run.
    pub fn allows(&self) -> bool {
        matches!(
            self,
            Self::Approved | Self::ApprovedForSession | Self::ApprovedPolicyAmendment
        )
    }
}

open_enum! {
    /// How long a grant lasts.
    pub enum ApprovalChoiceScope {
        /// This request only.
        Once = "once",
        /// The rest of the session.
        Session = "session",
        /// Saved in the local policy store.
        LocalPersistent = "localPersistent",
    }
}

/// One option for deciding an approval. Show `label`; send `choiceId`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalChoice {
    /// Whether `approval/decide` may carry `feedback` for the model with this choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepts_feedback: Option<bool>,
    /// The id to send. The set is open: act on `decision`, not on specific ids.
    pub choice_id: String,
    /// What the choice applies.
    pub decision: ApprovalDecision,
    /// The label to show, such as "Allow once".
    pub label: String,
    /// What the choice grants, such as the rule it would save. Show it exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_preview: Option<String>,
    /// How long the grant lasts.
    pub scope: ApprovalChoiceScope,
}

/// Names one requirement of an approval: for a shell command, one stage.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequirementRef {
    /// The approval.
    pub approval_id: String,
    /// The requirement's index, from 0.
    pub source_index: u32,
}

/// How a shell stage was resolved: `unresolved`, `allowOnce`, and so on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalStageResolution {
    /// The matching rule's prefix, when a saved rule satisfied the stage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv_prefix: Vec<String>,
    /// A diagnostic, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
    /// The resolution.
    pub kind: String,
}

/// A command prefix Muse suggests always allowing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSuggestedPrefix {
    /// The prefix, such as `["echo"]`.
    pub argv_prefix: Vec<String>,
    /// The label to show.
    pub label: String,
}

/// One stage of a shell command: one command of a pipeline or command list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalStage {
    /// The command words.
    pub argv: Vec<String>,
    /// Whether `argv` is the complete command, known before it runs.
    pub argv_complete: bool,
    /// The stage's position, from 1.
    pub position: u32,
    /// The stage's requirement.
    pub requirement_id: ApprovalRequirementRef,
    /// Whether it still needs a decision.
    pub resolution: ApprovalStageResolution,
    /// A prefix to always allow, when Muse suggests one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_prefix: Option<ApprovalSuggestedPrefix>,
    /// The number of stages.
    pub total_stages: u32,
}

open_enum! {
    /// What an approval is for.
    pub enum ApprovalSubjectKind {
        /// A shell command: `command`, `stages`, `workspaceRoot`.
        Shell = "shell",
        /// Reading or writing a file: `path`, `access`.
        FileAccess = "fileAccess",
        /// A network connection: `host`, `port`, `protocol`.
        Network = "network",
        /// A Unix socket: `path`.
        UnixSocket = "unixSocket",
        /// Controlling a process: `target`.
        Process = "process",
        /// A tool call: `toolName`.
        Tool = "tool",
    }
}

/// What an approval is for. Show kinds you don't know generically, and never auto-approve them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSubject {
    /// `fileAccess`: `read` or `write`; `process`: `control`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
    /// `shell`: the full command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// `network`: the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The kind.
    pub kind: ApprovalSubjectKind,
    /// `fileAccess`, `unixSocket`: the path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `network`: the port, when named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// `network`: the protocol, such as `https`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// `shell`: the stages, approved one by one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<ApprovalStage>,
    /// `process`: the process to control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Every kind but `shell`: the tool asking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// `shell`: the workspace the command runs in, when known. On Windows Muse sends it as a
    /// `\\?\` path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
}

/// `approval/request` and `approval/requested` params: an approval waiting for a decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestParams {
    /// The approval's id. Match later events by it, never by the JSON-RPC id.
    pub approval_id: String,
    /// The choices for the current requirement.
    pub available_choices: Vec<ApprovalChoice>,
    /// The requirement the next decision must name.
    pub current_requirement_id: ApprovalRequirementRef,
    /// The `toolCall` item waiting on this approval.
    pub item_id: String,
    /// Whether an automated reviewer saw it first and passed it to a person.
    pub judge_escalated: bool,
    /// Whether it guards a write to a protected file: only allow-once and reject are offered.
    pub protected_write: bool,
    /// The tool call's arguments, usually JSON.
    pub raw_args: String,
    /// The reviewer's reason for escalating, as one display line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_reason: Option<String>,
    /// The session.
    pub session_id: String,
    /// What it's for.
    pub subject: ApprovalSubject,
    /// The task running the tool call.
    pub task_id: String,
    /// The provider's id for the tool call.
    pub tool_call_id: String,
    /// The tool, such as `powershell`.
    pub tool_name: String,
    /// The turn waiting on it.
    pub turn_id: String,
    /// The cursor of the `approval/requested` event that opened it.
    pub view_cursor: String,
}

impl ApprovalRequestParams {
    /// The first choice that applies `decision`.
    pub fn choice(&self, decision: &ApprovalDecision) -> Option<&ApprovalChoice> {
        self.available_choices
            .iter()
            .find(|choice| choice.decision == *decision)
    }

    /// The narrowest choice that lets the call run: allow once if offered, else the first
    /// allowing choice.
    pub fn allow_once(&self) -> Option<&ApprovalChoice> {
        self.available_choices
            .iter()
            .find(|choice| choice.decision.allows() && choice.scope == ApprovalChoiceScope::Once)
            .or_else(|| {
                self.available_choices
                    .iter()
                    .find(|choice| choice.decision.allows())
            })
    }

    /// A choice that stops the call: `abort` or `denied`, once if offered.
    pub fn reject(&self) -> Option<&ApprovalChoice> {
        let rejects = |choice: &&ApprovalChoice| {
            matches!(
                choice.decision,
                ApprovalDecision::Abort | ApprovalDecision::Denied
            )
        };
        self.available_choices
            .iter()
            .filter(rejects)
            .find(|choice| choice.scope == ApprovalChoiceScope::Once)
            .or_else(|| self.available_choices.iter().find(rejects))
    }
}

/// `approval/decide` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecideParams {
    /// The approval.
    pub approval_id: String,
    /// One of the current `availableChoices`.
    pub choice_id: String,
    /// Idempotency handle (UUIDv7).
    pub command_id: String,
    /// Guidance for the model, only with a choice that `acceptsFeedback`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    /// The approval's `currentRequirementId`, unchanged.
    pub requirement_id: ApprovalRequirementRef,
    /// The session.
    pub session_id: String,
}

impl ApprovalDecideParams {
    /// Decides `request` with `choice`, with a fresh `commandId`.
    pub fn new(request: &ApprovalRequestParams, choice: &ApprovalChoice) -> Self {
        Self {
            approval_id: request.approval_id.clone(),
            choice_id: choice.choice_id.clone(),
            command_id: new_command_id(),
            feedback: None,
            requirement_id: request.current_requirement_id.clone(),
            session_id: request.session_id.clone(),
        }
    }
}

/// `approval/decide` result: admission only. The outcome arrives as `approval/updated` and
/// `approval/resolved`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecideResult {
    /// The approval.
    pub approval_id: String,
    /// Echo of the `commandId`.
    pub command_id: String,
    /// Admission status.
    pub status: CommandStatus,
    /// `false` when more stages still need a decision.
    pub terminal: bool,
}

/// What an `approval/updated` reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalChange {
    /// `stageResolved`: the choice that satisfied the stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice_id: Option<String>,
    /// `stageResolved`: what that choice applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ApprovalDecision>,
    /// What happened: `stageResolved`, `policyPersistence`, `reviewStarted`, and so on.
    pub kind: String,
    /// `policyPersistence` failures: why saving the rule failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `stageResolved`: the stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement_id: Option<ApprovalRequirementRef>,
    /// `policyPersistence`: `succeeded` or `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// `approval/updated` params: a pending approval changed without being decided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalUpdatedParams {
    /// The approval.
    pub approval_id: String,
    /// The choices now.
    pub available_choices: Vec<ApprovalChoice>,
    /// What changed.
    pub change: ApprovalChange,
    /// The requirement now current.
    pub current_requirement_id: ApprovalRequirementRef,
    /// The session.
    pub session_id: String,
    /// The subject, with stage resolutions updated.
    pub subject: ApprovalSubject,
    /// The view position.
    pub view_cursor: String,
}

open_enum! {
    /// Whether a final decision lets the action run.
    pub enum ApprovalPolicyResult {
        /// It runs.
        Allow = "allow",
        /// It doesn't.
        Deny = "deny",
    }
}

open_enum! {
    /// Who made a final decision.
    pub enum ApprovalResolvedBy {
        /// A person, through `approval/decide`.
        User = "user",
        /// A saved policy rule.
        Policy = "policy",
        /// Muse's automated LLM reviewer.
        LlmJudge = "llmJudge",
    }
}

/// `approval/resolved` params: the final decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalResolvedParams {
    /// The approval.
    pub approval_id: String,
    /// When, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
    /// The `approval/decide` command that won, if a person decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by_command_id: Option<String>,
    /// The decision.
    pub decision: ApprovalDecision,
    /// The `toolCall` item it held.
    pub item_id: String,
    /// Whether the action runs.
    pub policy_result: ApprovalPolicyResult,
    /// Who decided.
    pub resolved_by: ApprovalResolvedBy,
    /// The session.
    pub session_id: String,
    /// The turn it held.
    pub turn_id: String,
    /// The view position.
    pub view_cursor: String,
}

/// `approval/listPending` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalListPendingParams {
    /// The session.
    pub session_id: String,
}

/// `approval/listPending` result: everything waiting on a person in a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalListPendingResult {
    /// Pending approvals.
    pub approvals: Vec<ApprovalRequestParams>,
    /// Pending user-input prompts.
    pub user_inputs: Vec<UserInputRequestParams>,
}
