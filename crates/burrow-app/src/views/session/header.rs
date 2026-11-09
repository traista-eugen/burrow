//! The session's header: title, state, the model and agent mode pickers, usage, a usage limit's
//! countdown, and the session's actions.
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────────────────────────┐
//! │ Fix the login bug ✎                     ● working   [Cancel]  ⏹ stop  ▣ archive  📂 folder │
//! │ Claude Code · [Sonnet 4.6 ▾] · [Ask before edits ▾]      turn 1.2k in · 340 out · $0.01   │
//! │                                                       session 12.3k in · 3.4k out · $0.12 │
//! ├──────────────────────────────────────────────────────────────────────────────────────────┤
//! │ ⏳ Usage limit reached (claude): resets in 42:10, at 15:04. <the agent's message>          │
//! └──────────────────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! - **Pickers** read [`SessionLog::session`]: `models` (shown `name ?? id`, sent by `id`,
//!   D-032) and `agentModes` with their descriptions. They call `session/setModel` and
//!   `session/setAgentMode`; the `modelChanged` and `agentModeChanged` events update the labels.
//! - **Usage:** the latest turn's tokens and cost ([`SessionLog::turn_usage`]) and the session's
//!   totals, where the agent reports them.
//! - **Limits:** while `limited`, the reset time comes from the session's `limit`, else from
//!   `usage/get`, which is asked again when the session becomes limited. A one-second ticker
//!   runs only while a countdown is on screen.
//! - **Actions:** cancel the turn (Esc), stop the agent, archive (with a confirmation), rename
//!   inline (the pencil, or double-click the title; Enter keeps, Esc drops), and open the
//!   session's folder in Explorer.
//!
//! [`SessionLog::session`]: crate::state::session::SessionLog::session
//! [`SessionLog::turn_usage`]: crate::state::session::SessionLog::turn_usage

use std::path::PathBuf;
use std::time::Duration;

use burrow_protocol::{
    AgentMode, ModelInfo, ProviderUsage, Session, SessionState, SessionUsage, Timestamp,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::spinner::Spinner;
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _};
use gpui_kit_assets::IconName as Lucide;

use crate::actions::{CancelRename, RENAME_CONTEXT};
use crate::state::AppState;
use crate::state::session::{LoadStatus, SessionStore};
use crate::theme;
use crate::views::{countdown, dialogs, local_time, now_millis, report_failure};

pub struct SessionHeader {
    store: Entity<SessionStore>,
    state: Entity<AppState>,
    /// The inline title editor, while renaming.
    renaming: Option<Renaming>,
    /// Whether `usage/get` was asked during the current limited spell.
    usage_refreshed: bool,
    /// Whether the one-second countdown ticker runs.
    ticking: bool,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

struct Renaming {
    input: Entity<InputState>,
    _subscription: Subscription,
}

impl SessionHeader {
    pub fn new(
        store: Entity<SessionStore>,
        state: Entity<AppState>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&store, |this, _, cx| {
                this.follow_state(cx);
                cx.notify();
            }),
            cx.observe(&state, |this, _, cx| {
                this.follow_state(cx);
                cx.notify();
            }),
        ];
        let mut this = Self {
            store,
            state,
            renaming: None,
            usage_refreshed: false,
            ticking: false,
            _ticker: None,
            _subscriptions: subscriptions,
        };
        this.follow_state(cx);
        this
    }

    /// Asks for usage once per limited spell, and runs the countdown ticker while one shows.
    fn follow_state(&mut self, cx: &mut Context<Self>) {
        let limited = self.store.read(cx).log().state() == SessionState::Limited;
        if limited && !self.usage_refreshed {
            self.usage_refreshed = true;
            self.state.update(cx, |state, cx| state.refresh_usage(cx));
        } else if !limited {
            self.usage_refreshed = false;
        }
        if !self.ticking && self.countdown_target(cx).is_some() {
            self.ticking = true;
            self._ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let go_on = this.update(cx, |this, cx| {
                        cx.notify();
                        let go_on = this.countdown_target(cx).is_some();
                        this.ticking = go_on;
                        go_on
                    });
                    if !matches!(go_on, Ok(true)) {
                        break;
                    }
                }
            }));
        }
    }

    /// The reset time a countdown shows, while the session is limited and it is still ahead
    /// (or just passed, so "resetting now" shows for a moment).
    fn countdown_target(&self, cx: &Context<Self>) -> Option<Timestamp> {
        let log = self.store.read(cx).log();
        let session = log.session()?;
        let limit = Limit::of(session, self.state.read(cx).usage())?;
        let resets_at = limit.resets_at?;
        (resets_at.as_unix_millis() > now_millis() - 5_000).then_some(resets_at)
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.finish_rename(true, cx),
                _ => {}
            }
            let _ = window;
        });
        self.renaming = Some(Renaming {
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Ends renaming; `keep` sends the new title if it changed.
    fn finish_rename(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let title = renaming.input.read(cx).value().trim().to_owned();
        if keep && !title.is_empty() && title != self.title(cx) {
            let session_id = self.store.read(cx).id().to_owned();
            let call = self.state.read(cx).rename(session_id, title, cx);
            cx.spawn(async move |_, _| {
                if let Err(error) = call.await {
                    tracing::warn!(%error, "couldn't rename the session");
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn on_cancel_rename(&mut self, _: &CancelRename, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_rename(false, cx);
    }

    fn title(&self, cx: &Context<Self>) -> String {
        let store = self.store.read(cx);
        self.state
            .read(cx)
            .index()
            .session(store.id())
            .map(|entry| entry.display_title())
            .or_else(|| store.log().session().and_then(|s| s.title.clone()))
            .unwrap_or_else(|| "New session".to_owned())
    }

    fn title_row(&mut self, archived: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if let Some(renaming) = &self.renaming {
            return div()
                .key_context(RENAME_CONTEXT)
                .on_action(cx.listener(Self::on_cancel_rename))
                .w(px(360.))
                .child(Input::new(&renaming.input).small())
                .into_any_element();
        }
        let title = self.title(cx);
        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("session-title")
                    .min_w_0()
                    .text_base()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .truncate()
                    .child(title)
                    .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.start_rename(window, cx);
                        }
                    })),
            )
            .child(
                Button::new("rename")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::Pencil))
                    .tooltip("Rename (or double-click the title)")
                    .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx))),
            )
            .when(archived, |row| {
                row.child(
                    div()
                        .px(px(6.))
                        .rounded(theme.radius)
                        .bg(theme.muted)
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("archived"),
                )
            })
            .into_any_element()
    }
}

/// A usage limit, as the header shows it.
struct Limit {
    provider: String,
    resets_at: Option<Timestamp>,
    message: Option<String>,
}

impl Limit {
    /// The limit of a `limited` session: its own `limit`, with the reset time from `usage/get`
    /// when the session's doesn't say.
    fn of(session: &Session, usage: &[ProviderUsage]) -> Option<Self> {
        if session.state != SessionState::Limited {
            return None;
        }
        let reported = |provider: &str| usage.iter().find(|u| u.provider == provider);
        match &session.limit {
            Some(limit) => Some(Self {
                provider: limit.provider.clone(),
                resets_at: limit
                    .resets_at
                    .or_else(|| reported(&limit.provider).and_then(|u| u.resets_at)),
                message: Some(limit.message.clone()).filter(|m| !m.is_empty()),
            }),
            None => {
                let usage = usage
                    .iter()
                    .find(|u| u.agent.as_ref() == Some(&session.agent) && u.limited);
                Some(Self {
                    provider: usage
                        .map(|u| u.provider.clone())
                        .unwrap_or_else(|| session.agent.to_string()),
                    resets_at: usage.and_then(|u| u.resets_at),
                    message: usage.and_then(|u| u.message.clone()),
                })
            }
        }
    }

    /// "resets in 42:10, at 15:04", or what is known.
    fn when(&self, now: i64) -> String {
        match self.resets_at {
            Some(at) if at.as_unix_millis() > now => {
                let remaining = Duration::from_millis((at.as_unix_millis() - now) as u64);
                match local_time(at) {
                    Some(clock) => format!("resets in {}, at {clock}", countdown(remaining)),
                    None => format!("resets in {}", countdown(remaining)),
                }
            }
            Some(_) => "should have reset by now; the next prompt tries again".to_owned(),
            None => "no reset time given; the next prompt tries again".to_owned(),
        }
    }
}

/// "12.3k" for large token counts.
fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// "1.2k in · 340 out · $0.01".
fn usage_text(usage: &SessionUsage) -> String {
    let mut text = format!(
        "{} in · {} out",
        compact(usage.input_tokens),
        compact(usage.output_tokens)
    );
    if let Some(cost) = usage.cost_usd {
        text.push_str(&format!(" · ${cost:.2}"));
    }
    text
}

/// A model's label: its `name`, else its id (D-032).
fn model_label(models: &[ModelInfo], id: &str) -> String {
    models
        .iter()
        .find(|model| model.id == id)
        .and_then(|model| model.name.clone())
        .unwrap_or_else(|| id.to_owned())
}

/// An agent mode's label: its `name`, else its id.
fn mode_label(modes: &[AgentMode], id: &str) -> String {
    modes
        .iter()
        .find(|mode| mode.id == id)
        .and_then(|mode| mode.name.clone())
        .unwrap_or_else(|| id.to_owned())
}

impl Render for SessionHeader {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let store = self.store.read(cx);
        let log = store.log();
        let session_id = store.id().to_owned();
        let session = log.session().cloned();
        let turn_usage = log.turn_usage().map(|(_, usage)| usage.clone());
        let state = log.state();
        let busy = log.is_busy();
        let load_error = match store.status() {
            LoadStatus::Failed(error) => Some(error.clone()),
            _ => None,
        };
        let app = self.state.read(cx);
        let archived = app
            .index()
            .session(&session_id)
            .is_some_and(|entry| entry.archived)
            || session.as_ref().is_some_and(|s| s.archived);
        let agent_name = session.as_ref().map(|s| app.agent_name(&s.agent));
        let limit = session.as_ref().and_then(|s| Limit::of(s, app.usage()));
        let title = self.title(cx);

        // ---- State indicator ----
        let color = theme::state_color(&state, cx);
        let reason = session
            .as_ref()
            .filter(|s| matches!(s.state, SessionState::Stopped | SessionState::Failed))
            .and_then(|s| s.reason.clone());
        let pill_tooltip: SharedString = match &reason {
            Some(reason) => format!("{} {reason}", theme::state_description(&state)),
            None => theme::state_description(&state).to_owned(),
        }
        .into();
        let spinning = matches!(state, SessionState::Starting | SessionState::Working);
        let pill = div()
            .id("session-state")
            .flex_shrink_0()
            .px_2()
            .py(px(3.))
            .rounded_full()
            .border_1()
            .border_color(color.opacity(0.5))
            .bg(color.opacity(0.12))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_xs()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(color)
            .map(|pill| {
                if spinning {
                    pill.child(Spinner::new().xsmall().color(color))
                } else {
                    pill.child(div().size(px(6.)).rounded_full().bg(color))
                }
            })
            .child(theme::state_label(&state))
            .tooltip(move |window, cx| Tooltip::new(pill_tooltip.clone()).build(window, cx));

        // ---- Actions ----
        let mut actions = div().flex_shrink_0().flex().items_center().gap_1();
        if busy {
            let state = self.state.clone();
            let id = session_id.clone();
            actions = actions.child(
                Button::new("cancel-turn")
                    .small()
                    .outline()
                    .icon(IconName::Square)
                    .label("Cancel")
                    .tooltip("Stop the running turn; the session stays open (Esc)")
                    .on_click(move |_, window, cx| {
                        let call = state.read(cx).cancel(id.clone(), cx);
                        report_failure("Couldn't cancel the turn", call, window, cx);
                    }),
            );
        }
        if state.is_live() {
            let state = self.state.clone();
            let id = session_id.clone();
            actions = actions.child(
                Button::new("stop-session")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::CircleStop))
                    .tooltip("Stop the agent: ends its process and everything it started")
                    .on_click(move |_, window, cx| {
                        let call = state.read(cx).stop(id.clone(), cx);
                        report_failure("Couldn't stop the session", call, window, cx);
                    }),
            );
        }
        if !archived && session.is_some() {
            let state_entity = self.state.clone();
            let id = session_id.clone();
            let live = state.is_live();
            let title = title.clone();
            actions = actions.child(
                Button::new("archive-session")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::Archive))
                    .tooltip("Archive…")
                    .on_click(move |_, window, cx| {
                        dialogs::confirm_archive(
                            state_entity.clone(),
                            id.clone(),
                            title.clone(),
                            live,
                            window,
                            cx,
                        );
                    }),
            );
        }
        if let Some(cwd) = session.as_ref().map(|s| s.cwd.clone()) {
            let tooltip: SharedString = format!("Open the folder in Explorer\n{cwd}").into();
            actions = actions.child(
                Button::new("open-folder")
                    .ghost()
                    .small()
                    .icon(IconName::FolderOpen)
                    .tooltip(tooltip)
                    .on_click(move |_, _, cx| cx.open_with_system(&PathBuf::from(&cwd))),
            );
        }

        // ---- Facts: agent, pickers, folder ----
        let mut facts = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .text_color(theme.muted_foreground);
        if let Some(name) = agent_name {
            facts = facts.child(div().flex_shrink_0().child(name));
        }
        if let Some(session) = &session {
            facts = facts
                .child(div().child("·"))
                .child(self.model_picker(session, cx))
                .when_some(self.mode_picker(session, cx), |facts, picker| {
                    facts.child(div().child("·")).child(picker)
                });
        }

        // ---- Usage ----
        let session_usage = session.as_ref().and_then(|s| s.usage.clone());
        let usage = (turn_usage.is_some() || session_usage.is_some()).then(|| {
            let line = |label: &'static str, usage: &SessionUsage| {
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        div()
                            .text_color(theme.muted_foreground.opacity(0.7))
                            .child(label),
                    )
                    .child(usage_text(usage))
            };
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_end()
                .text_xs()
                .text_color(theme.muted_foreground)
                .when_some(turn_usage.as_ref(), |col, usage| {
                    col.child(line("turn", usage))
                })
                .when_some(session_usage.as_ref(), |col, usage| {
                    col.child(line("session", usage))
                })
        });

        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_5()
                    .pt_3()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().flex_1().min_w_0().child(self.title_row(archived, cx)))
                            .child(pill)
                            .child(actions),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .child(facts)
                            .children(usage),
                    )
                    .when_some(reason.or(load_error), |col, reason| {
                        col.child(
                            div()
                                .text_xs()
                                .text_color(if state == SessionState::Failed {
                                    theme.danger
                                } else {
                                    theme.muted_foreground
                                })
                                .child(reason),
                        )
                    }),
            )
            .when_some(limit, |col, limit| {
                let mut text = format!(
                    "Usage limit reached ({}): {}.",
                    limit.provider,
                    limit.when(now_millis())
                );
                if let Some(message) = &limit.message {
                    text.push(' ');
                    text.push_str(message);
                }
                col.child(
                    div()
                        .id("limit-banner")
                        .px_5()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .bg(theme.warning.opacity(0.10))
                        .border_t_1()
                        .border_color(theme.warning.opacity(0.3))
                        .text_xs()
                        .text_color(theme.warning)
                        .child(Icon::new(Lucide::Hourglass).xsmall())
                        .child(div().flex_1().min_w_0().child(text)),
                )
            })
    }
}

impl SessionHeader {
    /// The model picker: the session's models by `name ?? id`; picking one sends its id.
    fn model_picker(&self, session: &Session, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = session.model.clone();
        let label = match &current {
            Some(id) => model_label(&session.models, id),
            None => "default model".to_owned(),
        };
        if session.models.is_empty() || !session.state.is_live() {
            return div()
                .truncate()
                .text_color(theme.muted_foreground)
                .child(label)
                .into_any_element();
        }
        let models = session.models.clone();
        let state = self.state.clone();
        let session_id = session.id.clone();
        Button::new("model-picker")
            .ghost()
            .xsmall()
            .label(label)
            .dropdown_caret(true)
            .tooltip("Model")
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.min_w(px(220.)).max_h(px(380.)).scrollable(true);
                for model in &models {
                    let label = model.name.clone().unwrap_or_else(|| model.id.clone());
                    let id = model.id.clone();
                    let state = state.clone();
                    let session_id = session_id.clone();
                    let checked = current.as_deref() == Some(model.id.as_str());
                    menu = menu.item(PopupMenuItem::new(label).checked(checked).on_click(
                        move |_, window, cx| {
                            if checked {
                                return;
                            }
                            let call = state.read(cx).set_model(session_id.clone(), id.clone(), cx);
                            report_failure("Couldn't switch the model", call, window, cx);
                        },
                    ));
                }
                menu
            })
            .into_any_element()
    }

    /// The agent mode picker, with each mode's description. `None` for agents without modes.
    fn mode_picker(&self, session: &Session, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let current = session.agent_mode.clone()?;
        let label = mode_label(&session.agent_modes, &current);
        let description = session
            .agent_modes
            .iter()
            .find(|mode| mode.id == current)
            .and_then(|mode| mode.description.clone());
        if session.agent_modes.is_empty() || !session.state.is_live() {
            return Some(
                div()
                    .truncate()
                    .text_color(theme.muted_foreground)
                    .child(label)
                    .into_any_element(),
            );
        }
        let modes = session.agent_modes.clone();
        let state = self.state.clone();
        let session_id = session.id.clone();
        let tooltip: SharedString = match description {
            Some(description) => format!("Agent mode: {description}").into(),
            None => "Agent mode: what the agent asks permission for".into(),
        };
        let muted = theme.muted_foreground;
        Some(
            Button::new(ElementId::Name("mode-picker".into()))
                .ghost()
                .xsmall()
                .label(label)
                .dropdown_caret(true)
                .tooltip(tooltip)
                .dropdown_menu(move |mut menu, _, _| {
                    menu = menu.min_w(px(260.)).max_w(px(400.));
                    for mode in &modes {
                        let name = mode.name.clone().unwrap_or_else(|| mode.id.clone());
                        let description = mode.description.clone();
                        let id = mode.id.clone();
                        let state = state.clone();
                        let session_id = session_id.clone();
                        let checked = current == mode.id;
                        menu = menu.item(
                            PopupMenuItem::element(move |_, _| {
                                div()
                                    .py(px(2.))
                                    .flex()
                                    .flex_col()
                                    .child(name.clone())
                                    .when_some(description.clone(), |col, description| {
                                        col.child(
                                            div().text_xs().text_color(muted).child(description),
                                        )
                                    })
                            })
                            .checked(checked)
                            .on_click(move |_, window, cx| {
                                if checked {
                                    return;
                                }
                                let call = state.read(cx).set_agent_mode(
                                    session_id.clone(),
                                    id.clone(),
                                    cx,
                                );
                                report_failure("Couldn't switch the agent mode", call, window, cx);
                            }),
                        );
                    }
                    menu
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use burrow_protocol::{AgentId, LimitInfo, SessionMode, UsageSource};

    use super::*;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(1_800_000_000_000 + ms).unwrap()
    }

    fn session(state: SessionState, limit: Option<LimitInfo>) -> Session {
        Session {
            id: "ses_1".into(),
            project_id: None,
            agent: AgentId::Claude,
            model: None,
            mode: SessionMode::Structured,
            cwd: "D:\\repo".into(),
            title: None,
            state,
            reason: None,
            turn_id: None,
            models: Vec::new(),
            agent_mode: None,
            agent_modes: Vec::new(),
            usage: None,
            limit,
            archived: false,
            created_at: at(0),
            updated_at: at(0),
            last_activity_at: None,
        }
    }

    fn reported(provider: &str, resets_at: Option<Timestamp>) -> ProviderUsage {
        ProviderUsage {
            provider: provider.into(),
            agent: Some(AgentId::Claude),
            used: None,
            limit: None,
            unit: None,
            limited: true,
            resets_at,
            message: Some("5-hour limit".into()),
            source: UsageSource::Reported,
            updated_at: at(0),
        }
    }

    #[test]
    fn token_counts_are_compact() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(12_345), "12.3k");
        assert_eq!(compact(2_500_000), "2.5M");
        let usage = SessionUsage {
            input_tokens: 1_234,
            output_tokens: 340,
            cost_usd: Some(0.0123),
        };
        assert_eq!(usage_text(&usage), "1.2k in · 340 out · $0.01");
    }

    #[test]
    fn labels_are_names_or_ids() {
        let models = vec![
            ModelInfo {
                id: "gpt-5.5[context=272k]".into(),
                name: Some("gpt-5.5".into()),
            },
            ModelInfo {
                id: "fake".into(),
                name: None,
            },
        ];
        assert_eq!(model_label(&models, "gpt-5.5[context=272k]"), "gpt-5.5");
        assert_eq!(model_label(&models, "fake"), "fake");
        assert_eq!(model_label(&models, "gone"), "gone");
        let modes = vec![AgentMode {
            id: "read-only".into(),
            name: Some("Read Only".into()),
            description: None,
        }];
        assert_eq!(mode_label(&modes, "read-only"), "Read Only");
        assert_eq!(mode_label(&modes, "plan"), "plan");
    }

    #[test]
    fn a_limit_takes_its_reset_time_from_the_session_then_from_usage() {
        let limit = |resets_at| LimitInfo {
            provider: "claude".into(),
            resets_at,
            message: "You've hit your limit".into(),
        };
        // Not limited: no banner, whatever usage says.
        let idle = session(SessionState::Idle, None);
        assert!(Limit::of(&idle, &[reported("claude", Some(at(5)))]).is_none());

        // The session's own reset time wins.
        let own = session(SessionState::Limited, Some(limit(Some(at(60_000)))));
        let shown = Limit::of(&own, &[reported("claude", Some(at(1)))]).unwrap();
        assert_eq!(shown.resets_at, Some(at(60_000)));
        assert_eq!(shown.message.as_deref(), Some("You've hit your limit"));

        // Without one, usage/get's for the same provider.
        let without = session(SessionState::Limited, Some(limit(None)));
        let shown = Limit::of(&without, &[reported("claude", Some(at(9_000)))]).unwrap();
        assert_eq!(shown.resets_at, Some(at(9_000)));
        assert!(Limit::of(&without, &[]).unwrap().resets_at.is_none());

        // A limited session whose metadata lacks the limit falls back to its agent's usage.
        let bare = session(SessionState::Limited, None);
        let shown = Limit::of(&bare, &[reported("anthropic", Some(at(7)))]).unwrap();
        assert_eq!(shown.provider, "anthropic");
        assert_eq!(shown.resets_at, Some(at(7)));
    }

    #[test]
    fn the_countdown_reads_well() {
        let limit = Limit {
            provider: "fake".into(),
            resets_at: Some(at(42 * 60_000 + 10_000)),
            message: None,
        };
        let text = limit.when(at(0).as_unix_millis());
        assert!(text.starts_with("resets in 42:10"), "{text}");
        assert!(
            limit
                .when(at(50 * 60_000).as_unix_millis())
                .contains("should have reset"),
            "a reset time in the past"
        );
        let unknown = Limit {
            resets_at: None,
            ..limit
        };
        assert!(unknown.when(0).contains("no reset time"));
    }
}
