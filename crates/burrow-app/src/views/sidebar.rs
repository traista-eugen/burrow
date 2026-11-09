//! The sidebar: projects with their sessions, sessions without a project, runs (from M5), the
//! connection status and the inbox button.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _};

use super::relative_time;
use super::workspace::Workspace;
use crate::backend::ConnectionStatus;
use crate::state::index::{IndexModel, ProjectEntry, SessionEntry};
use crate::theme;

/// The sidebar's width.
pub const WIDTH: f32 = 264.;

/// Session ids in the order the sidebar shows them, for stepping through with the keyboard.
pub fn session_order(index: &IndexModel) -> Vec<String> {
    let mut ids = Vec::new();
    for project in index.projects() {
        ids.extend(
            index
                .sessions_of(Some(&project.id))
                .iter()
                .map(|s| s.id.clone()),
        );
    }
    ids.extend(index.sessions_of(None).iter().map(|s| s.id.clone()));
    ids
}

pub fn render(
    workspace: &mut Workspace,
    _window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let state = workspace.state().read(cx);
    let index = state.index();
    let loaded = index.is_loaded();
    let groups: Vec<(ProjectEntry, Vec<SessionEntry>)> = index
        .projects()
        .iter()
        .map(|project| {
            let sessions = index
                .sessions_of(Some(&project.id))
                .into_iter()
                .cloned()
                .collect();
            (project.clone(), sessions)
        })
        .collect();
    let loose: Vec<SessionEntry> = index.sessions_of(None).into_iter().cloned().collect();
    let inbox_count = index.inbox_count();
    let connection = state.connection().clone();
    let selected = workspace.selected_session(cx);
    let new_session_open = workspace.is_new_session_open();

    let mut list = div()
        .id("sidebar-list")
        .flex_1()
        .overflow_y_scroll()
        .px_2()
        .pb_2()
        .flex()
        .flex_col()
        .gap_px();

    list = list.child(section_header(
        "Projects",
        Some(
            Button::new("add-project")
                .ghost()
                .xsmall()
                .icon(IconName::Plus)
                .tooltip("Add a project folder (Ctrl+O)")
                .on_click(cx.listener(|this, _, window, cx| this.add_project(window, cx)))
                .into_any_element(),
        ),
        cx,
    ));
    if !loaded {
        list = list.child(muted_line("Loading…", cx));
    } else if groups.is_empty() {
        list = list.child(muted_line("No projects yet", cx));
    }
    for (project, sessions) in &groups {
        let project_id = project.id.clone();
        let tooltip: SharedString = project.path.clone().unwrap_or_default().into();
        list = list.child(
            div()
                .id(ElementId::Name(format!("project-{}", project.id).into()))
                .group("project-row")
                .h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(theme.radius)
                .text_color(theme.muted_foreground)
                .hover(|style| style.bg(theme.sidebar_accent))
                .child(Icon::new(IconName::Folder).xsmall())
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .text_color(theme.sidebar_foreground)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(project.name.clone()),
                )
                .child(
                    div()
                        .invisible()
                        .group_hover("project-row", |s| s.visible())
                        .child(
                            Button::new(ElementId::Name(format!("new-in-{}", project.id).into()))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Plus)
                                .tooltip("New session in this project")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_new_session(Some(project_id.clone()), window, cx)
                                })),
                        ),
                )
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                }),
        );
        for session in sessions {
            list = list.child(session_row(session, selected.as_deref(), true, cx));
        }
    }
    if !loose.is_empty() {
        list = list.child(section_header("No project", None, cx));
        for session in &loose {
            list = list.child(session_row(session, selected.as_deref(), false, cx));
        }
    }
    list = list
        .child(section_header("Runs", None, cx))
        .child(muted_line("No runs yet", cx));

    div()
        .id("sidebar")
        .w(px(WIDTH))
        .flex_shrink_0()
        .h_full()
        .flex()
        .flex_col()
        .bg(theme.sidebar)
        .border_r_1()
        .border_color(theme.sidebar_border)
        .child(
            div()
                .p_3()
                .pb_2()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(super::empty::logo(px(22.)))
                        .child(
                            div()
                                .text_base()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Burrow"),
                        ),
                )
                .child(
                    Button::new("new-session")
                        .when(new_session_open, |b| b.primary())
                        .when(!new_session_open, |b| b.outline())
                        .small()
                        .w_full()
                        .icon(IconName::Plus)
                        .label("New session")
                        .tooltip("New session (Ctrl+N)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_new_session(None, window, cx)
                        })),
                ),
        )
        .child(list)
        .child(footer(
            connection,
            inbox_count,
            workspace.is_inbox_open(),
            cx,
        ))
}

fn section_header(
    label: &'static str,
    action: Option<AnyElement>,
    cx: &Context<Workspace>,
) -> impl IntoElement {
    div()
        .mt_3()
        .mb_1()
        .px_2()
        .h(px(22.))
        .flex()
        .items_center()
        .justify_between()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(label.to_uppercase())
        .children(action)
}

fn muted_line(text: &'static str, cx: &Context<Workspace>) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground.opacity(0.7))
        .child(text)
}

fn session_row(
    session: &SessionEntry,
    selected: Option<&str>,
    indented: bool,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let is_selected = selected == Some(session.id.as_str());
    let id = session.id.clone();
    let dot = theme::state_color(&session.state, cx);
    let label = theme::state_label(&session.state);
    let when = session.activity.map(relative_time).unwrap_or_default();
    div()
        .id(ElementId::Name(format!("session-{}", session.id).into()))
        .h(px(30.))
        .when(indented, |row| row.pl(px(22.)))
        .when(!indented, |row| row.pl_2())
        .pr_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(theme.radius)
        .cursor_pointer()
        .when(is_selected, |row| row.bg(theme.list_active))
        .when(!is_selected, |row| {
            row.hover(|style| style.bg(theme.sidebar_accent))
        })
        .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(dot))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .when(!is_selected, |d| {
                    d.text_color(theme.sidebar_foreground.opacity(0.88))
                })
                .child(session.display_title()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(match session.state {
                    burrow_protocol::SessionState::Waiting
                    | burrow_protocol::SessionState::Working => dot,
                    _ => theme.muted_foreground.opacity(0.7),
                })
                .child(match session.state {
                    burrow_protocol::SessionState::Waiting
                    | burrow_protocol::SessionState::Working
                    | burrow_protocol::SessionState::Starting
                    | burrow_protocol::SessionState::Limited
                    | burrow_protocol::SessionState::Failed => label,
                    _ => when,
                }),
        )
        .on_click(cx.listener(move |this, _, window, cx| this.open_session(id.clone(), window, cx)))
}

fn workspace_pipe(cx: &Context<Workspace>) -> String {
    crate::backend::Backend::global(cx).pipe_path()
}

fn footer(
    connection: ConnectionStatus,
    inbox_count: usize,
    inbox_open: bool,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let (dot, text): (gpui::Hsla, String) = match &connection {
        ConnectionStatus::Connected { server_version } => (
            theme.success,
            format!("Connected · burrowd {server_version}"),
        ),
        ConnectionStatus::Connecting { attempt: 0 } => (theme.warning, "Connecting…".into()),
        ConnectionStatus::Connecting { attempt } => {
            (theme.warning, format!("Reconnecting… (attempt {attempt})"))
        }
        ConnectionStatus::StartingDaemon => (theme.warning, "Starting burrowd…".into()),
        ConnectionStatus::NotRunning { detail } | ConnectionStatus::Failed { detail } => {
            (theme.danger, detail.clone())
        }
    };
    let pipe = workspace_pipe(cx);
    let tooltip: SharedString = format!(
        "{text}
{pipe}"
    )
    .into();
    div()
        .border_t_1()
        .border_color(theme.sidebar_border)
        .px_3()
        .py_2()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .id("connection")
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(dot))
                .child(div().truncate().child(text))
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                }),
        )
        .child(
            div()
                .relative()
                .child(
                    Button::new("toggle-inbox")
                        .ghost()
                        .small()
                        .when(inbox_open, |b| b.outline())
                        .icon(IconName::Inbox)
                        .tooltip("Inbox (Ctrl+I)")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_inbox(cx))),
                )
                .when(inbox_count > 0, |this| {
                    this.child(
                        div()
                            .absolute()
                            .top(px(-3.))
                            .right(px(-3.))
                            .min_w(px(16.))
                            .h(px(16.))
                            .px(px(4.))
                            .rounded_full()
                            .bg(theme.primary)
                            .text_color(theme.primary_foreground)
                            .text_size(px(10.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if inbox_count > 99 {
                                "99+".to_owned()
                            } else {
                                inbox_count.to_string()
                            }),
                    )
                }),
        )
}
