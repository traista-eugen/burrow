//! The inbox: `inbox/list`, `inbox/get` and `inbox/respond`.
//!
//! Every point where a human is needed becomes an [`InboxItem`]. In M2 the only kind `burrowd`
//! creates is `permission`, from agent permission requests. Inbox changes also appear on the
//! `index` stream as `inboxItemAdded` and `inboxItemResolved`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact::ArtifactRef;
use crate::events::{InboxItemKind, InboxItemStatus, InboxSource};
use crate::time::Timestamp;

/// Something that waits on a human.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    /// The item's id (`inb_…`).
    pub id: String,
    /// What it asks for.
    pub kind: InboxItemKind,
    /// A one-line summary.
    pub title: String,
    /// More detail, in Markdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Where it comes from.
    #[serde(default)]
    pub source: InboxSource,
    /// The possible answers. For a permission request, the agent's options.
    pub actions: Vec<InboxAction>,
    /// A JSON Schema for a structured answer, sent as `payload`, for example a feedback field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<Value>,
    /// Files to look at before answering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ArtifactRef>,
    /// Open, resolved or expired.
    pub status: InboxItemStatus,
    /// The answer, once resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<InboxResolution>,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it expires on its own, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// When it was resolved or expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<Timestamp>,
}

/// One possible answer to an inbox item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxAction {
    /// The action's id, sent as `action` in `inbox/respond`.
    pub id: String,
    /// Its label, for example `Allow once`.
    pub label: String,
    /// How to draw it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<InboxActionStyle>,
}

/// How to draw an inbox action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum InboxActionStyle {
    /// The suggested answer.
    Primary,
    /// An answer that refuses or destroys something.
    Danger,
}

/// How an inbox item was answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxResolution {
    /// The action chosen.
    pub action: String,
    /// The structured answer, if the item has a `form`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    /// Who answered: a device id, or `local`.
    pub by: String,
    /// When.
    pub at: Timestamp,
}

/// `inbox/list` params.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxListParams {
    /// Only items with this status. All of them when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<InboxItemStatus>,
    /// At most this many. Default 100, at most 1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Only items created before this one: pass the last id of the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// Params naming one inbox item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxItemIdParams {
    /// The item's id.
    pub item_id: String,
}

/// `inbox/respond` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxRespondParams {
    /// The item.
    pub item_id: String,
    /// One of the item's action ids.
    pub action: String,
    /// The structured answer, if the item has a `form`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}
