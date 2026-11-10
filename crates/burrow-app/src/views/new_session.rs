//! The new session page: chat or terminal, pick the agent, where it works, its model and agent
//! mode, type the first prompt, start.
//!
//! - **Chat or terminal.** A chat (structured) session shows the conversation in Burrow's
//!   transcript, with a prompt box. A terminal session runs the agent's own TUI in Burrow's
//!   terminal, so the page asks for no model, mode or prompt: the TUI does. Any installed agent
//!   can run in a terminal, whether or not its ACP adapter is installed or it is logged in. The
//!   terminal starts at the size the session's terminal will have in this window.
//! - **Agents** come from `agent/list`. One that can't start shows its `problem` and can't be
//!   picked. One whose ACP adapter is missing (Claude Code, Codex) offers **Install**, which runs
//!   `agent/install` with its progress and any error on the card; once installed it is picked.
//!   An available agent's `problem` (Muse's sandbox note) shows on its card and, when it is
//!   picked, above the prompt.
//! - **Where:** a project, a folder added as a project (`project/add`), or just a folder, which
//!   starts the session with `cwd` and no project.
//! - **Model and agent mode:** from the agent's `models` and `agentModes` (D-032, D-035); left
//!   alone, the session starts on the agent's defaults. Models show by `name ?? id` and are sent
//!   by id.
//! - **Start** calls `session/create`, which answers once the agent is ready, then sends the
//!   prompt, if any, and opens the session.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use burrow_protocol::{
    AgentId, AgentInfo, AgentMode, ModelInfo, SessionCreateParams, SessionMode, TerminalSize,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Bounds, Context, ElementId, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Pixels, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, canvas,
    div, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::spinner::Spinner;
use gpui_component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _};
use gpui_kit_assets::IconName as Lucide;

use crate::state::{AppState, InstallStatus, describe_error};
use crate::views::session::terminal::STATUS_LINE;
use crate::views::terminal::{TerminalOptions, grid_fitting};

/// The height of a session's header above its terminal, without a reason line: what the main
/// area loses before the terminal starts.
const SESSION_HEADER: f32 = 74.0;

/// What the page asks the workspace to do.
pub enum NewSessionEvent {
    /// A session started: open it. `terminal`: a terminal session, started at this window's
    /// size.
    Created { session_id: String, terminal: bool },
    /// The page was closed.
    Dismissed,
}

/// Where the session works.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// A project; the session works in its folder.
    Project(String),
    /// Just a folder, with no project.
    Folder(String),
}

pub struct NewSessionView {
    state: Entity<AppState>,
    /// Chat (structured) or terminal.
    mode: SessionMode,
    agent: Option<AgentId>,
    /// An agent whose install was started here: picked once it is available.
    installing: Option<AgentId>,
    target: Option<Target>,
    /// The model to start on; `None` for the agent's default.
    model: Option<String>,
    /// The agent mode to start in; `None` for the agent's default.
    agent_mode: Option<String>,
    prompt: Entity<TextareaState>,
    busy: bool,
    /// `agent/refresh` is running.
    detecting: bool,
    error: Option<String>,
    /// Redraws every second while an install runs, for its elapsed time.
    ticker: Option<Task<()>>,
    /// The page's bounds when last drawn: the main area a new session's view will fill.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
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
                this.follow_installs(cx);
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
            mode: SessionMode::Structured,
            agent: None,
            installing: None,
            target: project.map(Target::Project),
            model: None,
            agent_mode: None,
            prompt,
            busy: false,
            detecting: false,
            error: None,
            ticker: None,
            bounds: Rc::default(),
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

    /// Picks the first available agent and the first project, until the user picks; picks an
    /// agent installed from here once it is available.
    fn pick_defaults(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let mode = self.mode;
        let available = |id: &AgentId| state.agent(id).is_some_and(|agent| can_start(agent, mode));
        if let Some(installed) = self.installing.clone()
            && available(&installed)
        {
            self.installing = None;
            self.select_agent(installed);
            return;
        }
        if !self.agent.as_ref().is_some_and(available) {
            let first = state
                .agents()
                .iter()
                .find(|agent| can_start(agent, mode))
                .map(|agent| agent.id.clone());
            if first != self.agent {
                self.agent = first;
                self.model = None;
                self.agent_mode = None;
            }
        }
        let target_ok = match &self.target {
            Some(Target::Project(id)) => state.index().project(id).is_some(),
            Some(Target::Folder(_)) => true,
            None => false,
        };
        if !target_ok {
            self.target = state
                .index()
                .projects()
                .first()
                .map(|p| Target::Project(p.id.clone()));
        }
    }

    /// Switches between a chat and a terminal session.
    fn select_mode(&mut self, mode: SessionMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.error = None;
        self.pick_defaults(cx);
        if mode == SessionMode::Structured {
            self.focus(window, cx);
        }
        cx.notify();
    }

    /// The grid a terminal session's view will fit in this window: the main area, less the
    /// session's header and the terminal's status line.
    fn terminal_size(&self, window: &Window) -> Option<TerminalSize> {
        let bounds = self.bounds.get()?;
        let area = size(
            bounds.size.width,
            bounds.size.height - px(SESSION_HEADER + STATUS_LINE),
        );
        let (cols, rows) = grid_fitting(area, &TerminalOptions::default(), window);
        Some(TerminalSize {
            cols: cols.clamp(TerminalSize::MIN_COLS, TerminalSize::MAX_COLS),
            rows: rows.clamp(TerminalSize::MIN_ROWS, TerminalSize::MAX_ROWS),
        })
    }

    fn select_agent(&mut self, agent: AgentId) {
        if self.agent.as_ref() != Some(&agent) {
            self.agent = Some(agent);
            self.model = None;
            self.agent_mode = None;
        }
    }

    /// Runs the one-second ticker while an install runs.
    fn follow_installs(&mut self, cx: &mut Context<Self>) {
        let running = self.state.read(cx).agents().iter().any(|agent| {
            matches!(
                self.state.read(cx).install_status(&agent.id),
                Some(InstallStatus::Running { .. })
            )
        });
        if running && self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        } else if !running {
            self.ticker = None;
        }
    }

    /// Detects the agents again (`agent/refresh`), for example after logging one in.
    fn detect_again(&mut self, cx: &mut Context<Self>) {
        if self.detecting {
            return;
        }
        self.detecting = true;
        cx.notify();
        let task = self.state.update(cx, |state, cx| state.redetect_agents(cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.detecting = false;
                if let Err(error) = result {
                    this.error = Some(format!(
                        "Couldn't detect the agents: {}",
                        describe_error(&error)
                    ));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install(&mut self, agent: AgentId, cx: &mut Context<Self>) {
        self.installing = Some(agent.clone());
        self.state
            .update(cx, |state, cx| state.install_agent(agent, cx));
    }

    /// Asks for a folder. `as_project` adds it as a project first; otherwise the session works
    /// there with no project.
    fn choose_folder(&mut self, as_project: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                if as_project {
                    "Add project"
                } else {
                    "Work in this folder"
                }
                .into(),
            ),
        });
        let state = self.state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let path = path.to_string_lossy().into_owned();
            if !as_project {
                let _ = this.update(cx, |this, cx| {
                    this.target = Some(Target::Folder(path));
                    this.error = None;
                    cx.notify();
                });
                return;
            }
            let Ok(call) = cx.update(|_, cx| state.read(cx).add_project(path, cx)) else {
                return;
            };
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(project) => {
                        this.target = Some(Target::Project(project.id));
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
        let (Some(agent), Some(target)) = (self.agent.clone(), self.target.clone()) else {
            self.error = Some("Pick an agent, and a project or folder, first.".into());
            cx.notify();
            return;
        };
        let agent_name = self.state.read(cx).agent_name(&agent);
        let terminal = self.mode == SessionMode::Terminal;
        let text = if terminal {
            String::new()
        } else {
            self.prompt.read(cx).value().trim().to_owned()
        };
        let size = if terminal {
            self.terminal_size(window)
        } else {
            None
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        let (project_id, cwd) = match target {
            Target::Project(id) => (Some(id), None),
            Target::Folder(path) => (None, Some(path)),
        };
        let state = self.state.clone();
        let create = state.read(cx).create_session(
            SessionCreateParams {
                project_id,
                agent,
                model: self.model.clone().filter(|_| !terminal),
                mode: self.mode,
                agent_mode: self.agent_mode.clone().filter(|_| !terminal),
                cwd,
                // A terminal session has no first prompt to take its title from.
                title: terminal.then(|| agent_name.clone()),
                cols: size.map(|size| size.cols),
                rows: size.map(|size| size.rows),
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
                cx.emit(NewSessionEvent::Created {
                    session_id: session.id,
                    terminal,
                });
            });
        })
        .detach();
    }

    fn agent_card(&self, agent: &AgentInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.agent.as_ref() == Some(&agent.id);
        let terminal = self.mode == SessionMode::Terminal;
        let available = can_start(agent, self.mode);
        let id = agent.id.clone();
        let detail = match (&agent.version, available, terminal) {
            (Some(version), true, true) => format!("its own TUI · {version}"),
            (None, true, true) => "its own TUI".to_owned(),
            (Some(version), true, false) => format!("{} · {version}", agent.integration),
            (None, true, false) => agent.integration.to_string(),
            (_, false, true) => "Not installed".to_owned(),
            (_, false, false) => "Unavailable".to_owned(),
        };
        // A terminal session runs the agent's own program: no adapter, and it can log in there.
        let problem = agent.problem.clone().filter(|_| !terminal || !available);
        let install = agent
            .adapter
            .as_ref()
            .filter(|adapter| !adapter.installed && !terminal)
            .map(|adapter| {
                let status = self.state.read(cx).install_status(&agent.id).cloned();
                self.install_row(&agent.id, &adapter.package, &adapter.version, status, cx)
            });
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
                        this.select_agent(id.clone());
                        cx.notify();
                    }))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .when(!available, |row| row.opacity(0.6))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(agent.name.clone()),
                    )
                    .when(selected, |row| {
                        row.child(Icon::new(IconName::Check).small().text_color(theme.primary))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(detail),
            )
            .when_some(problem, |card, problem| {
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
            .children(install)
    }

    /// The Install button of an agent whose ACP adapter is missing, or its progress or error.
    fn install_row(
        &self,
        agent: &AgentId,
        package: &str,
        version: &str,
        status: Option<InstallStatus>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let what: SharedString = format!(
            "Installs {package} {version} with npm into Burrow's own folder. It takes a few \
             seconds and 400–500 MB."
        )
        .into();
        let button = |label: &'static str, cx: &mut Context<Self>| {
            let agent = agent.clone();
            Button::new(ElementId::Name(format!("install-{agent}").into()))
                .small()
                .primary()
                .icon(Icon::new(Lucide::Download))
                .label(label)
                .tooltip(what.clone())
                .on_click(cx.listener(move |this, _, _, cx| this.install(agent.clone(), cx)))
        };
        let row = div().pt_1().flex().flex_col().gap_1().text_xs();
        match status {
            None => row.child(button("Install", cx)).into_any_element(),
            Some(InstallStatus::Running { since }) => row
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(theme.foreground)
                        .child(Spinner::new().xsmall().color(theme.primary))
                        .child(format!("Installing… {}s", since.elapsed().as_secs())),
                )
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(format!("npm install {package}@{version}")),
                )
                .into_any_element(),
            Some(InstallStatus::Failed(error)) => row
                .child(
                    div()
                        .text_color(theme.danger)
                        .child(format!("Install failed: {error}")),
                )
                .child(button("Retry", cx))
                .into_any_element(),
        }
    }

    /// The model picker: "Agent's default", then the agent's models by `name ?? id`.
    fn model_picker(&self, agent: &AgentInfo, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if agent.models.is_empty() {
            return muted(
                "The agent's default. Its models show here once one of its sessions has run.",
                cx,
            );
        }
        let label = match &self.model {
            Some(id) => label_of(&agent.models, id),
            None => "Agent's default".to_owned(),
        };
        let models = agent.models.clone();
        let current = self.model.clone();
        let this = cx.entity().downgrade();
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Button::new("new-model")
                    .outline()
                    .small()
                    .label(label)
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.min_w(px(240.)).max_h(px(380.)).scrollable(true);
                        let pick = |model: Option<String>| {
                            let this = this.clone();
                            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |this, cx| {
                                    this.model = model.clone();
                                    cx.notify();
                                });
                            }
                        };
                        menu = menu.item(
                            PopupMenuItem::new("Agent's default")
                                .checked(current.is_none())
                                .on_click(pick(None)),
                        );
                        for model in &models {
                            menu = menu.item(
                                PopupMenuItem::new(
                                    model.name.clone().unwrap_or_else(|| model.id.clone()),
                                )
                                .checked(current.as_deref() == Some(model.id.as_str()))
                                .on_click(pick(Some(model.id.clone()))),
                            );
                        }
                        menu
                    }),
            )
            .when_some(
                self.model
                    .as_ref()
                    .filter(|id| label_of(&agent.models, id) != **id),
                |row, id| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(id.clone()),
                    )
                },
            )
            .into_any_element()
    }

    /// The agent mode picker, with each mode's description; the chosen one's shows below it.
    fn mode_picker(&self, agent: &AgentInfo, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if agent.agent_modes.is_empty() {
            let text: SharedString = match (&agent.default_agent_mode, agent.integration.as_str()) {
                (Some(mode), _) => format!(
                    "Starts in {mode}, the mode that asks before changes. Its other modes show \
                     here once one of its sessions has run."
                )
                .into(),
                (None, "acp") => {
                    "The agent's default. Its modes show here once one of its sessions has run."
                        .into()
                }
                (None, _) => "This agent has no modes.".into(),
            };
            return div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element();
        }
        let default_label = match &agent.default_agent_mode {
            Some(id) => format!("Default: {}", mode_name(&agent.agent_modes, id)),
            None => "Agent's default".to_owned(),
        };
        let label = match &self.agent_mode {
            Some(id) => mode_name(&agent.agent_modes, id),
            None => default_label.clone(),
        };
        let shown = self
            .agent_mode
            .clone()
            .or_else(|| agent.default_agent_mode.clone());
        let description = shown.as_ref().and_then(|id| {
            agent
                .agent_modes
                .iter()
                .find(|mode| &mode.id == id)
                .and_then(|mode| mode.description.clone())
        });
        let modes = agent.agent_modes.clone();
        let current = self.agent_mode.clone();
        let this = cx.entity().downgrade();
        let muted_color = theme.muted_foreground;
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_1()
            .child(
                div().child(
                    Button::new("new-agent-mode")
                        .outline()
                        .small()
                        .label(label)
                        .dropdown_caret(true)
                        .dropdown_menu(move |mut menu, _, _| {
                            menu = menu.min_w(px(280.)).max_w(px(420.));
                            let pick = |mode: Option<String>| {
                                let this = this.clone();
                                move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                                    let _ = this.update(cx, |this, cx| {
                                        this.agent_mode = mode.clone();
                                        cx.notify();
                                    });
                                }
                            };
                            menu = menu.item(
                                PopupMenuItem::new(default_label.clone())
                                    .checked(current.is_none())
                                    .on_click(pick(None)),
                            );
                            for mode in &modes {
                                let name = mode.name.clone().unwrap_or_else(|| mode.id.clone());
                                let description = mode.description.clone();
                                menu = menu.item(
                                    PopupMenuItem::element(move |_, _| {
                                        div()
                                            .py(px(2.))
                                            .flex()
                                            .flex_col()
                                            .child(name.clone())
                                            .when_some(description.clone(), |col, text| {
                                                col.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(muted_color)
                                                        .child(text),
                                                )
                                            })
                                    })
                                    .checked(current.as_deref() == Some(mode.id.as_str()))
                                    .on_click(pick(Some(mode.id.clone()))),
                                );
                            }
                            menu
                        }),
                ),
            )
            .when_some(description, |col, description| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(description),
                )
            })
            .into_any_element()
    }
}

/// Whether `agent` can start a session in `mode`: a chat needs it available (installed, logged
/// in, with its adapter); a terminal only needs it installed, since its TUI logs in itself.
fn can_start(agent: &AgentInfo, mode: SessionMode) -> bool {
    match mode {
        SessionMode::Structured => agent.available,
        SessionMode::Terminal => agent.installed,
    }
}

fn label_of(models: &[ModelInfo], id: &str) -> String {
    models
        .iter()
        .find(|model| model.id == id)
        .and_then(|model| model.name.clone())
        .unwrap_or_else(|| id.to_owned())
}

fn mode_name(modes: &[AgentMode], id: &str) -> String {
    modes
        .iter()
        .find(|mode| mode.id == id)
        .and_then(|mode| mode.name.clone())
        .unwrap_or_else(|| id.to_owned())
}

fn muted(text: &'static str, cx: &Context<NewSessionView>) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
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
        let selected_agent = self
            .agent
            .as_ref()
            .and_then(|id| agents.iter().find(|agent| &agent.id == id))
            .cloned();
        let agent_name = selected_agent
            .as_ref()
            .map(|agent| agent.name.clone())
            .unwrap_or_else(|| "the agent".into());
        let can_start = connected && self.agent.is_some() && self.target.is_some() && !self.busy;
        let terminal = self.mode == SessionMode::Terminal;

        let label = |text: &'static str| {
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(text.to_uppercase())
        };
        let section = |title: &'static str| div().flex().flex_col().gap_2().child(label(title));

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

        let mut where_row = div().flex().flex_wrap().gap_2();
        for (id, name, path) in projects {
            let selected = self.target.as_ref() == Some(&Target::Project(id.clone()));
            let pick = id.clone();
            where_row = where_row.child(
                Button::new(ElementId::Name(format!("pick-{id}").into()))
                    .small()
                    .icon(IconName::Folder)
                    .label(name)
                    .when_some(path, |b, path| b.tooltip(path))
                    .map(|b| if selected { b.primary() } else { b.outline() })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.target = Some(Target::Project(pick.clone()));
                        cx.notify();
                    })),
            );
        }
        if let Some(Target::Folder(path)) = &self.target {
            where_row = where_row.child(
                Button::new("picked-folder")
                    .small()
                    .primary()
                    .icon(IconName::FolderOpen)
                    .label(path.clone())
                    .tooltip("Just this folder, with no project"),
            );
        }
        where_row = where_row
            .child(
                Button::new("add-project-folder")
                    .small()
                    .ghost()
                    .icon(IconName::Plus)
                    .label("Add project…")
                    .tooltip("Add a folder as a project, and work there")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.choose_folder(true, window, cx)),
                    ),
            )
            .child(
                Button::new("just-a-folder")
                    .small()
                    .ghost()
                    .icon(IconName::FolderOpen)
                    .label("Just a folder…")
                    .tooltip("Work in a folder without making it a project")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.choose_folder(false, window, cx)),
                    ),
            );

        let note = selected_agent
            .as_ref()
            .filter(|agent| agent.available && !terminal)
            .and_then(|agent| Some((agent.name.clone(), agent.problem.clone()?)));

        let mode_button = |id: &'static str, label: &'static str, icon: IconName, mode| {
            let selected = self.mode == mode;
            Button::new(id)
                .small()
                .icon(icon)
                .label(label)
                .map(|b| if selected { b.primary() } else { b.outline() })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.select_mode(mode, window, cx)),
                )
        };
        let mode_row = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(mode_button(
                        "mode-chat",
                        "Chat",
                        IconName::Bot,
                        SessionMode::Structured,
                    ))
                    .child(mode_button(
                        "mode-terminal",
                        "Terminal",
                        IconName::SquareTerminal,
                        SessionMode::Terminal,
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if terminal {
                        "The agent's own TUI, in Burrow's terminal. Its model, its mode and its \
                         questions are handled there; burrowctl can attach to it too."
                    } else {
                        "Burrow shows the conversation, the tool calls and the permission \
                         requests, and the inbox collects the questions."
                    }),
            );
        let page_bounds = self.bounds.clone();

        div()
            .id("new-session")
            .size_full()
            .relative()
            .overflow_y_scroll()
            // Notes the page's size: the area a terminal session will have.
            .child(
                canvas(
                    move |bounds, _, _| page_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
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
                    .child(section("Session").child(mode_row))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(label("Agent"))
                                    .child(
                                        Button::new("detect-again")
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::RefreshCw)
                                            .label("Detect again")
                                            .loading(self.detecting)
                                            .tooltip(
                                                "Look for the agents again, for example after                                                  logging one in",
                                            )
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.detect_again(cx)),
                                            ),
                                    ),
                            )
                            .child(agent_grid),
                    )
                    .child(section("Where").child(where_row))
                    .when_some(selected_agent.clone().filter(|_| !terminal), |page, agent| {
                        page.child(
                            div()
                                .flex()
                                .gap_8()
                                .child(
                                    section("Model")
                                        .flex_1()
                                        .min_w_0()
                                        .child(self.model_picker(&agent, cx)),
                                )
                                .child(
                                    section("Agent mode")
                                        .flex_1()
                                        .min_w_0()
                                        .child(self.mode_picker(&agent, cx)),
                                ),
                        )
                    })
                    .when_some(note, |page, (name, problem)| {
                        page.child(
                            div()
                                .p_3()
                                .rounded(theme.radius_lg)
                                .border_1()
                                .border_color(theme.warning.opacity(0.4))
                                .bg(theme.warning.opacity(0.08))
                                .flex()
                                .items_start()
                                .gap_2()
                                .text_xs()
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .small()
                                        .text_color(theme.warning),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(theme.warning)
                                                .child(format!("A note about {name}")),
                                        )
                                        .child(div().text_color(theme.foreground).child(problem)),
                                ),
                        )
                    })
                    .when(!terminal, |page| {
                        page.child(section("Prompt").child(Textarea::new(&self.prompt)))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                Button::new("start-session")
                                    .primary()
                                    .icon(IconName::ArrowUp)
                                    .label(match (self.busy, terminal) {
                                        (true, _) => format!("Starting {agent_name}…"),
                                        (false, true) => "Start terminal".to_owned(),
                                        (false, false) => "Start session".to_owned(),
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
