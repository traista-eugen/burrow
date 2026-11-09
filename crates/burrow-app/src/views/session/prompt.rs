//! The prompt box: a multi-line text field that grows to six lines, then scrolls.
//!
//! - **Enter** sends, **Shift+Enter** adds a line.
//! - **Up** in an empty box recalls the session's earlier prompts, newest first, and **Down**
//!   goes back towards an empty box ([`History`]). Up and Down move the cursor as usual once
//!   the text is edited.
//! - **Disabled, with the reason,** while the session is starting, stopped or failed.
//! - **While a turn runs** (`working`, or `waiting` on a permission answer) the box stays
//!   editable, so the next prompt can be drafted, but sending is refused here with a message
//!   that says so, and the text is kept. A refusal from `burrowd` (another client started a
//!   turn, or a limit hasn't reset) shows the same way. The send button becomes a stop button.

use burrow_client::CallError;
use burrow_protocol::{SessionState, code};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::{ActiveTheme as _, Disableable as _, IconName};

use crate::actions::{HistoryNext, HistoryPrevious, PROMPT_CONTEXT};
use crate::state::session::SessionStore;
use crate::state::{AppState, describe_error};

pub struct PromptBox {
    store: Entity<SessionStore>,
    state: Entity<AppState>,
    input: Entity<TextareaState>,
    sending: bool,
    /// Why the last send didn't go through; cleared by editing, or when the session is ready.
    refused: Option<String>,
    history: History,
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
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder("Message the agent…")
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                    InputEvent::Change => {
                        let value = input.read(cx).value();
                        this.history.edited(&value);
                        if this.refused.take().is_some() {
                            cx.notify();
                        }
                    }
                    _ => {}
                },
            ),
            cx.observe(&store, |this, store, cx| {
                if this.refused.is_some() && store.read(cx).log().state() == SessionState::Idle {
                    this.refused = None;
                }
                cx.notify();
            }),
        ];
        Self {
            store,
            state,
            input,
            sending: false,
            refused: None,
            history: History::default(),
            _subscriptions: subscriptions,
        }
    }

    /// Focuses the text field.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_owned();
        if text.is_empty() || self.sending {
            return;
        }
        let state = self.store.read(cx).log().state();
        if let Some(refusal) = refusal(&state) {
            self.refused = Some(refusal.to_owned());
            cx.notify();
            return;
        }
        if !state.is_live() || state == SessionState::Starting {
            return;
        }
        let session_id = self.store.read(cx).id().to_owned();
        let call = self.state.read(cx).prompt(session_id, text.clone(), cx);
        self.sending = true;
        self.refused = None;
        self.history.reset();
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
                            set_text(input, text, window, cx);
                        }
                    });
                    this.refused = Some(refused_by_burrowd(&error));
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

    fn recall(&mut self, older: bool, window: &mut Window, cx: &mut Context<Self>) {
        let entries = History::entries(self.store.read(cx).log().prompts());
        let current = self.input.read(cx).value();
        let recalled = if older {
            self.history.previous(&entries, &current)
        } else {
            self.history.next(&entries, &current)
        };
        match recalled {
            Some(text) => self
                .input
                .update(cx, |input, cx| set_text(input, text, window, cx)),
            // Not browsing: the key moves the cursor as usual.
            None => cx.propagate(),
        }
    }

    fn on_history_previous(
        &mut self,
        _: &HistoryPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.recall(true, window, cx);
    }

    fn on_history_next(&mut self, _: &HistoryNext, window: &mut Window, cx: &mut Context<Self>) {
        self.recall(false, window, cx);
    }
}

/// Replaces the box's text, with the cursor at its end.
fn set_text(
    input: &mut TextareaState,
    text: String,
    window: &mut Window,
    cx: &mut Context<TextareaState>,
) {
    let end = text.len();
    input.set_value(text, window, cx);
    input.set_selected_range(end..end, cx);
}

/// Why a prompt can't be sent in `state` while the box is still usable, or `None`.
fn refusal(state: &SessionState) -> Option<&'static str> {
    match state {
        SessionState::Working => Some(
            "Not sent: the agent is still working on the last prompt. Wait for the turn to end, \
             or press Esc to stop it. Your text is kept.",
        ),
        SessionState::Waiting => Some(
            "Not sent: the agent is waiting for your answer to its request above. Your text is \
             kept.",
        ),
        _ => None,
    }
}

/// What to say when `burrowd` refused a prompt.
fn refused_by_burrowd(error: &CallError) -> String {
    match error {
        CallError::Rpc(error) if error.code == code::INVALID_STATE => format!(
            "Not sent: {}. Your text is kept.",
            error.message.trim_end_matches('.')
        ),
        CallError::Rpc(error) if error.code == code::LIMIT_REACHED => format!(
            "Not sent: the usage limit hasn't reset yet ({}). Your text is kept.",
            error.message.trim_end_matches('.')
        ),
        other => format!(
            "Couldn't send: {}. Your text is kept.",
            describe_error(other)
        ),
    }
}

/// The hint under the box, and whether the box is disabled.
fn hint(state: &SessionState, reason: Option<&str>) -> (String, bool) {
    match state {
        SessionState::Starting => (
            "The agent is starting; the box opens once it's ready.".into(),
            true,
        ),
        SessionState::Stopped => (
            match reason {
                Some(reason) => {
                    format!("This session has stopped ({reason}). Start a new session to carry on.")
                }
                None => "This session has stopped. Start a new session to carry on.".into(),
            },
            true,
        ),
        SessionState::Failed => (
            match reason {
                Some(reason) => format!("The agent failed: {reason}"),
                None => "The agent failed. Start a new session to carry on.".into(),
            },
            true,
        ),
        SessionState::Working => (
            "The agent is working · Esc stops it · you can draft the next prompt meanwhile".into(),
            false,
        ),
        SessionState::Waiting => (
            "The agent waits for your answer to its request above".into(),
            false,
        ),
        SessionState::Limited => (
            "A usage limit was hit · sending tries again, and says if it hasn't reset".into(),
            false,
        ),
        _ => (
            "Enter sends · Shift+Enter adds a line · ↑ recalls earlier prompts · Ctrl+L focuses"
                .into(),
            false,
        ),
    }
}

/// Walking through earlier prompts with Up and Down. No GPUI in here.
#[derive(Debug, Default)]
struct History {
    /// The entry shown, while browsing.
    position: Option<usize>,
    /// Its text, to tell whether it was edited since.
    shown: Option<String>,
}

impl History {
    /// The prompts to walk through, oldest first, without repeats in a row.
    fn entries(prompts: impl Iterator<Item = String>) -> Vec<String> {
        let mut entries: Vec<String> = Vec::new();
        for prompt in prompts {
            if entries.last() != Some(&prompt) {
                entries.push(prompt);
            }
        }
        entries
    }

    fn browsing(&self, current: &str) -> Option<usize> {
        self.position
            .filter(|_| self.shown.as_deref() == Some(current))
    }

    /// Up: the prompt before the one shown, or the newest when the box is empty. `None` when
    /// the box holds the user's own text, or the oldest prompt is already shown.
    fn previous(&mut self, entries: &[String], current: &str) -> Option<String> {
        let position = match self.browsing(current) {
            Some(0) => return None,
            Some(position) => position - 1,
            None if current.is_empty() && !entries.is_empty() => entries.len() - 1,
            None => return None,
        };
        self.show(entries, position)
    }

    /// Down: the prompt after the one shown, or an empty box after the newest. `None` when not
    /// browsing.
    fn next(&mut self, entries: &[String], current: &str) -> Option<String> {
        let position = self.browsing(current)?;
        if position + 1 < entries.len() {
            self.show(entries, position + 1)
        } else {
            self.reset();
            Some(String::new())
        }
    }

    fn show(&mut self, entries: &[String], position: usize) -> Option<String> {
        let text = entries.get(position)?.clone();
        self.position = Some(position);
        self.shown = Some(text.clone());
        Some(text)
    }

    /// The text changed: once it isn't the recalled prompt any more, Up and Down move the
    /// cursor again.
    fn edited(&mut self, current: &str) {
        if self.shown.as_deref() != Some(current) {
            self.reset();
        }
    }

    fn reset(&mut self) {
        self.position = None;
        self.shown = None;
    }
}

impl Render for PromptBox {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let log = self.store.read(cx).log();
        let state = log.state();
        let busy = log.is_busy();
        let reason = log.session().and_then(|s| s.reason.clone());
        let (hint, disabled) = hint(&state, reason.as_deref());
        let can_send = matches!(state, SessionState::Idle | SessionState::Limited);
        let refused = self.refused.clone();

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
                                .key_context(PROMPT_CONTEXT)
                                .on_action(cx.listener(Self::on_history_previous))
                                .on_action(cx.listener(Self::on_history_next))
                                .child(Textarea::new(&self.input).disabled(disabled)),
                        )
                        .map(|row| {
                            if busy {
                                row.child(
                                    Button::new("stop")
                                        .icon(IconName::Square)
                                        .tooltip("Stop the turn (Esc)")
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
                                        .disabled(disabled || !can_send)
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.send(window, cx)
                                            }),
                                        ),
                                )
                            }
                        }),
                )
                .map(|col| match refused {
                    Some(refused) => col.child(
                        div()
                            .id("prompt-refused")
                            .px_1()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(theme.warning)
                            .child(gpui_component::Icon::new(IconName::TriangleAlert).size(px(12.)))
                            .child(refused),
                    ),
                    None => col.child(
                        div()
                            .px_1()
                            .text_xs()
                            .text_color(if disabled && state == SessionState::Failed {
                                theme.danger.opacity(0.9)
                            } else {
                                theme.muted_foreground.opacity(0.8)
                            })
                            .child(hint),
                    ),
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<String> {
        History::entries(
            ["one", "two", "two", "three"]
                .into_iter()
                .map(str::to_owned),
        )
    }

    #[test]
    fn up_in_an_empty_box_walks_back_and_down_returns() {
        let entries = entries();
        assert_eq!(entries, ["one", "two", "three"]);
        let mut history = History::default();
        assert_eq!(history.previous(&entries, "").as_deref(), Some("three"));
        assert_eq!(history.previous(&entries, "three").as_deref(), Some("two"));
        assert_eq!(history.previous(&entries, "two").as_deref(), Some("one"));
        // The oldest: Up moves the cursor again.
        assert_eq!(history.previous(&entries, "one"), None);
        assert_eq!(history.next(&entries, "one").as_deref(), Some("two"));
        assert_eq!(history.next(&entries, "two").as_deref(), Some("three"));
        // Past the newest: an empty box, and Down is the cursor's again.
        assert_eq!(history.next(&entries, "three").as_deref(), Some(""));
        assert_eq!(history.next(&entries, ""), None);
    }

    #[test]
    fn the_users_own_text_is_never_replaced() {
        let entries = entries();
        let mut history = History::default();
        assert_eq!(history.previous(&entries, "draft"), None);
        assert_eq!(history.next(&entries, "draft"), None);
        // Recall, then edit: the edit is the user's text now.
        assert_eq!(history.previous(&entries, "").as_deref(), Some("three"));
        history.edited("three!");
        assert_eq!(history.previous(&entries, "three!"), None);
        // Recalling into the box itself doesn't count as an edit.
        let mut history = History::default();
        history.previous(&entries, "");
        history.edited("three");
        assert_eq!(history.previous(&entries, "three").as_deref(), Some("two"));
        // No prompts yet: nothing to recall.
        assert_eq!(History::default().previous(&[], ""), None);
    }

    #[test]
    fn prompts_during_a_turn_are_refused_with_a_reason() {
        assert!(
            refusal(&SessionState::Working)
                .unwrap()
                .contains("still working")
        );
        assert!(
            refusal(&SessionState::Waiting)
                .unwrap()
                .contains("your answer")
        );
        assert_eq!(refusal(&SessionState::Idle), None);
        assert_eq!(refusal(&SessionState::Limited), None);

        let (text, disabled) = hint(&SessionState::Stopped, Some("stopped by request"));
        assert!(disabled);
        assert!(text.contains("stopped by request"), "{text}");
        assert!(hint(&SessionState::Starting, None).1);
        assert!(!hint(&SessionState::Working, None).1);
        assert!(!hint(&SessionState::Idle, None).1);

        let error = CallError::Rpc(burrow_protocol::ErrorObject::new(
            burrow_protocol::ErrorKind::InvalidState,
            "a turn is already running.",
        ));
        assert_eq!(
            refused_by_burrowd(&error),
            "Not sent: a turn is already running. Your text is kept."
        );
    }
}
