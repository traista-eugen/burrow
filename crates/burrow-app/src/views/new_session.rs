//! The new session page: pick an agent and a project, type the first prompt, start.
//!
//! Agents come from `agent/list`; ones that can't start are shown with their `problem` and can't
//! be picked. A folder that isn't a project yet is added with `project/add` first. Starting calls
//! `session/create`, which answers once the agent is ready, then sends the prompt, if any, and
//! opens the session.

use burrow_protocol::{AgentId, AgentInfo, SessionCreateParams};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, ElementId, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _};

use crate::state::{AppState, describe_error};

/// What the page asks the workspace to do.
pub enum NewSessionEvent {
    /// A session started: open it.
    Created(String),
    /// The page was closed.
    Dismissed,
}

pub struct NewSessionView {
    state: Entity<AppState>,
    agent: Option<AgentId>,
    project: Option<String>,
    prompt: Entity<TextareaState>,
    busy: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<NewSessionEvent> for NewSessionView {}

impl NewSessionView {
    pub fn new(
        state: Entity<AppState>,
        project: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 12)
                .submit_on_enter(true)
                .placeholder("What should the agent do?  Enter starts, Shift+Enter adds a line")
        });
        let subscriptions = vec![
            cx.observe(&state, |this, _, cx| {
                this.pick_defaults(cx);
                cx.notify();
            }),
            cx.subscribe_in(
                &prompt,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { shift: false, .. } = event {
                        this.start(window, cx);
                    }
                },
            ),
        ];
        let mut this = Self {
            state,
            agent: None,
            project,
            prompt,
            busy: false,
            error: None,
            _subscriptions: subscriptions,
        };
        this.pick_defaults(cx);
        this
    }

    /// Focuses the prompt box.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt
            .update(cx, |prompt, cx| prompt.focus(window, cx));
    }

    /// Picks the first available agent and the first project, until the user picks.
    fn pick_defaults(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let agent_ok = self
            .agent
            .as_ref()
            .is_some_and(|id| state.agents().iter().any(|a| &a.id == id && a.available));
        if !agent_ok {
            self.agent = state
                .agents()
                .iter()
                .find(|agent| agent.available)
                .map(|agent| agent.id.clone());
        }
        let project_ok = self
            .project
            .as_ref()
            .is_some_and(|id| state.index().project(id).is_some());
        if !project_ok {
            self.project = state.index().projects().first().map(|p| p.id.clone());
        }
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Work in this folder".into()),
        });
        let state = self.state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let Ok(call) = cx.update(|_, cx| {
                state
                    .read(cx)
                    .add_project(path.to_string_lossy().into_owned(), cx)
            }) else {
                return;
            };
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(project) => {
                        this.project = Some(project.id);
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(format!(
                            "Couldn't add the folder: {}",
                            describe_error(&error)
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let (Some(agent), Some(project)) = (self.agent.clone(), self.project.clone()) else {
            self.error = Some("Pick an agent and a project first.".into());
            cx.notify();
            return;
        };
        let text = self.prompt.read(cx).value().trim().to_owned();
        self.busy = true;
        self.error = None;
        cx.notify();
        let state = self.state.clone();
        let create = state.read(cx).create_session(
            SessionCreateParams {
                project_id: Some(project),
                agent,
                model: None,
                mode: Default::default(),
                agent_mode: None,
                cwd: None,
                title: None,
            },
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let session = match create.await {
                Ok(session) => session,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.busy = false;
                        this.error = Some(format!(
                            "Couldn't start the session: {}",
                            describe_error(&error)
                        ));
                        cx.notify();
                    });
                    return;
                }
            };
            if !text.is_empty() {
                let prompt = cx.update(|_, cx| state.read(cx).prompt(session.id.clone(), text, cx));
                if let Ok(prompt) = prompt
                    && let Err(error) = prompt.await
                {
                    let _ = cx.update(|window, cx| {
                        super::notify_error("Couldn't send the prompt", &error, window, cx)
                    });
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                cx.emit(NewSessionEvent::Created(session.id));
            });
        })
        .detach();
    }

    fn agent_card(&self, agent: &AgentInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.agent.as_ref() == Some(&agent.id);
        let available = agent.available;
        let id = agent.id.clone();
        let detail = match (&agent.version, available) {
            (Some(version), true) => format!("{} · {version}", agent.integration),
            (None, true) => agent.integration.to_string(),
            (_, false) => "Unavailable".to_owned(),
        };
        div()
            .id(ElementId::Name(format!("agent-{}", agent.id).into()))
            .w(px(214.))
            .p_3()
            .rounded(theme.radius_lg)
            .border_1()
            .flex()
            .flex_col()
            .gap_1()
            .map(|card| {
                if selected {
                    card.border_color(theme.primary)
                        .bg(theme.primary.opacity(0.10))
                } else {
                    card.border_color(theme.border).bg(theme.secondary)
                }
            })
            .when(available, |card| {
                card.cursor_pointer()
                    .hover(|s| s.border_color(theme.primary.opacity(0.7)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.agent = Some(id.clone());
                        cx.notify();
                    }))
            })
            .when(!available, |card| card.opacity(0.55))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(agent.name.clone()),
                    )
                    .when(selected, |row| {
                        row.child(
                            gpui_component::Icon::new(IconName::Check)
                                .small()
                                .text_color(theme.primary),
                        )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(detail),
            )
            .when_some(agent.problem.clone(), |card, problem| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(if available {
                            theme.warning
                        } else {
                            theme.danger
                        })
                        .child(problem),
                )
            })
    }
}

impl Render for NewSessionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let state = self.state.read(cx);
        let agents: Vec<AgentInfo> = state.agents().to_vec();
        let agents_loaded = state.agents_loaded();
        let projects: Vec<(String, String, Option<String>)> = state
            .index()
            .projects()
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.path.clone()))
            .collect();
        let connected = state.connection().is_connected();
        let agent_name = self
            .agent
            .as_ref()
            .map(|id| state.agent_name(id))
            .unwrap_or_else(|| "the agent".into());
        let can_start = connected && self.agent.is_some() && self.project.is_some() && !self.busy;

        let label = |text: &'static str| {
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(text.to_uppercase())
        };

        let mut agent_grid = div().flex().flex_wrap().gap_3();
        if !agents_loaded {
            agent_grid = agent_grid.child(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Detecting agents…"),
            );
        } else if agents.is_empty() {
            agent_grid = agent_grid.child(
                div()
                    .text_color(theme.muted_foreground)
                    .child("burrowd offers no agents."),
            );
        }
        for agent in &agents {
            agent_grid = agent_grid.child(self.agent_card(agent, cx));
        }

        let mut project_row = div().flex().flex_wrap().gap_2();
        for (id, name, path) in projects {
            let selected = self.project.as_deref() == Some(id.as_str());
            let pick = id.clone();
            project_row = project_row.child(
                Button::new(ElementId::Name(format!("pick-{id}").into()))
                    .small()
                    .icon(IconName::Folder)
                    .label(name)
                    .when_some(path, |b, path| b.tooltip(path))
                    .map(|b| if selected { b.primary() } else { b.outline() })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.project = Some(pick.clone());
                        cx.notify();
                    })),
            );
        }
        project_row = project_row.child(
            Button::new("choose-folder")
                .small()
                .ghost()
                .icon(IconName::FolderOpen)
                .label("Choose folder…")
                .on_click(cx.listener(|this, _, window, cx| this.choose_folder(window, cx))),
        );

        div()
            .id("new-session")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(px(760.))
                    .mx_auto()
                    .px_8()
                    .py_8()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("New session"),
                            )
                            .child(
                                Button::new("dismiss-new-session")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .tooltip("Close")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(NewSessionEvent::Dismissed)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(label("Agent"))
                            .child(agent_grid),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(label("Project"))
                            .child(project_row),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(label("Prompt"))
                            .child(Textarea::new(&self.prompt)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                Button::new("start-session")
                                    .primary()
                                    .icon(IconName::ArrowUp)
                                    .label(if self.busy {
                                        format!("Starting {agent_name}…")
                                    } else {
                                        "Start session".to_owned()
                                    })
                                    .loading(self.busy)
                                    .disabled(!can_start)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.start(window, cx)),
                                    ),
                            )
                            .when_some(self.error.clone(), |row, error| {
                                row.child(div().text_color(theme.danger).child(error))
                            }),
                    ),
            )
    }
}
