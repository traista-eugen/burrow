//! The main area when nothing is open, and the app's logo.

use std::sync::{Arc, LazyLock};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Image, ImageFormat, IntoElement, ParentElement as _, Pixels, Styled as _, Window, div,
    img, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::kbd::Kbd;
use gpui_component::{ActiveTheme as _, IconName};

use super::workspace::Workspace;
use crate::backend::ConnectionStatus;

static LOGO: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        include_bytes!("../../../../assets/icon/burrow.svg").to_vec(),
    ))
});

/// The dachshund, at `size`.
pub fn logo(size: Pixels) -> impl IntoElement {
    img(LOGO.clone()).size(size).flex_shrink_0()
}

pub fn render(
    workspace: &mut Workspace,
    _window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let state = workspace.state().read(cx);
    let connected = state.connection().is_connected();
    let has_sessions = !state.index().sessions(false).is_empty();
    let inbox = state.index().inbox_count();
    let detail = match state.connection() {
        ConnectionStatus::Connected { .. } if has_sessions => {
            "Pick a session in the sidebar, or start a new one.".to_owned()
        }
        ConnectionStatus::Connected { .. } => "Start a session with one of your agents.".to_owned(),
        ConnectionStatus::StartingDaemon => "Starting burrowd…".to_owned(),
        ConnectionStatus::Connecting { .. } => "Connecting to burrowd…".to_owned(),
        ConnectionStatus::NotRunning { detail } | ConnectionStatus::Failed { detail } => {
            detail.clone()
        }
    };
    let shortcut = |keys: &'static str, label: &'static str| {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_6()
            .text_color(theme.muted_foreground)
            .child(label)
            .child(Kbd::new(gpui::Keystroke::parse(keys).unwrap()))
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_5()
        .child(logo(px(96.)))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Burrow"),
                )
                .child(div().text_color(theme.muted_foreground).child(detail)),
        )
        .when(connected, |this| {
            this.child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("empty-new-session")
                            .primary()
                            .icon(IconName::Plus)
                            .label("New session")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_new_session(None, window, cx)
                            })),
                    )
                    .child(
                        Button::new("empty-add-project")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Add project")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_project(window, cx)),
                            ),
                    )
                    .when(inbox > 0, |this| {
                        this.child(
                            Button::new("empty-inbox")
                                .outline()
                                .icon(IconName::Inbox)
                                .label(format!("Inbox ({inbox})"))
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_inbox(cx))),
                        )
                    }),
            )
        })
        .child(
            div()
                .mt_4()
                .w(px(260.))
                .flex()
                .flex_col()
                .gap_2()
                .text_xs()
                .child(shortcut("ctrl-n", "New session"))
                .child(shortcut("ctrl-l", "Focus the prompt"))
                .child(shortcut("ctrl-i", "Toggle the inbox"))
                .child(shortcut("ctrl-o", "Add a project")),
        )
        .text_sm()
}
