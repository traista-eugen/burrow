//! One open session: header, then either a chat (transcript and prompt box) or a terminal, over
//! one [`SessionStore`].
//!
//! [`SessionView`] only composes. Each part is its own entity in its own file and reads the
//! store, so the transcript and the header can be rebuilt independently (see the layout in
//! [`crate::views`]):
//!
//! - [`header::SessionHeader`]: title, agent, model, state, usage, and the controls;
//! - for a structured session, [`transcript::Transcript`], the conversation built from the
//!   store's events, and [`prompt::PromptBox`], the prompt box (Enter sends, Shift+Enter adds a
//!   line);
//! - for a terminal session, [`terminal::SessionTerminal`]: the agent's own TUI.
//!
//! Which one shows is known once `session/get` answers with the session's `mode`; until then
//! the body is empty.
//!
//! Esc ([`CancelTurn`]) stops the running turn when the focus is anywhere in the session and
//! nothing closer uses Esc. A focused terminal keeps Esc for its program (D-041).

pub mod header;
pub mod prompt;
pub mod terminal;
pub mod transcript;

use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Window, div,
};

use self::header::SessionHeader;
use self::prompt::PromptBox;
use self::terminal::SessionTerminal;
use self::transcript::Transcript;
use crate::actions::{CancelTurn, SESSION_CONTEXT};
use crate::state::AppState;
use crate::state::session::{LoadStatus, SessionStore};

/// What shows under the header.
enum Body {
    /// Until `session/get` says the session's mode.
    Loading,
    /// A structured session.
    Chat {
        transcript: Entity<Transcript>,
        prompt: Entity<PromptBox>,
    },
    /// A terminal session.
    Terminal(Entity<SessionTerminal>),
}

pub struct SessionView {
    session_id: String,
    state: Entity<AppState>,
    store: Entity<SessionStore>,
    header: Entity<SessionHeader>,
    body: Body,
    /// Focus the body's input as soon as it exists.
    focus_pending: bool,
    /// This app started the session: its terminal started at this view's size, and keeps
    /// following it (D-040).
    started_here: bool,
    _subscriptions: Vec<Subscription>,
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
        let subscriptions = vec![cx.observe_in(&store, window, |this, _, window, cx| {
            this.make_body(window, cx)
        })];
        Self {
            session_id,
            state,
            store,
            header,
            body: Body::Loading,
            focus_pending: false,
            started_here: false,
            _subscriptions: subscriptions,
        }
    }

    /// Notes that this app just started the session, so its terminal keeps this view's size.
    pub fn started_here(&mut self) {
        self.started_here = true;
    }

    /// The session's id.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Whether the session has loaded and replayed its history.
    pub fn is_caught_up(&self, cx: &App) -> bool {
        self.store.read(cx).is_caught_up()
    }

    /// How many events the session holds.
    pub fn event_count(&self, cx: &App) -> usize {
        self.store.read(cx).log().events().len()
    }

    /// Focuses where the user types: the prompt box, or the terminal.
    pub fn focus_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.body {
            Body::Loading => self.focus_pending = true,
            Body::Chat { prompt, .. } => prompt.update(cx, |prompt, cx| prompt.focus(window, cx)),
            Body::Terminal(terminal) => {
                terminal.update(cx, |terminal, cx| terminal.focus(window, cx))
            }
        }
    }

    /// Makes the body once `session/get` says what the session is.
    fn make_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.body, Body::Loading) {
            return;
        }
        let store = self.store.read(cx);
        let terminal = match (store.log().is_terminal(), store.status()) {
            (Some(terminal), _) => terminal,
            // It won't load: the chat body shows the error.
            (None, LoadStatus::Failed(_)) => false,
            (None, _) => return,
        };
        let (store, state) = (self.store.clone(), self.state.clone());
        if terminal {
            let started_here = self.started_here;
            let terminal =
                cx.new(|cx| SessionTerminal::new(store, state, started_here, window, cx));
            self.header
                .update(cx, |header, cx| header.set_terminal(terminal.clone(), cx));
            self.body = Body::Terminal(terminal);
        } else {
            let transcript = cx.new(|cx| Transcript::new(store.clone(), state.clone(), window, cx));
            let prompt = cx.new(|cx| PromptBox::new(store, state, window, cx));
            self.body = Body::Chat { transcript, prompt };
        }
        if std::mem::take(&mut self.focus_pending) {
            self.focus_prompt(window, cx);
        }
        cx.notify();
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
        let view = div()
            .key_context(SESSION_CONTEXT)
            .on_action(cx.listener(Self::on_cancel))
            .size_full()
            .flex()
            .flex_col()
            .child(self.header.clone());
        match &self.body {
            Body::Loading => view.child(div().flex_1()),
            Body::Chat { transcript, prompt } => view
                .child(div().flex_1().min_h_0().child(transcript.clone()))
                .child(prompt.clone()),
            Body::Terminal(terminal) => {
                view.child(div().flex_1().min_h_0().child(terminal.clone()))
            }
        }
    }
}
