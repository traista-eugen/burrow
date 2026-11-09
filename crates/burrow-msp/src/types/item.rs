//! Transcript items and their notifications.
//!
//! An item is one row of a session's transcript: a user message, an agent message, reasoning,
//! a tool call, and so on. Its life on the wire:
//! - `item/started` opens it at `revision` 1;
//! - `item/delta` appends text to one of its fields (`text`, `output`, `summary.N`);
//! - `item/updated` re-sends it whole at a higher revision;
//! - `item/completed` sends its final state. Single-shot items only get this one.
//!
//! [`Item`] mirrors the wire's flat object, where `kind` says which members apply.
//! [`Item::details`] gives a typed view of the kinds a coding session cares about.

use serde::{Deserialize, Serialize};

open_enum! {
    /// What an item is. Render kinds you don't know generically: kind, status, `fallbackText`.
    pub enum ItemKind {
        /// The user's prompt.
        UserMessage = "userMessage",
        /// The agent's reply. `text` streams as deltas.
        AgentMessage = "agentMessage",
        /// The model's reasoning summary. `summary.N` streams as deltas.
        Reasoning = "reasoning",
        /// A tool call. `output` streams as deltas.
        ToolCall = "toolCall",
        /// A shell command the user ran through `session/userShell`.
        UserShell = "userShell",
        /// A subagent.
        Subagent = "subagent",
        /// A workflow run.
        Workflow = "workflow",
        /// A reminder agent that runs at the end of a turn, such as `goal-reminder`.
        ReminderChild = "reminderChild",
        /// Context compaction.
        Compaction = "compaction",
        /// A hook run.
        HookRun = "hookRun",
        /// A side chat.
        SideChat = "sideChat",
        /// A web search.
        WebSearch = "webSearch",
        /// A monitor's feed.
        MonitorFeed = "monitorFeed",
        /// A damaged range of the log.
        DamagedRange = "damagedRange",
        /// A scheduled job fired.
        ScheduledFire = "scheduledFire",
        /// A background terminal finished.
        BackgroundTerminalCompletion = "backgroundTerminalCompletion",
        /// A notice that something is overdue.
        OverdueNotice = "overdueNotice",
        /// History before a compaction boundary.
        EarlierHistory = "earlierHistory",
    }
}

open_enum! {
    /// An item's status. Anything but `inProgress` is terminal, unknown values included.
    pub enum ItemStatus {
        /// Still open.
        InProgress = "inProgress",
        /// Finished.
        Completed = "completed",
        /// Failed; see `failureReason`.
        Failed = "failed",
        /// Cancelled.
        Cancelled = "cancelled",
        /// Rejected, for example by an approval decision.
        Rejected = "rejected",
        /// Timed out.
        TimedOut = "timedOut",
    }
}

impl ItemStatus {
    /// Whether the item is finished.
    pub fn is_terminal(&self) -> bool {
        *self != Self::InProgress
    }
}

open_enum! {
    /// The phase an agent message belongs to.
    pub enum MessagePhase {
        /// Commentary while working, such as "I'll run that command".
        Commentary = "commentary",
        /// The turn's answer.
        FinalAnswer = "finalAnswer",
    }
}

/// A tool's own words for a call, in three tenses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ToolDisplayLabel {
    /// While running, such as "Reading foo.rs".
    pub active: String,
    /// Once done, such as "Read foo.rs".
    pub done: String,
    /// Neutral, such as "Read foo.rs".
    pub neutral: String,
}

/// A reference to stored output, fetched with `item/readOutput`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct OutputRef {
    /// Whether the bytes can be read: `available`, `missing`, `unsupported` or `accessFailed`.
    pub availability: String,
    /// The stored size in bytes.
    pub byte_len: u64,
    /// A content digest, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Pass this as `outputRef` to `item/readOutput`.
    pub id: String,
    /// What the output is, such as `tool_output` or `tool_patch`.
    pub kind: String,
    /// The media type, such as `application/json` for a patch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// A display path, when it names a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Display and provenance only; never fetch by it.
    pub uri: String,
}

/// The size of an edit, computed by Muse for edit-family tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct PatchSummary {
    /// Lines added.
    pub added: u64,
    /// Files touched.
    pub files: u64,
    /// Lines removed.
    pub removed: u64,
}

/// One transcript item. Which members are set depends on `kind`; the docs name the kinds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// `toolCall`: the approval that gated the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    /// `toolCall`: the arguments as the model wrote them, usually JSON. Parse with care.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// `toolCall`: whether the task was sent to the background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// `toolCall`: the provider's call id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// `subagent`, `reminderChild`: the child's own session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
    /// `userMessage`, `userShell`: the submitting command, for de-duplicating a local echo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    /// `userShell`: the command as submitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_text: Option<String>,
    /// `agentMessage`: the stream died mid-reply and this partial text was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_off: Option<bool>,
    /// `toolCall`: the tool's own words for this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_label: Option<ToolDisplayLabel>,
    /// `userMessage`: the presentation form of the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_text: Option<String>,
    /// Observed duration, for kinds that measure one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// `userShell`: the exit code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// `toolCall`: the failure class, snake_case, such as `environment_failure`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    /// `toolCall`, `subagent`: why it failed, was rejected or cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// A one-line summary for rendering kinds a client doesn't know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_text: Option<String>,
    /// The item's id, stable across its life.
    pub item_id: String,
    /// What the item is.
    pub kind: ItemKind,
    /// `agentMessage`: commentary or the final answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_phase: Option<MessagePhase>,
    /// `toolCall`, `userShell`: the full stored output, when the view shows only part of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<OutputRef>,
    /// `toolCall`: the stored structured patch of an edit, fetched with `item/readOutput`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_ref: Option<OutputRef>,
    /// `toolCall`: the edit's size; set whenever `patchRef` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_summary: Option<PatchSummary>,
    /// `reasoning`: the provider's reasoning item id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_item_id: Option<String>,
    /// RFC 3339 time of the item's durable record; absent on items opened by a delta stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
    /// `userMessage`: the submission was retracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracted: Option<bool>,
    /// Starts at 1 and only grows. Apply an update only if its revision is higher.
    pub revision: u64,
    /// The status.
    pub status: ItemStatus,
    /// `toolCall`: the task's latest status line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<String>,
    /// `userMessage`: injected into a running turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steered: Option<bool>,
    /// `reasoning`: one entry per summary part; part N streams as field `summary.N`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub summary: Vec<String>,
    /// `userMessage`: the prompt. `agentMessage`: the reply so far. `reasoning`: raw reasoning
    /// text where the provider exposes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `toolCall`: the tool's name, such as `powershell` or `write_file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// A streamed text surface hit Muse's size budget; the full text stays in the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// The turn the item belongs to; `None` for kinds outside turns.
    #[serde(default)]
    pub turn_id: Option<String>,
    /// `toolCall`, `userShell`: the visible part of the output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_output: Option<String>,
}

/// A typed view of an [`Item`], by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemDetails<'a> {
    /// The user's prompt.
    UserMessage {
        /// The prompt as submitted.
        text: &'a str,
    },
    /// The agent's reply, or commentary while it works.
    AgentMessage {
        /// The text so far; final once the item completes.
        text: &'a str,
        /// The phase, when the model declared one.
        phase: Option<&'a MessagePhase>,
    },
    /// The model's reasoning.
    Reasoning {
        /// Summary parts, in order.
        summary: &'a [String],
        /// Raw reasoning text, where the provider exposes it.
        text: Option<&'a str>,
    },
    /// A tool call.
    ToolCall(ToolCall<'a>),
    /// A shell command the user ran.
    UserShell {
        /// The command.
        command: Option<&'a str>,
        /// The visible output.
        output: Option<&'a str>,
        /// The exit code, once known.
        exit_code: Option<i64>,
    },
    /// Any other kind, including ones this crate doesn't know.
    Other {
        /// The kind.
        kind: &'a ItemKind,
        /// Muse's one-line summary, when it sent one.
        fallback_text: Option<&'a str>,
    },
}

/// A tool call, as [`ItemDetails::ToolCall`] shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolCall<'a> {
    /// The tool's name.
    pub tool: &'a str,
    /// The provider's call id.
    pub call_id: Option<&'a str>,
    /// The arguments as the model wrote them.
    pub args: Option<&'a str>,
    /// The tool's own label for the call.
    pub label: Option<&'a ToolDisplayLabel>,
    /// The visible output so far.
    pub output: Option<&'a str>,
    /// Why it failed, when it did.
    pub failure: Option<&'a str>,
    /// The approval that gated it, if any.
    pub approval_id: Option<&'a str>,
    /// The edit it made, for edit-family tools.
    pub diff: Option<Diff<'a>>,
}

/// An edit a tool call made: its size, and where to fetch the structured patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diff<'a> {
    /// Files and lines touched.
    pub summary: &'a PatchSummary,
    /// The stored patch; fetch it with `item/readOutput` and this ref's `id`.
    pub patch: Option<&'a OutputRef>,
}

impl Item {
    /// A typed view by kind.
    pub fn details(&self) -> ItemDetails<'_> {
        match self.kind {
            ItemKind::UserMessage => ItemDetails::UserMessage {
                text: self.text.as_deref().unwrap_or_default(),
            },
            ItemKind::AgentMessage => ItemDetails::AgentMessage {
                text: self.text.as_deref().unwrap_or_default(),
                phase: self.message_phase.as_ref(),
            },
            ItemKind::Reasoning => ItemDetails::Reasoning {
                summary: &self.summary,
                text: self.text.as_deref(),
            },
            ItemKind::ToolCall => ItemDetails::ToolCall(ToolCall {
                tool: self.tool.as_deref().unwrap_or_default(),
                call_id: self.call_id.as_deref(),
                args: self.args.as_deref(),
                label: self.display_label.as_ref(),
                output: self.visible_output.as_deref(),
                failure: self.failure_reason.as_deref(),
                approval_id: self.approval_id.as_deref(),
                diff: self.patch_summary.as_ref().map(|summary| Diff {
                    summary,
                    patch: self.patch_ref.as_ref(),
                }),
            }),
            ItemKind::UserShell => ItemDetails::UserShell {
                command: self.command_text.as_deref(),
                output: self.visible_output.as_deref(),
                exit_code: self.exit_code,
            },
            _ => ItemDetails::Other {
                kind: &self.kind,
                fallback_text: self.fallback_text.as_deref(),
            },
        }
    }
}

/// `item/started`, `item/updated` and `item/completed` params: the full item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ItemParams {
    /// The item.
    pub item: Item,
    /// The session.
    pub session_id: String,
    /// The view position.
    pub view_cursor: String,
}

/// `item/delta` params: text appended to one field of an open item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ItemDeltaParams {
    /// The appended text.
    pub delta: String,
    /// The dotted field path; absent means `text`. See [`ItemDeltaParams::field`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// The item.
    pub item_id: String,
    /// The session.
    pub session_id: String,
    /// The view position.
    pub view_cursor: String,
}

/// Which field an `item/delta` appends to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeltaField<'a> {
    /// `text`: an agent message's reply.
    Text,
    /// `output`: a tool call's or user shell's visible output.
    Output,
    /// `summary.N`: reasoning summary part N.
    Summary(usize),
    /// Any other path.
    Other(&'a str),
}

impl ItemDeltaParams {
    /// The field this delta appends to.
    pub fn field(&self) -> DeltaField<'_> {
        match self.field.as_deref() {
            None | Some("text") => DeltaField::Text,
            Some("output") => DeltaField::Output,
            Some(path) => match path.strip_prefix("summary.").and_then(|n| n.parse().ok()) {
                Some(index) => DeltaField::Summary(index),
                None => DeltaField::Other(path),
            },
        }
    }
}

/// `item/readOutput` params: read a page of stored output, such as an edit's patch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ItemReadOutputParams {
    /// The item.
    pub item_id: String,
    /// At most this many stored bytes; 6 MiB when absent, and at most that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_bytes: Option<u64>,
    /// Where to start; 0 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_bytes: Option<u64>,
    /// The `id` of the item's `outputRef` or `patchRef`.
    pub output_ref: String,
    /// The session.
    pub session_id: String,
}

/// How `item/readOutput` encodes its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum OutputEncoding {
    /// Text media.
    Utf8,
    /// Binary media.
    Base64,
}

/// `item/readOutput` result: one page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ItemReadOutputResult {
    /// Stored bytes in this page. The next page starts at `offsetBytes + byteLen`.
    pub byte_len: u64,
    /// The page, decoded per `encoding`.
    pub content: String,
    /// How `content` is encoded.
    pub encoding: OutputEncoding,
    /// Whether this page reaches the end.
    pub eof: bool,
    /// The stored output's media type.
    pub media_type: String,
    /// Where this page starts.
    pub offset_bytes: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_tool_call_with_a_patch_shows_its_diff() {
        let item: Item = serde_json::from_value(json!({
            "itemId": "01a1bf46-cc1c-7143-aedf-cf2804bfbed5", "kind": "toolCall", "revision": 2,
            "status": "completed", "turnId": "t", "tool": "edit_file",
            "args": "{\"path\":\"a.rs\"}",
            "patchSummary": {"files": 1, "added": 3, "removed": 1},
            "patchRef": {"id": "p1", "kind": "tool_patch", "uri": "muse://p1", "byteLen": 120,
                         "availability": "available", "mediaType": "application/json"}
        }))
        .unwrap();
        let ItemDetails::ToolCall(call) = item.details() else {
            panic!("expected a tool call");
        };
        assert_eq!(call.tool, "edit_file");
        let diff = call.diff.expect("a diff");
        assert_eq!(
            (diff.summary.files, diff.summary.added, diff.summary.removed),
            (1, 3, 1)
        );
        assert_eq!(diff.patch.map(|patch| patch.id.as_str()), Some("p1"));
    }

    #[test]
    fn unknown_kinds_fall_back_to_other() {
        let item: Item = serde_json::from_value(json!({
            "itemId": "x", "kind": "hologram", "revision": 1, "status": "glowing",
            "fallbackText": "A hologram"
        }))
        .unwrap();
        assert_eq!(
            item.details(),
            ItemDetails::Other {
                kind: &ItemKind::Other("hologram".into()),
                fallback_text: Some("A hologram"),
            }
        );
        assert!(item.status.is_terminal());
    }

    #[test]
    fn delta_fields() {
        let delta = |field: Option<&str>| ItemDeltaParams {
            delta: String::new(),
            field: field.map(str::to_owned),
            item_id: String::new(),
            session_id: String::new(),
            view_cursor: String::new(),
        };
        assert_eq!(delta(None).field(), DeltaField::Text);
        assert_eq!(delta(Some("text")).field(), DeltaField::Text);
        assert_eq!(delta(Some("output")).field(), DeltaField::Output);
        assert_eq!(delta(Some("summary.2")).field(), DeltaField::Summary(2));
        assert_eq!(
            delta(Some("summary.x")).field(),
            DeltaField::Other("summary.x")
        );
    }
}
