//! The session's header: title, agent, model and mode, state, usage, and controls.
//!
//! The shell shows what the session is and offers Cancel while a turn runs. The model picker,
//! agent mode picker, stop and archive belong here next; they read [`SessionLog::session`]
//! (`models`, `agentModes`, `usage`, `limit`) and call through [`AppState`] or
//! [`crate::backend::Backend::call`].
//!
//! [`SessionLog::session`]: crate::state::session::SessionLog::session

use burrow_protocol::SessionState;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, px,
};
use gpui_component::button::Button;
use gpui_component::{ActiveTheme as _, IconName, Sizable as _};

use crate::state::AppState;
use crate::state::session::{LoadStatus, SessionStore};
use crate::theme;

pub struct SessionHeader {
    store: Entity<SessionStore>,
    state: Entity<AppState>,
    _subscriptions: Vec<Subscription>,
}

impl SessionHeader {
    pub fn new(
        store: Entity<SessionStore>,
        state: Entity<AppState>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&state, |_, _, cx| cx.notify()),
        ];
        Self {
            store,
            state,
            _subscriptions: subscriptions,
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

impl Render for SessionHeader {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let store = self.store.read(cx);
        let log = store.log();
        let app = self.state.read(cx);
        let entry = app.index().session(store.id());
        let session = log.session();

        let title = entry
            .map(|entry| entry.display_title())
            .or_else(|| session.and_then(|s| s.title.clone()))
            .unwrap_or_else(|| "New session".to_owned());
        let state = log.state();
        let busy = log.is_busy();
        let session_id = store.id().to_owned();

        let mut facts: Vec<String> = Vec::new();
        if let Some(session) = session {
            facts.push(app.agent_name(&session.agent));
            if let Some(model) = &session.model {
                facts.push(model.clone());
            }
            if let Some(mode) = &session.agent_mode {
                let name = session
                    .agent_modes
                    .iter()
                    .find(|m| &m.id == mode)
                    .and_then(|m| m.name.clone())
                    .unwrap_or_else(|| mode.clone());
                facts.push(name);
            }
            if let Some(usage) = &session.usage {
                let mut text = format!(
                    "{} in · {} out",
                    compact(usage.input_tokens),
                    compact(usage.output_tokens)
                );
                if let Some(cost) = usage.cost_usd {
                    text.push_str(&format!(" · ${cost:.2}"));
                }
                facts.push(text);
            }
        }
        let cwd: Option<SharedString> = session.map(|s| s.cwd.clone().into());
        let reason = session
            .filter(|s| matches!(s.state, SessionState::Stopped | SessionState::Failed))
            .and_then(|s| s.reason.clone());
        let failed_to_load = match store.status() {
            LoadStatus::Failed(error) => Some(error.clone()),
            _ => None,
        };
        let dot = theme::state_color(&state, cx);

        div()
            .flex_shrink_0()
            .px_5()
            .py_3()
            .flex()
            .items_center()
            .gap_4()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .children(facts.into_iter().enumerate().map(|(ix, fact)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .when(ix > 0, |d| d.child("·"))
                                    .child(fact)
                            }))
                            .when_some(cwd, |row, cwd| {
                                row.child("·").child(div().truncate().child(cwd))
                            }),
                    )
                    .when_some(reason.or(failed_to_load), |col, reason| {
                        col.child(
                            div()
                                .text_xs()
                                .text_color(theme.danger.opacity(0.9))
                                .child(reason),
                        )
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_2()
                    .py(px(3.))
                    .rounded_full()
                    .border_1()
                    .border_color(dot.opacity(0.5))
                    .bg(dot.opacity(0.12))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_xs()
                    .text_color(dot)
                    .child(div().size(px(6.)).rounded_full().bg(dot))
                    .child(theme::state_label(&state)),
            )
            .when(busy, |row| {
                let state = self.state.clone();
                row.child(
                    Button::new("cancel-turn")
                        .small()
                        .outline()
                        .icon(IconName::Square)
                        .label("Cancel")
                        .tooltip("Stop the running turn (Esc)")
                        .on_click(move |_, _, cx| {
                            state.read(cx).cancel(session_id.clone(), cx).detach();
                        }),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::compact;

    #[test]
    fn token_counts_are_compact() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(12_345), "12.3k");
        assert_eq!(compact(2_500_000), "2.5M");
    }
}
