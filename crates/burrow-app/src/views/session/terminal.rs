//! A terminal session's body: the terminal view ([`crate::views::terminal`]), attached to the
//! session's terminal through the [`TerminalHub`], above a status line.
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────────────────────────┐
//! │ (the agent's TUI, as burrowd emulates it)                                                 │
//! │                                                                                          │
//! ├──────────────────────────────────────────────────────────────────────────────────────────┤
//! │ ● running · 120×32                    Shift+Esc leaves the terminal · Ctrl+Shift+C/V copy, paste │
//! └──────────────────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! - **Attaching.** Once the session names its terminal ([`SessionLog::terminal_id`]), this
//!   attaches through the hub and applies its snapshot, its updates and the scrollback pages it
//!   asks for, mapped onto the widget's model by [`convert`]. The hub attaches again after a
//!   reconnect, with a fresh snapshot. It detaches when this view is dropped: the workspace keeps
//!   the last few sessions' views (D-036), and with them their attachments (D-043).
//! - **Input** goes to `terminal/input`; **sizes** follow "the latest wins" ([`Sizing`], D-040):
//!   this client resizes the terminal only when the user types here, or when it started the
//!   session. Focus and mouse-movement reports go to `terminal/input` too, but aren't typing
//!   (D-045).
//! - **When the program exits** the view is disabled: the last screen stays, readable,
//!   selectable and scrollable, and the status line gives the exit code.
//! - **The bell** flashes a thin border around the terminal.
//!
//! [`SessionLog::terminal_id`]: crate::state::session::SessionLog::terminal_id

use std::time::Duration;

use burrow_protocol::{SessionState, TerminalSize};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
    SharedString, StyleRefinement, Styled as _, Subscription, Task, Window, div, px,
};
use gpui_component::ActiveTheme as _;
use tracing::debug;

use crate::state::AppState;
use crate::state::session::SessionStore;
use crate::state::terminals::{Key, MAX_PAGE, Sizing, TerminalHub, TerminalMessage};
use crate::views::terminal::{TerminalEvent, TerminalOptions, TerminalView};

/// How long the bell's flash shows.
const BELL_FLASH: Duration = Duration::from_millis(180);
/// The status line's height, which the new session page leaves out when it sizes a terminal.
pub const STATUS_LINE: f32 = 24.0;

/// Where the terminal is.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// The program hasn't started yet, or the first snapshot is on its way.
    Connecting,
    /// Attached; the program runs.
    Running,
    /// The connection dropped; a fresh snapshot follows when it is back.
    Reconnecting,
    /// The program exited, with its code; Burrow ended it when there is none.
    Exited(Option<i32>),
    /// The terminal can't be attached, for example because `burrowd` restarted since.
    Unavailable(String),
}

pub struct SessionTerminal {
    store: Entity<SessionStore>,
    hub: Entity<TerminalHub>,
    view: Entity<TerminalView>,
    /// The terminal and this view's registration with the hub.
    attachment: Option<(String, Key)>,
    status: Status,
    /// The window title the program set.
    title: Option<String>,
    /// The terminal's size, from its latest snapshot or reset.
    size: Option<TerminalSize>,
    sizing: Sizing,
    /// The bell's flash is showing.
    ringing: bool,
    _bell: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SessionTerminal {
    /// `holds_size`: this app started the session, at this view's size; it keeps following it.
    pub fn new(
        store: Entity<SessionStore>,
        state: Entity<AppState>,
        holds_size: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hub = state.read(cx).terminals().clone();
        let view = cx.new(|cx| TerminalView::new(TerminalOptions::default(), window, cx));
        let focus = view.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe(&view, |this, _, event: &TerminalEvent, cx| {
                this.on_view(event, cx)
            }),
            cx.observe(&store, |this, _, cx| this.follow_store(cx)),
            // The status line's hint depends on where the focus is.
            cx.on_focus(&focus, window, |_, _, cx| cx.notify()),
            cx.on_blur(&focus, window, |_, _, cx| cx.notify()),
        ];
        // The hub forgets the registration when the view goes (D-043).
        cx.on_release(|this, cx| {
            if let Some((terminal, key)) = this.attachment.take() {
                this.hub.update(cx, |hub, _| hub.detach(&terminal, key));
            }
        })
        .detach();
        let mut this = Self {
            store,
            hub,
            view,
            attachment: None,
            status: Status::Connecting,
            title: None,
            size: None,
            sizing: Sizing::new(holds_size),
            ringing: false,
            _bell: None,
            _subscriptions: subscriptions,
        };
        this.follow_store(cx);
        this
    }

    /// Focuses the terminal, so typing goes to the program.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.view.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Whether the terminal has the keyboard.
    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.view.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The window title the program set, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    #[cfg(test)]
    pub fn status(&self) -> &Status {
        &self.status
    }

    #[cfg(test)]
    pub fn view(&self) -> &Entity<TerminalView> {
        &self.view
    }

    /// Attaches once the session names its terminal; notes an exit its stream reports.
    fn follow_store(&mut self, cx: &mut Context<Self>) {
        let log = self.store.read(cx).log();
        let terminal = log.terminal_id().map(str::to_owned);
        let exit = log.terminal_exit();
        if self.attachment.is_none()
            && let Some(terminal) = terminal
        {
            let this = cx.entity().downgrade();
            let key = self.hub.update(cx, |hub, _| {
                hub.attach(&terminal, move |message, cx| {
                    let _ = this.update(cx, |this, cx| this.receive(message, cx));
                })
            });
            self.attachment = Some((terminal, key));
        }
        // The stream says it ended; terminal/closed says the same while attached.
        if let Some(exit_code) = exit
            && matches!(self.status, Status::Connecting | Status::Reconnecting)
        {
            self.status = Status::Exited(exit_code);
            self.view.update(cx, |view, cx| view.set_disabled(true, cx));
        }
        // The status line reads the session's state.
        cx.notify();
    }

    /// A message from the hub, in the order `burrowd` sent it.
    fn receive(&mut self, message: TerminalMessage, cx: &mut Context<Self>) {
        match message {
            TerminalMessage::Snapshot(snapshot) => {
                self.status = match &snapshot.exit {
                    Some(exit) => Status::Exited(exit.exit_code),
                    None => Status::Running,
                };
                self.title = snapshot.title.clone().filter(|title| !title.is_empty());
                let size = TerminalSize {
                    cols: snapshot.cols,
                    rows: snapshot.rows,
                };
                self.size = Some(size);
                let disabled = snapshot.exit.is_some();
                let modes = convert::modes(&snapshot.modes);
                self.view.update(cx, |view, cx| {
                    view.set_modes(modes, cx);
                    view.apply_snapshot(convert::snapshot(snapshot), cx);
                    view.set_disabled(disabled, cx);
                });
                if let Some(ask) = self.sizing.attached(size) {
                    self.resize(ask, "attached", cx);
                }
            }
            TerminalMessage::Update(update) => {
                if let Some(title) = &update.title {
                    self.title = (!title.is_empty()).then(|| title.clone());
                }
                if update.bell {
                    self.ring(cx);
                }
                let modes = update.modes.as_ref().map(convert::modes);
                let reset = convert::reset(&update);
                if let Some(reset) = &reset {
                    let size = TerminalSize {
                        cols: reset.cols,
                        rows: reset.rows,
                    };
                    self.size = Some(size);
                    self.sizing.resized(size);
                }
                self.view.update(cx, |view, cx| {
                    if let Some(modes) = modes {
                        view.set_modes(modes, cx);
                    }
                    // A reset drops the screen and the scrollback, as a snapshot does.
                    match reset {
                        Some(snapshot) => view.apply_snapshot(snapshot, cx),
                        None => view.apply_update(convert::update(update), cx),
                    }
                });
            }
            TerminalMessage::History { lines, complete } => {
                let lines = lines.into_iter().map(convert::line).collect();
                self.view
                    .update(cx, |view, cx| view.apply_history(lines, complete, cx));
            }
            TerminalMessage::Closed { exit_code } => {
                self.status = Status::Exited(exit_code);
                self.view.update(cx, |view, cx| view.set_disabled(true, cx));
            }
            TerminalMessage::Disconnected => {
                if !matches!(self.status, Status::Exited(_)) {
                    self.status = Status::Reconnecting;
                }
            }
            TerminalMessage::Failed(message) => {
                self.status = match self.store.read(cx).log().terminal_exit() {
                    Some(exit_code) => Status::Exited(exit_code),
                    None => Status::Unavailable(message),
                };
                self.view.update(cx, |view, cx| view.set_disabled(true, cx));
            }
        }
        cx.notify();
    }

    /// What the terminal view asks for.
    fn on_view(&mut self, event: &TerminalEvent, cx: &mut Context<Self>) {
        // The view lays out once, and may do so before the session names its terminal: its size
        // is noted even then, so typing can ask for it later.
        if let TerminalEvent::Resize { cols, rows } = event {
            let size = TerminalSize {
                cols: (*cols).clamp(TerminalSize::MIN_COLS, TerminalSize::MAX_COLS),
                rows: (*rows).clamp(TerminalSize::MIN_ROWS, TerminalSize::MAX_ROWS),
            };
            let ask = self.sizing.view_fits(size);
            if self.status == Status::Running
                && let Some(ask) = ask
            {
                self.resize(ask, "the view resized", cx);
            }
            // The status line says whose size it is.
            cx.notify();
            return;
        }
        let Some((terminal, key)) = self.attachment.clone() else {
            return;
        };
        match event {
            TerminalEvent::Input(data) => {
                if self.status != Status::Running {
                    return;
                }
                if let Some(size) = self.sizing.before_input() {
                    self.resize(size, "typing", cx);
                }
                self.hub.read(cx).input(&terminal, data.clone(), cx);
            }
            // Focus changes and mouse movement: the program asked for them, but the user isn't
            // using this terminal, so the size stays whoever's it is (D-045).
            TerminalEvent::Report(data) => {
                if self.status == Status::Running {
                    self.hub.read(cx).input(&terminal, data.clone(), cx);
                }
            }
            TerminalEvent::Resize { .. } => {}
            TerminalEvent::RequestHistory { before, count } => {
                let limit = u32::try_from(*count).unwrap_or(MAX_PAGE);
                self.hub
                    .update(cx, |hub, _| hub.scrollback(&terminal, key, *before, limit));
            }
        }
    }

    /// Asks for `size`; `why` goes to the log.
    fn resize(&mut self, size: TerminalSize, why: &str, cx: &mut Context<Self>) {
        if let Some((terminal, _)) = &self.attachment {
            debug!(
                terminal,
                cols = size.cols,
                rows = size.rows,
                why,
                "asking for a size"
            );
            self.hub.read(cx).resize(terminal, size, cx);
        }
    }

    /// Flashes the border for a moment.
    fn ring(&mut self, cx: &mut Context<Self>) {
        self.ringing = true;
        self._bell = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BELL_FLASH).await;
            let _ = this.update(cx, |this, cx| {
                this.ringing = false;
                cx.notify();
            });
        }));
    }

    /// The status line's left part: where the terminal is, and its size.
    fn describe(&self, cx: &App) -> String {
        let session_state = self.store.read(cx).log().state();
        let mut text = match &self.status {
            Status::Connecting if !session_state.is_live() => {
                let reason = self
                    .store
                    .read(cx)
                    .log()
                    .session()
                    .and_then(|s| s.reason.clone());
                match reason {
                    Some(reason) => format!("Not running: {reason}"),
                    None => "Not running".to_owned(),
                }
            }
            Status::Connecting if session_state == SessionState::Starting => "Starting…".to_owned(),
            Status::Connecting => "Connecting…".to_owned(),
            Status::Running => "Running".to_owned(),
            Status::Reconnecting => "Reconnecting to burrowd…".to_owned(),
            Status::Exited(Some(code)) => format!("Exited with code {code}"),
            Status::Exited(None) => "Ended by Burrow".to_owned(),
            Status::Unavailable(reason) => format!("Can't show the terminal: {reason}"),
        };
        if let Some(size) = self.size
            && matches!(self.status, Status::Running | Status::Exited(_))
        {
            text.push_str(&format!(" · {}×{}", size.cols, size.rows));
            let fits = self.view.read(cx).size();
            if self.status == Status::Running
                && !self.sizing.holds()
                && fits.is_some_and(|(cols, rows)| (cols, rows) != (size.cols, size.rows))
            {
                text.push_str(", another client's size until you type here");
            }
        }
        text
    }
}

impl Render for SessionTerminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let color = match self.status {
            Status::Running => theme.success,
            Status::Exited(Some(code)) if code != 0 => theme.danger,
            Status::Unavailable(_) => theme.danger,
            _ => theme.muted_foreground,
        };
        let focused = self.is_focused(window, cx);
        let running = self.status == Status::Running;
        let hint: SharedString = if focused && running {
            "Shift+Esc leaves the terminal · Ctrl+Shift+C / V copy and paste".into()
        } else if focused {
            "Shift+Esc leaves the terminal · Ctrl+Shift+C copies a selection".into()
        } else if running {
            "Click the terminal to type · Ctrl+L".into()
        } else {
            "Ctrl+Shift+C copies a selection".into()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        self.view
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    )
                    .when(self.ringing, |area| {
                        area.child(
                            div()
                                .absolute()
                                .inset_0()
                                .border_2()
                                .border_color(theme.warning.opacity(0.7)),
                        )
                    }),
            )
            .child(
                div()
                    .h(px(STATUS_LINE))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(color))
                    .child(div().flex_1().min_w_0().truncate().child(self.describe(cx)))
                    .child(div().flex_shrink_0().child(hint)),
            )
    }
}

/// The protocol's terminal types as the widget's model ([`crate::views::terminal::grid`]).
pub mod convert {
    use burrow_protocol::{
        Color, CursorShape, MouseTracking, TerminalCursor, TerminalLine, TerminalModes,
        TerminalRun, TerminalSnapshot, TerminalUpdate,
    };

    use crate::views::terminal::grid;

    pub fn color(color: Color) -> grid::Color {
        match color {
            Color::Indexed(index) => grid::Color::Indexed(index),
            Color::Rgb(r, g, b) => grid::Color::Rgb(r, g, b),
        }
    }

    pub fn run(run: TerminalRun) -> grid::Run {
        grid::Run {
            text: run.text,
            fg: run.fg.map(color),
            bg: run.bg.map(color),
            flags: grid::Flags::from_bits(run.flags),
        }
    }

    pub fn line(line: TerminalLine) -> grid::IndexedLine {
        let runs = line.runs.into_iter().map(run).collect();
        grid::IndexedLine::new(line.index, grid::Line::new(runs).wrapped(line.wrapped))
    }

    pub fn cursor(cursor: &TerminalCursor) -> grid::Cursor {
        grid::Cursor {
            row: cursor.row,
            col: cursor.col,
            visible: cursor.visible,
            shape: match cursor.shape {
                CursorShape::Underline => grid::CursorShape::Underline,
                CursorShape::Bar => grid::CursorShape::Bar,
                // A shape this version doesn't know draws as a block.
                _ => grid::CursorShape::Block,
            },
        }
    }

    pub fn modes(modes: &TerminalModes) -> grid::Modes {
        grid::Modes {
            app_cursor: modes.app_cursor,
            bracketed_paste: modes.bracketed_paste,
            mouse: match &modes.mouse {
                None => grid::MouseMode::Off,
                Some(MouseTracking::Click) => grid::MouseMode::Click,
                Some(MouseTracking::Drag) => grid::MouseMode::Drag,
                Some(MouseTracking::Motion) => grid::MouseMode::Motion,
                // A tracking mode this version doesn't know: report at least clicks.
                Some(_) => grid::MouseMode::Click,
            },
            mouse_sgr: modes.mouse_sgr,
            alt_screen: modes.alt_screen,
            alternate_scroll: modes.alternate_scroll,
            focus_reporting: modes.focus_events,
        }
    }

    pub fn snapshot(snapshot: TerminalSnapshot) -> grid::Snapshot {
        grid::Snapshot {
            cols: snapshot.cols,
            rows: snapshot.rows,
            cursor: cursor(&snapshot.cursor),
            lines: snapshot.lines.into_iter().map(line).collect(),
        }
    }

    /// A `reset` update as the snapshot it amounts to: the size and every row. `None` for any
    /// other update.
    pub fn reset(update: &TerminalUpdate) -> Option<grid::Snapshot> {
        if !update.reset {
            return None;
        }
        Some(grid::Snapshot {
            cols: update.cols?,
            rows: update.rows?,
            cursor: cursor(&update.cursor),
            lines: update.lines.iter().cloned().map(line).collect(),
        })
    }

    pub fn update(update: TerminalUpdate) -> grid::Update {
        grid::Update {
            cursor: Some(cursor(&update.cursor)),
            scrolled: usize::try_from(update.scrolled).unwrap_or(usize::MAX),
            lines: update.lines.into_iter().map(line).collect(),
            size: None,
        }
    }

    #[cfg(test)]
    mod tests {
        use burrow_protocol::TerminalExit;

        use super::*;

        fn proto_cursor(shape: CursorShape) -> TerminalCursor {
            TerminalCursor {
                row: 1,
                col: 2,
                visible: true,
                shape,
            }
        }

        #[test]
        fn lines_keep_their_styles_and_wraps() {
            let converted = line(TerminalLine {
                index: -3,
                runs: vec![
                    TerminalRun::plain("plain"),
                    TerminalRun {
                        text: "red".into(),
                        fg: Some(Color::Indexed(1)),
                        bg: Some(Color::Rgb(1, 2, 3)),
                        flags: TerminalRun::BOLD | TerminalRun::UNDERCURL,
                    },
                ],
                wrapped: true,
            });
            assert_eq!(converted.index, -3);
            assert!(converted.line.wrapped);
            assert_eq!(converted.line.text(), "plainred");
            let red = &converted.line.runs[1];
            assert_eq!(red.fg, Some(grid::Color::Indexed(1)));
            assert_eq!(red.bg, Some(grid::Color::Rgb(1, 2, 3)));
            assert!(
                red.flags
                    .contains(grid::Flags::BOLD | grid::Flags::UNDERCURL)
            );
        }

        #[test]
        fn cursors_and_modes() {
            let bar = cursor(&proto_cursor(CursorShape::Bar));
            assert_eq!(
                (bar.row, bar.col, bar.shape),
                (1, 2, grid::CursorShape::Bar)
            );
            let unknown = cursor(&proto_cursor(CursorShape::from("beam")));
            assert_eq!(unknown.shape, grid::CursorShape::Block);

            let converted = modes(&TerminalModes {
                app_cursor: true,
                app_keypad: true,
                bracketed_paste: true,
                mouse: Some(MouseTracking::Drag),
                mouse_sgr: true,
                focus_events: true,
                alt_screen: true,
                alternate_scroll: true,
            });
            assert_eq!(
                converted,
                grid::Modes {
                    app_cursor: true,
                    bracketed_paste: true,
                    mouse: grid::MouseMode::Drag,
                    mouse_sgr: true,
                    alt_screen: true,
                    alternate_scroll: true,
                    focus_reporting: true,
                }
            );
            assert_eq!(modes(&TerminalModes::default()), grid::Modes::default());
        }

        #[test]
        fn snapshots_resets_and_updates() {
            let rows = |n: i64| -> Vec<TerminalLine> {
                (0..n)
                    .map(|index| TerminalLine {
                        index,
                        runs: vec![TerminalRun::plain(format!("row {index}"))],
                        wrapped: false,
                    })
                    .collect()
            };
            let snap = snapshot(TerminalSnapshot {
                terminal_id: "trm_1".into(),
                cols: 80,
                rows: 2,
                cursor: proto_cursor(CursorShape::Block),
                lines: rows(2),
                modes: TerminalModes::default(),
                title: None,
                scrollback: 0,
                exit: Some(TerminalExit { exit_code: Some(0) }),
            });
            assert_eq!((snap.cols, snap.rows, snap.lines.len()), (80, 2, 2));

            let mut changed = TerminalUpdate {
                terminal_id: "trm_1".into(),
                reset: false,
                cols: None,
                rows: None,
                scrolled: 3,
                lines: rows(1),
                cursor: proto_cursor(CursorShape::Block),
                modes: None,
                title: None,
                scrollback: None,
                bell: false,
            };
            assert!(reset(&changed).is_none());
            let applied = update(changed.clone());
            assert_eq!(applied.scrolled, 3);
            assert_eq!(applied.size, None);
            assert_eq!(applied.cursor.map(|c| c.col), Some(2));

            changed.reset = true;
            changed.scrolled = 0;
            changed.cols = Some(40);
            changed.rows = Some(3);
            changed.lines = rows(3);
            let fresh = reset(&changed).unwrap();
            assert_eq!((fresh.cols, fresh.rows, fresh.lines.len()), (40, 3, 3));
        }
    }
}
