//! The transcript model: a session's events grouped into the rows the transcript shows. No GPUI.
//!
//! [`TranscriptModel::apply`] takes events in seq order and records each [`Change`], so the view
//! touches only what changed: a streamed message grows in place ([`Change::Appended`]), a tool
//! call's updates change its row ([`Change::Updated`]), and everything else adds a row
//! ([`Change::Added`]). Rows are only ever appended or changed in place; nothing is removed or
//! inserted before the end, so a row's index is stable for the life of the model.
//!
//! Grouping:
//! - Consecutive `agentMessageChunk`s of a turn join into one [`Item::Agent`], and
//!   `agentThoughtChunk`s into one [`Item::Thought`]; anything in between starts a new one.
//! - A run of two or more consecutive read or search tool calls becomes one
//!   [`Item::ToolGroup`], so exploring a codebase doesn't swamp the view. A lone one stays an
//!   [`Item::Tool`].
//! - A turn's `plan` events update one [`Item::Plan`] in place, where the turn's first plan
//!   appeared. The next turn's first plan starts a new one.
//! - `usage` is summed per turn and shown on the turn's [`Item::TurnEnded`], with its duration.
//! - A permission request shows how it ended. An expired one says why ([`Expiry`]), worked out
//!   from what follows it: the turn carrying on means the agent withdrew it (D-031).

use std::collections::HashMap;

use burrow_protocol::{
    ContentBlock, Event, InboxItemStatus, PermissionOption, PlanEntry, SessionState, StreamEvent,
    Timestamp, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolKind, TurnEndReason,
};

/// One tool call, kept current by its updates.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallItem {
    /// Its id, unique within the session.
    pub tool_call_id: String,
    /// A one-line description, for example `Edit README.md`.
    pub title: String,
    /// What it does.
    pub kind: ToolKind,
    /// How far it has got.
    pub status: ToolCallStatus,
    /// Files it touches.
    pub locations: Vec<ToolCallLocation>,
    /// What it produced or proposes: text, diffs, terminals.
    pub content: Vec<ToolCallContent>,
    /// Bumped by every change, so views can cache what they compute from it.
    pub revision: u32,
}

impl ToolCallItem {
    /// Whether runs of calls like this one are collapsed into a group: reads and searches.
    pub fn is_groupable(&self) -> bool {
        is_groupable(&self.kind)
    }
}

/// Whether runs of calls of this kind are collapsed into a group: reads and searches.
pub fn is_groupable(kind: &ToolKind) -> bool {
    matches!(kind, ToolKind::Read | ToolKind::Search)
}

/// Tokens and cost of one turn, summed from its `usage` events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnUsage {
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cost in US dollars, when the agent reports it.
    pub cost_usd: Option<f64>,
}

impl TurnUsage {
    fn add(&mut self, usage: &burrow_protocol::Usage) {
        self.input_tokens += usage.input_tokens;
        self.output_tokens += usage.output_tokens;
        if let Some(cost) = usage.cost_usd {
            *self.cost_usd.get_or_insert(0.0) += cost;
        }
    }
}

/// The end of a turn.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnEnd {
    /// The turn.
    pub turn_id: String,
    /// Why it ended.
    pub reason: TurnEndReason,
    /// More detail, for example the error.
    pub message: Option<String>,
    /// How long it ran, from its prompt to its end, when the prompt is in the transcript.
    pub duration_ms: Option<i64>,
    /// What it used, when the agent reported it.
    pub usage: Option<TurnUsage>,
}

/// Why a permission request expired without an answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Expiry {
    /// Not known yet: it expired while its turn was running, and the turn hasn't shown why.
    Pending,
    /// The agent stopped waiting for the answer and carried on (D-031), or finished the turn
    /// without it.
    Withdrawn,
    /// The turn ended first, for this reason: cancelled, stopped, or failed.
    TurnEnded {
        /// Why the turn ended.
        reason: TurnEndReason,
        /// The turn's end message, for example `the session was stopped`.
        message: Option<String>,
    },
    /// It expired when no turn was running, for example after `burrowd` restarted.
    NoTurn,
}

/// How a permission request ended, so far.
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionOutcome {
    /// Waiting for an answer.
    Open,
    /// Answered with this option.
    Chosen {
        /// The option's id.
        option: String,
        /// Who answered: a device id, or `local`.
        by: Option<String>,
    },
    /// Nobody answered.
    Expired(Expiry),
}

/// A permission request, as the transcript shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionItem {
    /// The inbox item that holds the question.
    pub inbox_item_id: String,
    /// The tool call it is about.
    pub tool_call_id: String,
    /// What the agent wants to do.
    pub title: String,
    /// The answers offered.
    pub options: Vec<PermissionOption>,
    /// How it ended.
    pub outcome: PermissionOutcome,
}

impl PermissionItem {
    /// The name of the chosen option, if one was chosen.
    pub fn chosen_name(&self) -> Option<&str> {
        let PermissionOutcome::Chosen { option, .. } = &self.outcome else {
            return None;
        };
        Some(
            self.options
                .iter()
                .find(|o| &o.option_id == option)
                .map_or(option.as_str(), |o| o.name.as_str()),
        )
    }
}

/// How a one-line notice looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Quiet: a model or mode change, a stop.
    Quiet,
    /// An error: the session failed.
    Error,
}

/// One row of the transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// The prompt that started a turn.
    User {
        /// The turn.
        turn_id: String,
        /// The prompt's text.
        text: String,
    },
    /// The agent's answer, chunks joined. Markdown.
    Agent {
        /// The turn.
        turn_id: String,
        /// The text so far.
        text: String,
    },
    /// The agent's visible reasoning, chunks joined.
    Thought {
        /// The turn.
        turn_id: String,
        /// The text so far.
        text: String,
    },
    /// One tool call.
    Tool(ToolCallItem),
    /// A run of reads and searches, collapsed.
    ToolGroup(Vec<ToolCallItem>),
    /// The agent's plan, kept current within its turn.
    Plan {
        /// The entries, in order.
        entries: Vec<PlanEntry>,
    },
    /// A permission request.
    Permission(PermissionItem),
    /// A turn ended.
    TurnEnded(TurnEnd),
    /// A provider limit was hit.
    Limit {
        /// The provider, for example `claude`.
        provider: String,
        /// The provider's message.
        message: String,
        /// When it resets, if known.
        resets_at: Option<Timestamp>,
    },
    /// A one-line notice: a new model or mode, the session stopping or failing.
    Notice {
        /// The text.
        text: String,
        /// How it looks.
        tone: Tone,
    },
    /// An event this version doesn't know.
    Unsupported {
        /// The event's `type`.
        type_name: String,
    },
}

/// What changed when an event was applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Row `ix` was added at the end.
    Added(usize),
    /// Text was appended to row `ix`, an [`Item::Agent`] or [`Item::Thought`].
    Appended(usize),
    /// Row `ix` changed in place.
    Updated(usize),
}

impl Change {
    /// The row that changed.
    pub fn ix(&self) -> usize {
        match self {
            Change::Added(ix) | Change::Appended(ix) | Change::Updated(ix) => *ix,
        }
    }
}

/// Where a tool call's row is.
#[derive(Clone, Copy, Debug)]
struct ToolRef {
    row: usize,
    /// Its place in an [`Item::ToolGroup`].
    in_group: Option<usize>,
}

/// The turn that is running.
#[derive(Debug)]
struct Turn {
    id: String,
    started_at: Timestamp,
    /// Its plan's row, once it has one.
    plan: Option<usize>,
    usage: Option<TurnUsage>,
}

/// The transcript's rows, built from session events.
#[derive(Debug, Default)]
pub struct TranscriptModel {
    items: Vec<Item>,
    tools: HashMap<String, ToolRef>,
    permissions: HashMap<String, usize>,
    turn: Option<Turn>,
    /// The rows of ended turns, by turn id, for usage that arrives after the end.
    turn_ends: HashMap<String, usize>,
    /// Permission rows that expired while their turn ran: [`Expiry::Pending`] until the turn
    /// carries on or ends.
    expiring: Vec<usize>,
}

impl TranscriptModel {
    /// An empty transcript.
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows, oldest first.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The number of rows.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether there are no rows.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether a turn is running, as far as the events say.
    pub fn in_turn(&self) -> bool {
        self.turn.is_some()
    }

    /// The row a tool call is shown in, alone or in a group.
    pub fn row_of_tool(&self, tool_call_id: &str) -> Option<usize> {
        self.tools.get(tool_call_id).map(|found| found.row)
    }

    /// Applies events in seq order, recording what changed.
    pub fn apply_all<'a>(
        &mut self,
        events: impl IntoIterator<Item = &'a StreamEvent>,
        changes: &mut Vec<Change>,
    ) {
        for event in events {
            self.apply(event, changes);
        }
    }

    /// Applies one event, recording what changed.
    pub fn apply(&mut self, event: &StreamEvent, changes: &mut Vec<Change>) {
        match &event.event {
            Event::UserMessage(message) => {
                self.turn = Some(Turn {
                    id: message.turn_id.clone(),
                    started_at: event.at,
                    plan: None,
                    usage: None,
                });
                let text = message
                    .content
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { text } => text.clone(),
                        ContentBlock::Image { .. } => "[image]".to_owned(),
                        ContentBlock::Resource { uri } => format!("<{uri}>"),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.push(
                    Item::User {
                        turn_id: message.turn_id.clone(),
                        text,
                    },
                    changes,
                );
            }
            Event::AgentMessageChunk(chunk) => {
                self.turn_carries_on(changes);
                self.append_text(false, &chunk.turn_id, &chunk.text, changes);
            }
            Event::AgentThoughtChunk(chunk) => {
                self.turn_carries_on(changes);
                self.append_text(true, &chunk.turn_id, &chunk.text, changes);
            }
            Event::ToolCall(call) => {
                if let Some(tool) = self.tool_mut(&call.tool_call_id) {
                    // Announced again: the new announcement replaces the old one.
                    tool.title = call.title.clone();
                    tool.kind = call.kind.clone();
                    tool.status = call.status.clone();
                    tool.locations = call.locations.clone();
                    tool.content = call.content.clone();
                    tool.revision += 1;
                    let row = self.tools[&call.tool_call_id].row;
                    changes.push(Change::Updated(row));
                } else {
                    self.turn_carries_on(changes);
                    self.add_tool(
                        ToolCallItem {
                            tool_call_id: call.tool_call_id.clone(),
                            title: call.title.clone(),
                            kind: call.kind.clone(),
                            status: call.status.clone(),
                            locations: call.locations.clone(),
                            content: call.content.clone(),
                            revision: 0,
                        },
                        changes,
                    );
                }
            }
            Event::ToolCallUpdate(update) => {
                if let Some(tool) = self.tool_mut(&update.tool_call_id) {
                    if let Some(title) = &update.title {
                        tool.title = title.clone();
                    }
                    if let Some(kind) = &update.kind {
                        tool.kind = kind.clone();
                    }
                    if let Some(status) = &update.status {
                        tool.status = status.clone();
                    }
                    if let Some(locations) = &update.locations {
                        tool.locations = locations.clone();
                    }
                    if let Some(content) = &update.content {
                        tool.content = content.clone();
                    }
                    tool.revision += 1;
                    let row = self.tools[&update.tool_call_id].row;
                    changes.push(Change::Updated(row));
                }
            }
            Event::Plan(plan) => {
                self.turn_carries_on(changes);
                let row = self.turn.as_ref().and_then(|turn| turn.plan);
                if let Some(row) = row
                    && let Some(Item::Plan { entries }) = self.items.get_mut(row)
                {
                    *entries = plan.entries.clone();
                    changes.push(Change::Updated(row));
                } else {
                    let row = self.push(
                        Item::Plan {
                            entries: plan.entries.clone(),
                        },
                        changes,
                    );
                    if let Some(turn) = &mut self.turn {
                        turn.plan = Some(row);
                    }
                }
            }
            Event::PermissionRequested(request) => {
                self.turn_carries_on(changes);
                let row = self.push(
                    Item::Permission(PermissionItem {
                        inbox_item_id: request.inbox_item_id.clone(),
                        tool_call_id: request.tool_call_id.clone(),
                        title: request.title.clone(),
                        options: request.options.clone(),
                        outcome: PermissionOutcome::Open,
                    }),
                    changes,
                );
                self.permissions.insert(request.inbox_item_id.clone(), row);
            }
            Event::PermissionResolved(resolved) => {
                let Some(&row) = self.permissions.get(&resolved.inbox_item_id) else {
                    return;
                };
                let outcome = match (&resolved.status, &resolved.option_id) {
                    (InboxItemStatus::Resolved, Some(option)) => PermissionOutcome::Chosen {
                        option: option.clone(),
                        by: resolved.by.clone(),
                    },
                    _ if self.turn.is_some() => {
                        self.expiring.push(row);
                        PermissionOutcome::Expired(Expiry::Pending)
                    }
                    _ => PermissionOutcome::Expired(Expiry::NoTurn),
                };
                if let Some(Item::Permission(item)) = self.items.get_mut(row) {
                    item.outcome = outcome;
                    changes.push(Change::Updated(row));
                }
            }
            Event::TurnEnded(ended) => {
                // Requests that expired while the turn ran: the turn ending first says why.
                let expiry = match ended.reason {
                    TurnEndReason::Completed => Expiry::Withdrawn,
                    _ => Expiry::TurnEnded {
                        reason: ended.reason.clone(),
                        message: ended.message.clone(),
                    },
                };
                self.settle_expiring(expiry, changes);
                let turn = self.turn.take().filter(|turn| turn.id == ended.turn_id);
                let duration_ms = turn
                    .as_ref()
                    .map(|turn| event.at.as_unix_millis() - turn.started_at.as_unix_millis());
                let row = self.push(
                    Item::TurnEnded(TurnEnd {
                        turn_id: ended.turn_id.clone(),
                        reason: ended.reason.clone(),
                        message: ended.message.clone(),
                        duration_ms,
                        usage: turn.and_then(|turn| turn.usage),
                    }),
                    changes,
                );
                self.turn_ends.insert(ended.turn_id.clone(), row);
            }
            Event::Usage(usage) => {
                let running = self.turn.as_mut().filter(|turn| {
                    usage
                        .turn_id
                        .as_ref()
                        .is_none_or(|turn_id| *turn_id == turn.id)
                });
                if let Some(turn) = running {
                    turn.usage.get_or_insert_with(TurnUsage::default).add(usage);
                } else if let Some(turn_id) = &usage.turn_id
                    && let Some(&row) = self.turn_ends.get(turn_id)
                    && let Some(Item::TurnEnded(end)) = self.items.get_mut(row)
                {
                    end.usage.get_or_insert_with(TurnUsage::default).add(usage);
                    changes.push(Change::Updated(row));
                }
            }
            Event::LimitReached(limit) => {
                self.push(
                    Item::Limit {
                        provider: limit.provider.clone(),
                        message: limit.message.clone(),
                        resets_at: limit.resets_at,
                    },
                    changes,
                );
            }
            Event::StateChanged(changed) => {
                let reason = changed
                    .reason
                    .as_ref()
                    .map(|reason| format!(": {reason}"))
                    .unwrap_or_default();
                let notice = match &changed.state {
                    SessionState::Stopped => {
                        Some((format!("Session stopped{reason}"), Tone::Quiet))
                    }
                    SessionState::Failed => Some((format!("Session failed{reason}"), Tone::Error)),
                    _ => None,
                };
                if let Some((text, tone)) = notice {
                    self.push(Item::Notice { text, tone }, changes);
                }
            }
            Event::ModelChanged(changed) => {
                self.push(
                    Item::Notice {
                        text: format!("Model: {}", changed.model),
                        tone: Tone::Quiet,
                    },
                    changes,
                );
            }
            Event::AgentModeChanged(changed) => {
                self.push(
                    Item::Notice {
                        text: format!("Mode: {}", changed.agent_mode),
                        tone: Tone::Quiet,
                    },
                    changes,
                );
            }
            Event::Unknown(unknown) => {
                self.push(
                    Item::Unsupported {
                        type_name: unknown.type_name().to_owned(),
                    },
                    changes,
                );
            }
            // Index events never arrive on a session stream; terminals come with M4.
            _ => {}
        }
    }

    fn push(&mut self, item: Item, changes: &mut Vec<Change>) -> usize {
        self.items.push(item);
        let row = self.items.len() - 1;
        changes.push(Change::Added(row));
        row
    }

    /// Appends message or thought text to the last row when it is the same kind and turn, or
    /// starts a new row.
    fn append_text(&mut self, thought: bool, turn: &str, piece: &str, changes: &mut Vec<Change>) {
        let row = self.items.len().wrapping_sub(1);
        let grown = match self.items.last_mut() {
            Some(Item::Agent { turn_id, text }) if !thought && turn_id == turn => Some(text),
            Some(Item::Thought { turn_id, text }) if thought && turn_id == turn => Some(text),
            _ => None,
        };
        if let Some(text) = grown {
            text.push_str(piece);
            changes.push(Change::Appended(row));
            return;
        }
        let (turn_id, text) = (turn.to_owned(), piece.to_owned());
        let item = if thought {
            Item::Thought { turn_id, text }
        } else {
            Item::Agent { turn_id, text }
        };
        self.push(item, changes);
    }

    /// Adds a new tool call: to the run of reads and searches it continues, or as its own row.
    fn add_tool(&mut self, tool: ToolCallItem, changes: &mut Vec<Change>) {
        let id = tool.tool_call_id.clone();
        let last = self.items.len().wrapping_sub(1);
        if tool.is_groupable() {
            if let Some(Item::ToolGroup(group)) = self.items.last_mut() {
                group.push(tool);
                let in_group = Some(group.len() - 1);
                self.tools.insert(
                    id,
                    ToolRef {
                        row: last,
                        in_group,
                    },
                );
                changes.push(Change::Updated(last));
                return;
            }
            if let Some(Item::Tool(previous)) = self.items.last()
                && previous.is_groupable()
            {
                // The second of a run: the lone call's row becomes the group's.
                let Some(Item::Tool(previous)) = self.items.pop() else {
                    unreachable!("checked above");
                };
                for (place, call) in [&previous.tool_call_id, &id].into_iter().enumerate() {
                    let at = ToolRef {
                        row: last,
                        in_group: Some(place),
                    };
                    self.tools.insert(call.clone(), at);
                }
                self.items.push(Item::ToolGroup(vec![previous, tool]));
                changes.push(Change::Updated(last));
                return;
            }
        }
        let row = self.push(Item::Tool(tool), changes);
        self.tools.insert(
            id,
            ToolRef {
                row,
                in_group: None,
            },
        );
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCallItem> {
        let found = *self.tools.get(id)?;
        match (self.items.get_mut(found.row)?, found.in_group) {
            (Item::Tool(tool), None) => Some(tool),
            (Item::ToolGroup(group), Some(ix)) => group.get_mut(ix),
            _ => None,
        }
    }

    /// The turn produced something new, so requests that expired in it were withdrawn: a cancel
    /// would have ended it instead.
    fn turn_carries_on(&mut self, changes: &mut Vec<Change>) {
        if !self.expiring.is_empty() {
            self.settle_expiring(Expiry::Withdrawn, changes);
        }
    }

    fn settle_expiring(&mut self, expiry: Expiry, changes: &mut Vec<Change>) {
        for row in std::mem::take(&mut self.expiring) {
            if let Some(Item::Permission(item)) = self.items.get_mut(row) {
                item.outcome = PermissionOutcome::Expired(expiry.clone());
                changes.push(Change::Updated(row));
            }
        }
    }
}

#[cfg(test)]
mod tests;
