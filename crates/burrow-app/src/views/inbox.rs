//! The inbox panel: open inbox items, newest first, each with its action buttons.

use burrow_protocol::{InboxActionStyle, InboxItemKind};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, IconName, Sizable as _};

use super::workspace::Workspace;
use super::{notify_error, relative_time};
use crate::state::AppState;
use crate::state::index::InboxEntry;

/// The panel's width.
pub const WIDTH: f32 = 340.;

pub fn render(
    workspace: &mut Workspace,
    _window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let state = workspace.state().clone();
    let index = state.read(cx).index();
    let items: Vec<InboxEntry> = index.inbox().into_iter().cloned().collect();
    let count = items.len();

    let mut list = div()
        .id("inbox-list")
        .flex_1()
        .overflow_y_scroll()
        .p_3()
        .flex()
        .flex_col()
        .gap_3();
    if items.is_empty() {
        list = list.child(
            div()
                .pt_8()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .text_color(theme.muted_foreground)
                .child(gpui_component::Icon::new(IconName::Inbox).large())
                .child("Nothing needs you"),
        );
    }
    for entry in items {
        list = list.child(card(&entry, &state, cx));
    }

    div()
        .id("inbox")
        .w(px(WIDTH))
        .flex_shrink_0()
        .h_full()
        .flex()
        .flex_col()
        .bg(theme.sidebar)
        .border_l_1()
        .border_color(theme.sidebar_border)
        .child(
            div()
                .h(px(48.))
                .px_3()
                .flex()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(theme.sidebar_border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Inbox")
                        .when(count > 0, |this| {
                            this.child(
                                div()
                                    .px(px(6.))
                                    .rounded_full()
                                    .bg(theme.primary)
                                    .text_color(theme.primary_foreground)
                                    .text_xs()
                                    .child(count.to_string()),
                            )
                        }),
                )
                .child(
                    Button::new("close-inbox")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .tooltip("Close (Ctrl+I)")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_inbox(cx))),
                ),
        )
        .child(list)
}

fn kind_label(kind: &InboxItemKind) -> &str {
    match kind {
        InboxItemKind::Permission => "Permission",
        InboxItemKind::Approval => "Approval",
        InboxItemKind::Question => "Question",
        InboxItemKind::Choice => "Choice",
        InboxItemKind::ManualTask => "Task",
        InboxItemKind::Review => "Review",
        InboxItemKind::Escalation => "Escalation",
        InboxItemKind::Other(other) => other,
    }
}

fn card(
    entry: &InboxEntry,
    state: &gpui::Entity<AppState>,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let index = state.read(cx).index();
    let session = entry
        .source
        .session_id
        .as_deref()
        .and_then(|id| index.session(id));
    let source_label = session.map(|session| session.display_title());
    let source_id = entry.source.session_id.clone();

    let mut actions = div().flex().flex_wrap().gap_2();
    match &entry.item {
        Some(item) => {
            for action in &item.actions {
                let item_id = item.id.clone();
                let action_id = action.id.clone();
                let state = state.clone();
                let button =
                    Button::new(ElementId::Name(format!("{}-{}", item.id, action.id).into()))
                        .small()
                        .label(action.label.clone())
                        .map(|b| match action.style {
                            Some(InboxActionStyle::Primary) => b.primary(),
                            Some(InboxActionStyle::Danger) => b.danger(),
                            None => b.outline(),
                        })
                        .on_click(cx.listener(move |_, _, window, cx| {
                            respond(&state, item_id.clone(), action_id.clone(), window, cx)
                        }));
                actions = actions.child(button);
            }
        }
        None => {
            actions = actions.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Loading…"),
            )
        }
    }

    div()
        .id(ElementId::Name(format!("inbox-{}", entry.id).into()))
        .p_3()
        .rounded(theme.radius_lg)
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_xs()
                .child(
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(theme.radius)
                        .bg(theme.primary.opacity(0.15))
                        .text_color(theme.primary)
                        .child(kind_label(&entry.kind).to_owned()),
                )
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(relative_time(entry.created)),
                ),
        )
        .child(
            div()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(entry.title.clone()),
        )
        .when_some(source_label, |this, label| {
            let source_id = source_id.clone();
            this.child(
                div()
                    .id(ElementId::Name(format!("inbox-source-{}", entry.id).into()))
                    .text_xs()
                    .text_color(theme.link)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.link_hover))
                    .child(format!("in {label}"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(id) = &source_id {
                            this.open_session(id.clone(), window, cx);
                        }
                    })),
            )
        })
        .child(actions)
}

/// Answers an inbox item, showing a notification if it fails. Used by the panel and the
/// transcript's inline buttons.
pub fn respond(
    state: &gpui::Entity<AppState>,
    item_id: String,
    action: String,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let task = state.update(cx, |state, cx| state.respond(item_id, action, cx));
    window
        .spawn(cx, async move |cx| {
            if let Err(error) = task.await {
                let conflict = matches!(&error, burrow_client::CallError::Rpc(e) if e.code == burrow_protocol::code::CONFLICT);
                if !conflict {
                    let _ = cx.update(|window, cx| notify_error("Couldn't answer", &error, window, cx));
                } else {
                    let _ = cx.update(|window, cx| {
                        use gpui_component::WindowExt as _;
                        window.push_notification(
                            gpui_component::notification::Notification::info(
                                "Already answered elsewhere",
                            ),
                            cx,
                        )
                    });
                }
            }
        })
        .detach();
}
