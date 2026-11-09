//! The prompt box: a multi-line text field. Enter sends, Shift+Enter adds a line. While a turn
//! runs, the send button becomes a stop button.

use burrow_protocol::SessionState;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::{ActiveTheme as _, Disableable as _, IconName};

use crate::state::AppState;
use crate::state::session::SessionStore;
use crate::views::notify_error;

pub struct PromptBox {
    store: Entity<SessionStore>,
    state: Entity<AppState>,
    input: Entity<TextareaState>,
    sending: bool,
    _subscriptions: Vec<Subscription>,
}

impl PromptBox {
    pub fn new(
        store: Entity<SessionStore>,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .submit_on_enter(true)
                .placeholder("Message the agent…")
        });
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send(window, cx);
                }
            }),
            cx.observe(&store, |_, _, cx| cx.notify()),
        ];
        Self {
            store,
            state,
            input,
            sending: false,
            _subscriptions: subscriptions,
        }
    }

    /// Focuses the text field.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn accepts_prompts(&self, cx: &Context<Self>) -> bool {
        matches!(
            self.store.read(cx).log().state(),
            SessionState::Idle | SessionState::Limited
        )
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_owned();
        if text.is_empty() || self.sending || !self.accepts_prompts(cx) {
            return;
        }
        let session_id = self.store.read(cx).id().to_owned();
        let call = self.state.read(cx).prompt(session_id, text.clone(), cx);
        self.sending = true;
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.sending = false;
                if let Err(error) = result {
                    // Give the text back so nothing is lost.
                    this.input.update(cx, |input, cx| {
                        if input.value().is_empty() {
                            input.set_value(text, window, cx);
                        }
                    });
                    notify_error("Couldn't send", &error, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        let session_id = self.store.read(cx).id().to_owned();
        self.state.read(cx).cancel(session_id, cx).detach();
    }
}

impl Render for PromptBox {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let log = self.store.read(cx).log();
        let state = log.state();
        let busy = log.is_busy();
        let ended = !state.is_live();
        let hint = match state {
            SessionState::Starting => "The agent is starting…",
            SessionState::Working => "The agent is working. Esc stops it.",
            SessionState::Waiting => "The agent is waiting for your answer above.",
            SessionState::Limited => "A usage limit was hit; prompting tries again.",
            SessionState::Stopped | SessionState::Failed => "This session has ended.",
            _ => "Enter sends · Shift+Enter adds a line · Ctrl+L focuses",
        };
        div().flex_shrink_0().px_6().pt_2().pb_4().child(
            div()
                .max_w(px(820.))
                .mx_auto()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_end()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .child(Textarea::new(&self.input).disabled(ended)),
                        )
                        .map(|row| {
                            if busy {
                                row.child(
                                    Button::new("stop")
                                        .icon(IconName::Square)
                                        .tooltip("Stop (Esc)")
                                        .outline()
                                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                                )
                            } else {
                                row.child(
                                    Button::new("send")
                                        .primary()
                                        .icon(IconName::ArrowUp)
                                        .tooltip("Send (Enter)")
                                        .loading(self.sending)
                                        .disabled(ended || !self.accepts_prompts(cx))
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.send(window, cx)
                                            }),
                                        ),
                                )
                            }
                        }),
                )
                .child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(theme.muted_foreground.opacity(0.8))
                        .child(hint),
                ),
        )
    }
}
