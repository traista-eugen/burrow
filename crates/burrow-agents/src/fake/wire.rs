//! The fake agent's protocol: one JSON object per line on stdin and stdout, tagged by `type`.
//!
//! It is deliberately trivial, so the fake exercises the launcher and the session manager
//! rather than a protocol. Payload enums reuse Burrow's protocol types.

use burrow_protocol::{
    PermissionOptionKind, Plan, Timestamp, ToolCall, ToolCallStatus, ToolCallUpdate, ToolKind,
    TurnEndReason,
};
use serde::{Deserialize, Serialize};

/// Adapter → fake agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ToAgent {
    /// Start a turn.
    Prompt {
        /// The prompt's text.
        text: String,
    },
    /// Stop the running turn.
    Cancel,
    /// Use another model.
    SetModel {
        /// The model.
        model: String,
    },
    /// The answer to a permission request.
    Permission {
        /// The request's id.
        id: String,
        /// The option chosen; `None` when the request was cancelled.
        option_id: Option<String>,
    },
}

/// Fake agent → adapter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FromAgent {
    /// Started and ready for prompts.
    Ready {
        /// The model in use.
        model: String,
        /// Models it accepts.
        models: Vec<String>,
        /// Its own process id.
        pid: u32,
        /// The id of the child it started, so tests can check the whole tree dies.
        child_pid: Option<u32>,
    },
    /// A piece of the answer.
    Chunk {
        /// The text.
        text: String,
    },
    /// A piece of reasoning.
    Thought {
        /// The text.
        text: String,
    },
    /// A tool call.
    ToolCall {
        /// Its id.
        id: String,
        /// What it does.
        title: String,
        /// Its kind.
        kind: ToolKind,
        /// Its status.
        status: ToolCallStatus,
    },
    /// A tool call changed.
    ToolCallUpdate {
        /// Its id.
        id: String,
        /// The new status.
        status: ToolCallStatus,
        /// Output text.
        text: Option<String>,
    },
    /// A plan.
    Plan {
        /// The entries' text, all pending, medium priority.
        entries: Vec<String>,
    },
    /// A tool call with everything Burrow's events carry: locations and content. For the
    /// content scripts.
    Call {
        /// The call.
        call: ToolCall,
    },
    /// A change to a tool call, with everything Burrow's events carry.
    CallUpdate {
        /// The change.
        update: ToolCallUpdate,
    },
    /// A plan with each entry's status and priority.
    FullPlan {
        /// The plan.
        plan: Plan,
    },
    /// A permission request; answered with [`ToAgent::Permission`].
    Permission {
        /// The request's id.
        id: String,
        /// The tool call it is about.
        tool_call_id: String,
        /// What it wants to do.
        title: String,
        /// The options.
        options: Vec<FakeOption>,
    },
    /// The fake no longer waits for the answer to a permission request.
    Withdraw {
        /// The request's id.
        id: String,
    },
    /// Tokens used by the turn.
    Usage {
        /// Input tokens.
        input_tokens: u64,
        /// Output tokens.
        output_tokens: u64,
        /// Cost in US dollars.
        cost_usd: Option<f64>,
    },
    /// A simulated usage limit.
    Limit {
        /// When it resets.
        resets_at: Option<Timestamp>,
        /// The message.
        message: String,
    },
    /// The model changed.
    ModelChanged {
        /// The new model.
        model: String,
    },
    /// The turn is over.
    TurnEnded {
        /// Why.
        reason: TurnEndReason,
        /// Detail.
        message: Option<String>,
    },
}

/// A permission option.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FakeOption {
    /// The option's id.
    pub id: String,
    /// Its label.
    pub name: String,
    /// What it does.
    pub kind: PermissionOptionKind,
}
