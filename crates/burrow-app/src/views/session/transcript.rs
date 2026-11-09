//! The conversation: a basic, non-virtualized transcript.
//!
//! **Interface.** `Transcript::new(store, state, window, cx)`. It builds its items from
//! [`SessionLog::events`] and keeps them current from [`SessionStoreEvent`]s: `Appended` adds to
//! them, `Reset` and `Loaded` rebuild. Inline permission buttons answer through
//! [`crate::views::inbox::respond`]. Nothing else reads this module, so it can be replaced
//! wholesale (virtualized list, tool call cards, diffs, plans).
//!
//! [`TranscriptModel`] turns events into [`Item`]s with no GPUI involved. Agent text is rendered
//! as markdown: each agent message has a [`TextViewState`] that only receives the new text, which
//! it parses incrementally in the background.
//!
//! [`SessionLog::events`]: crate::state::session::SessionLog::events

use std::collections::HashMap;

use burrow_protocol::{
    Event, InboxItemStatus, PermissionOption, PermissionOptionKind, PlanEntry, PlanEntryStatus,
    SessionState, StreamEvent, Timestamp, ToolCallStatus, ToolKind, TurnEndReason,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::text::{TextView, TextViewState};
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _};

use crate::state::AppState;
use crate::state::session::{SessionStore, SessionStoreEvent};

// ---- The model -----------------------------------------------------------------------------------

/// How a permission request ended, so far.
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionOutcome {
    /// Waiting for an answer.
    Open,
    /// Answered with this option.
    Chosen { option: String },
    /// Nobody answered: the turn ended or the session stopped.
    Expired,
}

/// One entry in the transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// The prompt that started a turn.
    User { text: String },
    /// The agent's answer, chunks joined.
    Agent { turn_id: String, text: String },
    /// The agent's visible reasoning, chunks joined.
    Thought { turn_id: String, text: String },
    /// A tool call, kept current by its updates.
    Tool {
        tool_call_id: String,
        title: String,
        kind: ToolKind,
        status: ToolCallStatus,
    },
    /// The agent's plan, as of this point.
    Plan { entries: Vec<PlanEntry> },
    /// A permission request.
    Permission {
        inbox_item_id: String,
        title: String,
        options: Vec<PermissionOption>,
        outcome: PermissionOutcome,
    },
    /// A turn ended.
    TurnEnded {
        reason: TurnEndReason,
        message: Option<String>,
    },
    /// A provider limit was hit.
    Limit {
        message: String,
        resets_at: Option<Timestamp>,
    },
    /// Something about the session itself: stopped, failed, a new model.
    Notice { text: String, error: bool },
    /// An event this version doesn't know.
    Unsupported { type_name: String },
}

/// What changed when events were applied.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Item `ix` was added.
    Added(usize),
    /// Text was appended to item `ix` (an agent message or thought).
    Appended { ix: usize, text: String },
    /// Item `ix` changed in place.
    Updated(usize),
}

/// The transcript's items, built from session events.
#[derive(Debug, Default)]
pub struct TranscriptModel {
    items: Vec<Item>,
    tools: HashMap<String, usize>,
    permissions: HashMap<String, usize>,
}

impl TranscriptModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// The items, oldest first.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    fn push(&mut self, item: Item, changes: &mut Vec<Change>) -> usize {
        self.items.push(item);
        let ix = self.items.len() - 1;
        changes.push(Change::Added(ix));
        ix
    }

    /// Applies one event, recording what changed.
    pub fn apply(&mut self, event: &StreamEvent, changes: &mut Vec<Change>) {
        match &event.event {
            Event::UserMessage(message) => {
                let text = message
                    .content
                    .iter()
                    .filter_map(|block| block.as_text())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.push(Item::User { text }, changes);
            }
            Event::AgentMessageChunk(chunk) => {
                let ix = self.items.len().wrapping_sub(1);
                if let Some(Item::Agent { turn_id, text }) = self.items.last_mut()
                    && *turn_id == chunk.turn_id
                {
                    text.push_str(&chunk.text);
                    changes.push(Change::Appended {
                        ix,
                        text: chunk.text.clone(),
                    });
                } else {
                    self.push(
                        Item::Agent {
                            turn_id: chunk.turn_id.clone(),
                            text: chunk.text.clone(),
                        },
                        changes,
                    );
                }
            }
            Event::AgentThoughtChunk(chunk) => {
                let ix = self.items.len().wrapping_sub(1);
                if let Some(Item::Thought { turn_id, text }) = self.items.last_mut()
                    && *turn_id == chunk.turn_id
                {
                    text.push_str(&chunk.text);
                    changes.push(Change::Appended {
                        ix,
                        text: chunk.text.clone(),
                    });
                } else {
                    self.push(
                        Item::Thought {
                            turn_id: chunk.turn_id.clone(),
                            text: chunk.text.clone(),
                        },
                        changes,
                    );
                }
            }
            Event::ToolCall(call) => {
                if let Some(&ix) = self.tools.get(&call.tool_call_id) {
                    if let Some(Item::Tool {
                        title,
                        kind,
                        status,
                        ..
                    }) = self.items.get_mut(ix)
                    {
                        *title = call.title.clone();
                        *kind = call.kind.clone();
                        *status = call.status.clone();
                        changes.push(Change::Updated(ix));
                    }
                } else {
                    let ix = self.push(
                        Item::Tool {
                            tool_call_id: call.tool_call_id.clone(),
                            title: call.title.clone(),
                            kind: call.kind.clone(),
                            status: call.status.clone(),
                        },
                        changes,
                    );
                    self.tools.insert(call.tool_call_id.clone(), ix);
                }
            }
            Event::ToolCallUpdate(update) => {
                if let Some(&ix) = self.tools.get(&update.tool_call_id)
                    && let Some(Item::Tool {
                        title,
                        kind,
                        status,
                        ..
                    }) = self.items.get_mut(ix)
                {
                    if let Some(new) = &update.title {
                        *title = new.clone();
                    }
                    if let Some(new) = &update.kind {
                        *kind = new.clone();
                    }
                    if let Some(new) = &update.status {
                        *status = new.clone();
                    }
                    changes.push(Change::Updated(ix));
                }
            }
            Event::Plan(plan) => {
                self.push(
                    Item::Plan {
                        entries: plan.entries.clone(),
                    },
                    changes,
                );
            }
            Event::PermissionRequested(request) => {
                let ix = self.push(
                    Item::Permission {
                        inbox_item_id: request.inbox_item_id.clone(),
                        title: request.title.clone(),
                        options: request.options.clone(),
                        outcome: PermissionOutcome::Open,
                    },
                    changes,
                );
                self.permissions.insert(request.inbox_item_id.clone(), ix);
            }
            Event::PermissionResolved(resolved) => {
                if let Some(&ix) = self.permissions.get(&resolved.inbox_item_id)
                    && let Some(Item::Permission { outcome, .. }) = self.items.get_mut(ix)
                {
                    *outcome = match (&resolved.status, &resolved.option_id) {
                        (InboxItemStatus::Resolved, Some(option)) => PermissionOutcome::Chosen {
                            option: option.clone(),
                        },
                        _ => PermissionOutcome::Expired,
                    };
                    changes.push(Change::Updated(ix));
                }
            }
            Event::TurnEnded(ended) => {
                self.push(
                    Item::TurnEnded {
                        reason: ended.reason.clone(),
                        message: ended.message.clone(),
                    },
                    changes,
                );
            }
            Event::LimitReached(limit) => {
                self.push(
                    Item::Limit {
                        message: limit.message.clone(),
                        resets_at: limit.resets_at,
                    },
                    changes,
                );
            }
            Event::StateChanged(changed) => {
                let notice = match &changed.state {
                    SessionState::Stopped => Some((
                        format!(
                            "Session stopped{}",
                            changed
                                .reason
                                .as_ref()
                                .map(|r| format!(": {r}"))
                                .unwrap_or_default()
                        ),
                        false,
                    )),
                    SessionState::Failed => Some((
                        format!(
                            "Session failed{}",
                            changed
                                .reason
                                .as_ref()
                                .map(|r| format!(": {r}"))
                                .unwrap_or_default()
                        ),
                        true,
                    )),
                    _ => None,
                };
                if let Some((text, error)) = notice {
                    self.push(Item::Notice { text, error }, changes);
                }
            }
            Event::ModelChanged(changed) => {
                self.push(
                    Item::Notice {
                        text: format!("Model: {}", changed.model),
                        error: false,
                    },
                    changes,
                );
            }
            Event::AgentModeChanged(changed) => {
                self.push(
                    Item::Notice {
                        text: format!("Agent mode: {}", changed.agent_mode),
                        error: false,
                    },
                    changes,
                );
            }
            Event::Usage(_) => {}
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
}

// ---- The view ------------------------------------------------------------------------------------

pub struct Transcript {
    store: Entity<SessionStore>,
    state: Entity<AppState>,
    model: TranscriptModel,
    /// The markdown state of each agent message, by item index.
    markdown: HashMap<usize, Entity<TextViewState>>,
    scroll: ScrollHandle,
    /// Whether to keep the newest item in view: true until the user scrolls up.
    follow: bool,
    _subscriptions: Vec<Subscription>,
}

impl Transcript {
    pub fn new(
        store: Entity<SessionStore>,
        state: Entity<AppState>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.subscribe(
                &store,
                |this, store, event: &SessionStoreEvent, cx| match event {
                    SessionStoreEvent::Appended(range) => {
                        let events = store.read(cx).log().events()[range.clone()].to_vec();
                        this.apply(&events, cx);
                    }
                    SessionStoreEvent::Reset | SessionStoreEvent::Loaded => this.rebuild(cx),
                },
            ),
            cx.observe(&state, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            store,
            state,
            model: TranscriptModel::new(),
            markdown: HashMap::new(),
            scroll: ScrollHandle::new(),
            follow: true,
            _subscriptions: subscriptions,
        };
        this.rebuild(cx);
        this
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.model = TranscriptModel::new();
        self.markdown.clear();
        let events = self.store.read(cx).log().events().to_vec();
        self.apply(&events, cx);
    }

    fn apply(&mut self, events: &[StreamEvent], cx: &mut Context<Self>) {
        let mut changes = Vec::new();
        for event in events {
            self.model.apply(event, &mut changes);
        }
        for change in changes {
            match change {
                Change::Added(ix) => {
                    if let Item::Agent { text, .. } = &self.model.items()[ix] {
                        let text = text.clone();
                        let state = cx.new(|cx| {
                            let mut state = TextViewState::markdown("", cx);
                            state.push_str(&text, cx);
                            state
                        });
                        self.markdown.insert(ix, state);
                    }
                }
                Change::Appended { ix, text } => {
                    if let Some(state) = self.markdown.get(&ix) {
                        state.update(cx, |state, cx| state.push_str(&text, cx));
                    }
                }
                Change::Updated(_) => {}
            }
        }
        if self.follow {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn render_item(&self, ix: usize, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        match item {
            Item::User { text } => div()
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(gpui::relative(0.85))
                        .px_4()
                        .py_2()
                        .rounded(theme.radius_lg)
                        .bg(theme.secondary)
                        .border_1()
                        .border_color(theme.border)
                        .child(text.clone()),
                )
                .into_any_element(),
            Item::Agent { .. } => match self.markdown.get(&ix) {
                Some(state) => div()
                    .w_full()
                    .child(TextView::new(state).selectable(true))
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Item::Thought { text, .. } => div()
                .pl_3()
                .border_l_2()
                .border_color(theme.border)
                .text_xs()
                .italic()
                .text_color(theme.muted_foreground)
                .child(text.trim().to_owned())
                .into_any_element(),
            Item::Tool {
                title,
                kind,
                status,
                ..
            } => {
                let (status_icon, status_color) = match status {
                    ToolCallStatus::Completed => (IconName::CircleCheck, theme.success),
                    ToolCallStatus::Failed => (IconName::CircleX, theme.danger),
                    ToolCallStatus::InProgress => (IconName::LoaderCircle, theme.warning),
                    _ => (IconName::LoaderCircle, theme.muted_foreground),
                };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py(px(6.))
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .text_xs()
                    .child(
                        Icon::new(tool_icon(kind))
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(title.clone()))
                    .child(Icon::new(status_icon).xsmall().text_color(status_color))
                    .into_any_element()
            }
            Item::Plan { entries } => div()
                .p_3()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child("PLAN"),
                )
                .children(entries.iter().map(|entry| {
                    let (mark, color) = match entry.status {
                        PlanEntryStatus::Completed => ("✓", theme.success),
                        PlanEntryStatus::InProgress => ("›", theme.warning),
                        _ => ("○", theme.muted_foreground),
                    };
                    div()
                        .flex()
                        .gap_2()
                        .child(div().w(px(12.)).text_color(color).child(mark))
                        .child(entry.content.clone())
                }))
                .into_any_element(),
            Item::Permission {
                inbox_item_id,
                title,
                options,
                outcome,
            } => {
                let mut row = div().flex().flex_wrap().gap_2();
                match outcome {
                    PermissionOutcome::Open => {
                        for option in options {
                            let state = self.state.clone();
                            let item_id = inbox_item_id.clone();
                            let action = option.option_id.clone();
                            row = row.child(
                                Button::new(ElementId::Name(
                                    format!("perm-{}-{}", inbox_item_id, option.option_id).into(),
                                ))
                                .small()
                                .label(option.name.clone())
                                .map(|b| match option.kind {
                                    PermissionOptionKind::AllowOnce => b.primary(),
                                    PermissionOptionKind::RejectOnce
                                    | PermissionOptionKind::RejectAlways => b.danger(),
                                    _ => b.outline(),
                                })
                                .on_click(move |_, window, cx| {
                                    crate::views::inbox::respond(
                                        &state,
                                        item_id.clone(),
                                        action.clone(),
                                        window,
                                        cx,
                                    )
                                }),
                            );
                        }
                    }
                    PermissionOutcome::Chosen { option } => {
                        let name = options
                            .iter()
                            .find(|o| &o.option_id == option)
                            .map(|o| o.name.clone())
                            .unwrap_or_else(|| option.clone());
                        row = row.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("Answered: {name}")),
                        );
                    }
                    PermissionOutcome::Expired => {
                        row = row.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Expired without an answer"),
                        );
                    }
                }
                let open = *outcome == PermissionOutcome::Open;
                div()
                    .p_3()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(if open { theme.primary } else { theme.border })
                    .when(open, |d| d.bg(theme.primary.opacity(0.08)))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .small()
                                    .text_color(if open {
                                        theme.primary
                                    } else {
                                        theme.muted_foreground
                                    }),
                            )
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(format!("Permission: {title}")),
                            ),
                    )
                    .child(row)
                    .into_any_element()
            }
            Item::TurnEnded { reason, message } => {
                let label = match reason {
                    TurnEndReason::Completed => None,
                    TurnEndReason::Cancelled => Some("Cancelled".to_owned()),
                    TurnEndReason::Limit => Some("Stopped at a usage limit".to_owned()),
                    TurnEndReason::Error => Some("Ended with an error".to_owned()),
                    TurnEndReason::Other(other) => Some(other.clone()),
                };
                let text = match (label, message) {
                    (Some(label), Some(message)) => Some(format!("{label}: {message}")),
                    (Some(label), None) => Some(label),
                    (None, Some(message)) => Some(message.clone()),
                    (None, None) => None,
                };
                let error = matches!(reason, TurnEndReason::Error);
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_xs()
                    .text_color(if error {
                        theme.danger
                    } else {
                        theme.muted_foreground.opacity(0.7)
                    })
                    .child(div().flex_1().h(px(1.)).bg(theme.border))
                    .when_some(text, |d, text| {
                        d.child(text)
                            .child(div().flex_1().h(px(1.)).bg(theme.border))
                    })
                    .into_any_element()
            }
            Item::Limit { message, resets_at } => div()
                .p_3()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.danger.opacity(0.6))
                .bg(theme.danger.opacity(0.08))
                .text_xs()
                .child(message.clone())
                .when_some(*resets_at, |d, at| {
                    d.child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(format!("Resets at {at}")),
                    )
                })
                .into_any_element(),
            Item::Notice { text, error } => div()
                .text_xs()
                .text_color(if *error {
                    theme.danger
                } else {
                    theme.muted_foreground
                })
                .child(text.clone())
                .into_any_element(),
            Item::Unsupported { type_name } => div()
                .text_xs()
                .text_color(theme.muted_foreground.opacity(0.6))
                .child(format!("Unsupported event: {type_name}"))
                .into_any_element(),
        }
    }
}

fn tool_icon(kind: &ToolKind) -> IconName {
    match kind {
        ToolKind::Read => IconName::FileText,
        ToolKind::Edit => IconName::Replace,
        ToolKind::Delete => IconName::Delete,
        ToolKind::Move => IconName::ArrowRight,
        ToolKind::Search => IconName::Search,
        ToolKind::Execute => IconName::SquareTerminal,
        ToolKind::Think => IconName::Bot,
        ToolKind::Fetch => IconName::Globe,
        ToolKind::SwitchMode => IconName::Settings2,
        ToolKind::Misc | ToolKind::Other(_) => IconName::Asterisk,
    }
}

impl Render for Transcript {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Follow new output only while the view is scrolled to the bottom.
        let offset = self.scroll.offset().y;
        let max = self.scroll.max_offset().y;
        self.follow = max <= px(0.) || (-offset) >= max - px(24.);

        let items = self.model.items().to_vec();
        let empty = items.is_empty();
        let mut column = div()
            .w_full()
            .max_w(px(820.))
            .mx_auto()
            .px_6()
            .py_5()
            .flex()
            .flex_col()
            .gap_4();
        for (ix, item) in items.iter().enumerate() {
            column = column.child(self.render_item(ix, item, cx));
        }
        if empty {
            column = column.child(
                div()
                    .pt_10()
                    .flex()
                    .justify_center()
                    .text_color(theme.muted_foreground)
                    .child("No messages yet. Say something below."),
            );
        }
        div()
            .id("transcript")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .vertical_scrollbar(&self.scroll)
            .child(column)
    }
}

#[cfg(test)]
mod tests {
    use burrow_protocol::{
        AgentMessageChunk, ContentBlock, PermissionRequested, PermissionResolved, StateChanged,
        StreamName, ToolCall, ToolCallUpdate, TurnEnded, UserMessage,
    };

    use super::*;

    fn ev(seq: u64, event: impl Into<Event>) -> StreamEvent {
        StreamEvent {
            stream: StreamName::session("ses_1"),
            seq,
            at: Timestamp::from_unix_millis(1_800_000_000_000).unwrap(),
            event: event.into(),
        }
    }

    fn chunk(turn: &str, text: &str) -> AgentMessageChunk {
        AgentMessageChunk {
            turn_id: turn.into(),
            text: text.into(),
        }
    }

    fn build(events: Vec<StreamEvent>) -> (TranscriptModel, Vec<Change>) {
        let mut model = TranscriptModel::new();
        let mut changes = Vec::new();
        for event in &events {
            model.apply(event, &mut changes);
        }
        (model, changes)
    }

    #[test]
    fn chunks_of_one_turn_join_into_one_message() {
        let (model, changes) = build(vec![
            ev(
                1,
                UserMessage {
                    turn_id: "t1".into(),
                    content: vec![ContentBlock::text("hello")],
                },
            ),
            ev(2, chunk("t1", "Echo: ")),
            ev(3, chunk("t1", "hello")),
            ev(
                4,
                TurnEnded {
                    turn_id: "t1".into(),
                    reason: TurnEndReason::Completed,
                    message: None,
                },
            ),
            ev(5, chunk("t2", "next")),
        ]);
        assert_eq!(
            model.items()[..2],
            [
                Item::User {
                    text: "hello".into()
                },
                Item::Agent {
                    turn_id: "t1".into(),
                    text: "Echo: hello".into()
                },
            ]
        );
        assert_eq!(model.items().len(), 4);
        assert!(changes.contains(&Change::Appended {
            ix: 1,
            text: "hello".into()
        }));
    }

    #[test]
    fn a_tool_call_between_chunks_splits_the_message() {
        let (model, _) = build(vec![
            ev(1, chunk("t1", "Let me look.")),
            ev(
                2,
                ToolCall {
                    turn_id: Some("t1".into()),
                    tool_call_id: "c1".into(),
                    title: "Read README.md".into(),
                    kind: ToolKind::Read,
                    status: ToolCallStatus::Pending,
                    locations: Vec::new(),
                    content: Vec::new(),
                    raw_input: None,
                },
            ),
            ev(
                3,
                ToolCallUpdate {
                    tool_call_id: "c1".into(),
                    status: Some(ToolCallStatus::Completed),
                    title: None,
                    kind: None,
                    locations: None,
                    content: None,
                },
            ),
            ev(4, chunk("t1", "Done.")),
        ]);
        assert_eq!(model.items().len(), 3);
        assert!(matches!(
            &model.items()[1],
            Item::Tool { status: ToolCallStatus::Completed, title, .. } if title == "Read README.md"
        ));
        assert!(matches!(&model.items()[2], Item::Agent { text, .. } if text == "Done."));
    }

    #[test]
    fn permission_requests_resolve_in_place() {
        let request = |id: &str| PermissionRequested {
            inbox_item_id: id.into(),
            turn_id: Some("t1".into()),
            tool_call_id: "c1".into(),
            title: "Run echo hi".into(),
            options: vec![PermissionOption {
                option_id: "allow".into(),
                name: "Allow".into(),
                kind: PermissionOptionKind::AllowOnce,
            }],
        };
        let (model, changes) = build(vec![
            ev(1, request("inb_1")),
            ev(2, request("inb_2")),
            ev(
                3,
                PermissionResolved {
                    inbox_item_id: "inb_1".into(),
                    tool_call_id: "c1".into(),
                    status: InboxItemStatus::Resolved,
                    option_id: Some("allow".into()),
                    by: Some("local".into()),
                },
            ),
            ev(
                4,
                PermissionResolved {
                    inbox_item_id: "inb_2".into(),
                    tool_call_id: "c1".into(),
                    status: InboxItemStatus::Expired,
                    option_id: None,
                    by: None,
                },
            ),
        ]);
        assert!(matches!(
            &model.items()[0],
            Item::Permission { outcome: PermissionOutcome::Chosen { option }, .. } if option == "allow"
        ));
        assert!(matches!(
            &model.items()[1],
            Item::Permission {
                outcome: PermissionOutcome::Expired,
                ..
            }
        ));
        assert_eq!(changes.last(), Some(&Change::Updated(1)));
    }

    #[test]
    fn stops_and_unknown_events_become_notices() {
        let unknown: Event = serde_json::from_str(r#"{"type":"fromTheFuture"}"#).unwrap();
        let (model, _) = build(vec![
            ev(
                1,
                StateChanged {
                    state: SessionState::Working,
                    reason: None,
                },
            ),
            ev(
                2,
                StateChanged {
                    state: SessionState::Stopped,
                    reason: Some("stopped by request".into()),
                },
            ),
            ev(3, unknown),
        ]);
        assert_eq!(
            model.items(),
            [
                Item::Notice {
                    text: "Session stopped: stopped by request".into(),
                    error: false
                },
                Item::Unsupported {
                    type_name: "fromTheFuture".into()
                },
            ]
        );
    }
}
