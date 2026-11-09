//! The window's root view: sidebar, main area and inbox panel, and the global actions.
//!
//! - **Recent sessions stay open.** The last [`KEEP_OPEN`] sessions shown keep their view, and
//!   with it their subscription and transcript, so switching back is instant. Older ones are
//!   dropped, which ends their subscription.
//! - **The clock.** Relative times ("5m") in the sidebar and the inbox are redrawn when the
//!   first of them would change ([`super::relative_time_changes_in`]): once a minute while
//!   something is less than an hour old, else hourly, and not at all when nothing shows a time.

use std::time::{Duration, Instant};

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, Styled as _, Subscription, Task,
    Window, div, px,
};
use gpui_component::ActiveTheme as _;

use super::new_session::{NewSessionEvent, NewSessionView};
use super::session::SessionView;
use super::{empty, inbox, notify_error, now_millis, relative_time_changes_in, sidebar};
use crate::actions::{
    AddProject, FocusPrompt, NewSession, NextSession, PreviousSession, ToggleInbox,
};
use crate::state::{AppState, AppStateEvent};

/// How many recently shown sessions keep their view.
pub const KEEP_OPEN: usize = 5;

/// What the main area shows.
pub enum Main {
    /// Nothing selected.
    Empty,
    /// The new session page.
    NewSession(Entity<NewSessionView>),
    /// An open session.
    Session(Entity<SessionView>),
}

/// The window's root view.
pub struct Workspace {
    state: Entity<AppState>,
    focus: FocusHandle,
    main: Main,
    /// Sessions shown recently, most recent first, at most [`KEEP_OPEN`].
    recent: Vec<Entity<SessionView>>,
    inbox_open: bool,
    show_archived: bool,
    /// When the clock next redraws, if it is set.
    clock_at: Option<Instant>,
    _clock: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&state, |this, _, cx| {
                this.schedule_clock(cx);
                cx.notify();
            }),
            cx.subscribe_in(
                &state,
                window,
                |this, _, event: &AppStateEvent, window, cx| match event {
                    AppStateEvent::Archived(id) => this.archived_here(id, window, cx),
                },
            ),
        ];
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            state,
            focus,
            main: Main::Empty,
            recent: Vec::new(),
            inbox_open: false,
            show_archived: false,
            clock_at: None,
            _clock: None,
            _subscriptions: subscriptions,
        }
    }

    /// Whether archived sessions show in the sidebar.
    pub fn show_archived(&self) -> bool {
        self.show_archived
    }

    /// Shows or hides archived sessions.
    pub fn toggle_archived(&mut self, cx: &mut Context<Self>) {
        self.show_archived = !self.show_archived;
        self.schedule_clock(cx);
        cx.notify();
    }

    /// The sessions with a view, most recently shown first.
    pub fn open_sessions(&self) -> &[Entity<SessionView>] {
        &self.recent
    }

    /// A session archived from this app leaves the main area, unless archived sessions show.
    fn archived_here(&mut self, session_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_archived {
            return;
        }
        self.recent
            .retain(|view| view.read(cx).session_id() != session_id);
        if self.selected_session(cx).as_deref() == Some(session_id) {
            self.main = Main::Empty;
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    /// Redraws when the first relative time on screen would change.
    fn schedule_clock(&mut self, cx: &mut Context<Self>) {
        let now = now_millis();
        let index = self.state.read(cx).index();
        let sessions = index
            .sessions(self.show_archived)
            .into_iter()
            .filter_map(|session| session.activity);
        let inbox = index
            .inbox()
            .into_iter()
            .filter(|_| self.inbox_open)
            .map(|entry| entry.created);
        let Some(next) = sessions
            .chain(inbox)
            .map(|at| relative_time_changes_in(at, now))
            .min()
        else {
            return;
        };
        let at = Instant::now() + next;
        if self.clock_at.is_some_and(|set| set <= at) {
            return;
        }
        self.clock_at = Some(at);
        self._clock = Some(cx.spawn(async move |this, cx| {
            // A little late rather than early, so the label has changed when it is drawn.
            cx.background_executor()
                .timer(next + Duration::from_millis(50))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.clock_at = None;
                this.schedule_clock(cx);
                cx.notify();
            });
        }));
    }

    /// The shared state.
    pub fn state(&self) -> &Entity<AppState> {
        &self.state
    }

    /// The open session's id, if a session is open.
    pub fn selected_session(&self, cx: &App) -> Option<String> {
        match &self.main {
            Main::Session(view) => Some(view.read(cx).session_id().to_owned()),
            _ => None,
        }
    }

    /// Whether the new session page is showing.
    pub fn is_new_session_open(&self) -> bool {
        matches!(self.main, Main::NewSession(_))
    }

    /// Whether the inbox panel is open.
    pub fn is_inbox_open(&self) -> bool {
        self.inbox_open
    }

    /// Opens a session in the main area and focuses its prompt.
    pub fn open_session(
        &mut self,
        session_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_session(cx).as_deref() == Some(session_id.as_str()) {
            return;
        }
        let view = match self
            .recent
            .iter()
            .position(|view| view.read(cx).session_id() == session_id)
        {
            Some(ix) => self.recent.remove(ix),
            None => {
                let state = self.state.clone();
                cx.new(|cx| SessionView::new(session_id, state, window, cx))
            }
        };
        self.recent.insert(0, view.clone());
        self.recent.truncate(KEEP_OPEN);
        view.update(cx, |view, cx| view.focus_prompt(window, cx));
        self.main = Main::Session(view);
        cx.notify();
    }

    /// Shows the new session page, preselecting a project if given.
    pub fn open_new_session(
        &mut self,
        project_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        let view = cx.new(|cx| NewSessionView::new(state, project_id, window, cx));
        cx.subscribe_in(
            &view,
            window,
            |this, _, event: &NewSessionEvent, window, cx| match event {
                NewSessionEvent::Created(session_id) => {
                    this.open_session(session_id.clone(), window, cx)
                }
                NewSessionEvent::Dismissed => {
                    this.main = Main::Empty;
                    this.focus.focus(window, cx);
                    cx.notify();
                }
            },
        )
        .detach();
        view.update(cx, |view, cx| view.focus(window, cx));
        self.main = Main::NewSession(view);
        cx.notify();
    }

    /// Shows or hides the inbox panel.
    pub fn toggle_inbox(&mut self, cx: &mut Context<Self>) {
        self.inbox_open = !self.inbox_open;
        self.schedule_clock(cx);
        cx.notify();
    }

    /// Asks for a folder and adds it as a project.
    pub fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add project".into()),
        });
        let state = self.state.clone();
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let call = cx.update(|_, cx| {
                state
                    .read(cx)
                    .add_project(path.to_string_lossy().into_owned(), cx)
            });
            let Ok(call) = call else { return };
            if let Err(error) = call.await {
                let _ = cx.update(|window, cx| {
                    notify_error("Couldn't add the project", &error, window, cx)
                });
            }
        })
        .detach();
    }

    fn step_session(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ordered = sidebar::session_order(self.state.read(cx).index(), self.show_archived);
        if ordered.is_empty() {
            return;
        }
        let current = self
            .selected_session(cx)
            .and_then(|id| ordered.iter().position(|other| *other == id));
        let next = match current {
            Some(ix) => (ix as isize + delta).rem_euclid(ordered.len() as isize) as usize,
            None if delta > 0 => 0,
            None => ordered.len() - 1,
        };
        self.open_session(ordered[next].clone(), window, cx);
    }

    fn on_new_session(&mut self, _: &NewSession, window: &mut Window, cx: &mut Context<Self>) {
        let project = match &self.main {
            Main::Session(view) => self
                .state
                .read(cx)
                .index()
                .session(view.read(cx).session_id())
                .and_then(|session| session.project_id.clone()),
            _ => None,
        };
        self.open_new_session(project, window, cx);
    }

    fn on_focus_prompt(&mut self, _: &FocusPrompt, window: &mut Window, cx: &mut Context<Self>) {
        match &self.main {
            Main::Session(view) => view.update(cx, |view, cx| view.focus_prompt(window, cx)),
            Main::NewSession(view) => view.update(cx, |view, cx| view.focus(window, cx)),
            Main::Empty => {}
        }
    }

    fn on_toggle_inbox(&mut self, _: &ToggleInbox, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_inbox(cx);
    }

    fn on_add_project(&mut self, _: &AddProject, window: &mut Window, cx: &mut Context<Self>) {
        self.add_project(window, cx);
    }

    fn on_next_session(&mut self, _: &NextSession, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(1, window, cx);
    }

    fn on_previous_session(
        &mut self,
        _: &PreviousSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_session(-1, window, cx);
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let main = match &self.main {
            Main::Empty => empty::render(self, window, cx).into_any_element(),
            Main::NewSession(view) => view.clone().into_any_element(),
            Main::Session(view) => view.clone().into_any_element(),
        };
        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_new_session))
            .on_action(cx.listener(Self::on_focus_prompt))
            .on_action(cx.listener(Self::on_toggle_inbox))
            .on_action(cx.listener(Self::on_add_project))
            .on_action(cx.listener(Self::on_next_session))
            .on_action(cx.listener(Self::on_previous_session))
            .size_full()
            .flex()
            .flex_row()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .child(sidebar::render(self, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w(px(360.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(main),
            )
            .when(self.inbox_open, |this| {
                this.child(inbox::render(self, window, cx))
            })
    }
}

use gpui::prelude::FluentBuilder as _;
