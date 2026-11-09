//! User input: the model asks the user up to three questions (`request_user_input`).
//!
//! Like approvals: `userInput/request` (answered with a receipt, which [`crate::MspClient`]
//! sends) plus `userInput/requested`, then the client answers with `userInput/answer` or
//! declines with `userInput/cancel`, and Muse confirms with `userInput/settled`. Hosts started
//! with `--disable-request-user-input` never ask.

use serde::{Deserialize, Serialize};

use super::{CommandStatus, new_command_id};

/// One option of a question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UserInputOption {
    /// More detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The label, which is also what an answer sends back.
    pub label: String,
}

/// How many options a question takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UserInputSelection {
    /// At most this many, for `multiple`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_selections: Option<u32>,
    /// At least this many, for `multiple`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_selections: Option<u32>,
    /// `single` or `multiple`.
    pub mode: String,
}

/// One question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UserInputQuestion {
    /// A short header.
    pub header: String,
    /// The id an answer names.
    pub id: String,
    /// The options.
    pub options: Vec<UserInputOption>,
    /// The question.
    pub question: String,
    /// How many options it takes.
    pub selection: UserInputSelection,
}

/// `userInput/request` and `userInput/requested` params: a prompt waiting for an answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UserInputRequestParams {
    /// Per question: the prompt settles as `timedOut` after this times the question count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resolution_ms: Option<u64>,
    /// The `toolCall` item waiting on it.
    pub item_id: String,
    /// One to three questions, in order.
    pub questions: Vec<UserInputQuestion>,
    /// The session.
    pub session_id: String,
    /// The provider's id for the tool call.
    pub tool_call_id: String,
    /// The tool that asked.
    pub tool_name: String,
    /// The turn waiting on it.
    pub turn_id: String,
    /// The prompt's id.
    pub user_input_id: String,
    /// The cursor of its `userInput/requested` event.
    pub view_cursor: String,
}

/// One question's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UserInputAnswer {
    /// A free-form answer, at most 500 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_text: Option<String>,
    /// A comment on the answer, at most 500 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The question's `id`.
    pub question_id: String,
    /// The chosen option, for a `single` question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_label: Option<String>,
    /// The chosen options, for a `multiple` question.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_labels: Vec<String>,
}

/// `userInput/answer` params: answers every question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UserInputAnswerParams {
    /// One per question.
    pub answers: Vec<UserInputAnswer>,
    /// Idempotency handle (UUIDv7).
    pub command_id: String,
    /// The session.
    pub session_id: String,
    /// The prompt.
    pub user_input_id: String,
}

/// `userInput/answer` and `userInput/cancel` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UserInputAnswerResult {
    /// Echo of the `commandId`.
    pub command_id: String,
    /// Admission status.
    pub status: CommandStatus,
    /// The prompt.
    pub user_input_id: String,
}

/// `userInput/cancel` result: the same shape as [`UserInputAnswerResult`].
pub type UserInputCancelResult = UserInputAnswerResult;

/// `userInput/cancel` params: decline the prompt; the model sees a cancelled result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UserInputCancelParams {
    /// Idempotency handle (UUIDv7).
    pub command_id: String,
    /// Why, for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The session.
    pub session_id: String,
    /// The prompt.
    pub user_input_id: String,
}

impl UserInputCancelParams {
    /// Declines `request`, with a fresh `commandId`.
    pub fn new(request: &UserInputRequestParams) -> Self {
        Self {
            command_id: new_command_id(),
            reason: None,
            session_id: request.session_id.clone(),
            user_input_id: request.user_input_id.clone(),
        }
    }
}

/// `userInput/settled` params: the prompt's final outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UserInputSettledParams {
    /// The answers, when answered.
    #[serde(default)]
    pub answers: Vec<UserInputAnswer>,
    /// `answered`, `cancelled`, `interrupted`, `clarified`, `timedOut` or `aborted`.
    pub outcome: String,
    /// The session.
    pub session_id: String,
    /// The prompt.
    pub user_input_id: String,
    /// The view position.
    pub view_cursor: String,
}
