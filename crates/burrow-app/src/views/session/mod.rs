//! One open session: header, transcript and prompt box, over one [`SessionStore`].
//!
//! [`SessionView`] only composes. Each part is its own entity in its own file and reads the
//! store, so the transcript and the header can be rebuilt independently (see the layout in
//! [`crate::views`]):
//!
//! - [`header::SessionHeader`]: title, agent, model, state, usage, and the controls;
//! - [`transcript::Transcript`]: the conversation, built from the store's events;
//! - [`prompt::PromptBox`]: the prompt box; Enter sends, Shift+Enter adds a line.
//!
//! Esc ([`CancelTurn`]) stops the running turn when the focus is anywhere in the session and
//! nothing closer uses Esc.

pub mod header;
pub mod prompt;
pub mod transcript;

use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Window, div,
};

use self::header::SessionHeader;
use self::prompt::PromptBox;
use self::transcript::Transcript;
use crate::actions::{CancelTurn, SESSION_CONTEXT};
use crate::state::AppState;
use crate::state::session::SessionStore;

pub struct SessionView {
    session_id: String,
    state: Entity<AppState>,
    store: Entity<SessionStore>,
    header: Entity<SessionHeader>,
    transcript: Entity<Transcript>,
    prompt: Entity<PromptBox>,
}

impl SessionView {
    pub fn new(
        session_id: String,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = cx.new(|cx| SessionStore::new(session_id.clone(), cx));
        let header = cx.new(|cx| SessionHeader::new(store.clone(), state.clone(), window, cx));
        let transcript = cx.new(|cx| Transcript::new(store.clone(), state.clone(), window, cx));
        let prompt = cx.new(|cx| PromptBox::new(store.clone(), state.clone(), window, cx));
        Self {
            session_id,
            state,
            store,
            header,
            transcript,
            prompt,
        }
    }

    /// The session's id.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Focuses the prompt box.
    pub fn focus_prompt(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt
            .update(cx, |prompt, cx| prompt.focus(window, cx));
    }

    fn on_cancel(&mut self, _: &CancelTurn, _: &mut Window, cx: &mut Context<Self>) {
        if self.store.read(cx).log().is_busy() {
            self.state
                .read(cx)
                .cancel(self.session_id.clone(), cx)
                .detach();
        } else {
            cx.propagate();
        }
    }
}

impl Render for SessionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context(SESSION_CONTEXT)
            .on_action(cx.listener(Self::on_cancel))
            .size_full()
            .flex()
            .flex_col()
            .child(self.header.clone())
            .child(div().flex_1().min_h_0().child(self.transcript.clone()))
            .child(self.prompt.clone())
    }
}
