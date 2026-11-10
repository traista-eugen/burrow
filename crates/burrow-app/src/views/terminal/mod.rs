//! The terminal view (M4): draws a terminal that `burrowd` emulates, and turns keys, pastes and
//! the mouse into input for it.
//!
//! `burrowd` runs the program in a pseudo-terminal and emulates it; this view only draws the
//! grid of styled runs it is sent and reports input (protocol.md, "Terminals").
//!
//! # Parts
//!
//! - [`grid`]: the plain model, [`TerminalGrid`]: the screen's lines of runs, the cursor, the
//!   modes and the scrollback, updated by [`Snapshot`]s and [`Update`]s that replace lines by
//!   index. No GPUI (D-030).
//! - [`input`]: keys, pastes, mouse and focus reports as bytes (D-041).
//! - [`selection`]: selecting with the mouse, and the copied text.
//! - [`metrics`]: how many cells fit, which cell the pointer is over, how far it is scrolled.
//! - [`palette`]: the 256 colours, themed, and how a style becomes colours.
//! - `render`: the font, per-line layouts cached by version, and painting (D-042).
//!
//! # Interface for the session view
//!
//! [`TerminalView`] is an entity. Feed it:
//! - [`TerminalView::apply_snapshot`] with `terminal/attach`'s answer;
//! - [`TerminalView::apply_update`] with each `terminal/update`;
//! - [`TerminalView::set_modes`] when the input modes change;
//! - [`TerminalView::apply_history`] with a page of scrollback it asked for.
//!
//! Subscribe to its [`TerminalEvent`]s:
//! - [`TerminalEvent::Input`]: send as `terminal/input`;
//! - [`TerminalEvent::Resize`]: send as `terminal/resize`, debounced;
//! - [`TerminalEvent::RequestHistory`]: fetch older scrollback and give it to `apply_history`.
//!
//! Call [`bind_keys`] once at startup, after the app's own bindings. Embed the view with
//! `AnyView::from(view).cached(..)`, so frames that redraw other parts of the window don't
//! repaint the terminal.

pub mod grid;
pub mod input;
pub mod metrics;
pub mod palette;
mod render;
pub mod selection;

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, HitboxBehavior,
    InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, NoAction, ParentElement as _, Pixels, Point, Render, ScrollEvent,
    SharedString, Styled as _, Subscription, Task, UTF16Selection, Window, actions, canvas, div,
    point, px, size,
};

pub use grid::{CursorShape, IndexedLine, Modes, MouseMode, Snapshot, TerminalGrid, Update};
pub use render::{FrameStats, FrameSummary};

use input::{MouseButton as ReportButton, MouseReport};
use metrics::{ScrollPosition, cell_at, grid_size};
use palette::TerminalTheme;
use render::{CellMetrics, CursorPaint, Frame, LineCache, MarkedPaint, hsla, hsla_alpha};
use selection::{GridPoint, Selection, SelectionKind, Side};

actions!(
    terminal,
    [
        /// Copies the selection (Ctrl+Shift+C, Ctrl+Insert).
        CopySelection,
        /// Pastes the clipboard's text (Ctrl+Shift+V, Shift+Insert).
        PasteClipboard,
    ]
);

/// The key context the terminal sets.
pub const TERMINAL_CONTEXT: &str = "BurrowTerminal";

/// How long the size has to stay put before a resize is reported.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(80);
/// The cursor's blink: half a period on, half off, and it stops (shown) after this long idle.
const BLINK_INTERVAL: Duration = Duration::from_millis(530);
const BLINK_IDLE: Duration = Duration::from_secs(15);
/// Scrollback is asked for this many lines at a time, when the view is this close to the oldest
/// line loaded.
const HISTORY_PAGE: usize = 500;
const HISTORY_PREFETCH: usize = 100;
/// The selection's opacity over the text.
const SELECTION_ALPHA: f32 = 0.35;
/// Space between the grid and the element's edges.
const PADDING: Pixels = px(6.0);

/// Binds the terminal's keys (D-041). Call after the app's own bindings: the keys the app binds
/// that a program in a terminal needs (Ctrl+N, Ctrl+L, Ctrl+I, Ctrl+O, Esc) go to the terminal
/// while it has focus.
pub fn bind_keys(cx: &mut App) {
    let context = Some(TERMINAL_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-c", CopySelection, context),
        KeyBinding::new("ctrl-insert", CopySelection, context),
        KeyBinding::new("ctrl-shift-v", PasteClipboard, context),
        KeyBinding::new("shift-insert", PasteClipboard, context),
        KeyBinding::new("ctrl-n", NoAction, context),
        KeyBinding::new("ctrl-l", NoAction, context),
        KeyBinding::new("ctrl-i", NoAction, context),
        KeyBinding::new("ctrl-o", NoAction, context),
        KeyBinding::new("escape", NoAction, context),
    ]);
}

/// What the terminal view asks of whoever runs the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    /// Bytes to send as `terminal/input`.
    Input(String),
    /// The grid that fits the view changed: send `terminal/resize`. Reported at once the first
    /// time, then after the size stays put for 80 ms.
    Resize { cols: u16, rows: u16 },
    /// The view is near the oldest scrollback loaded: fetch up to `count` lines above line
    /// `before` (negative indices; `0` when none is loaded), and give them to
    /// [`TerminalView::apply_history`].
    RequestHistory { before: i64, count: usize },
}

/// How the view looks.
#[derive(Clone, Debug)]
pub struct TerminalOptions {
    /// The font family; `None` picks Cascadia Mono, or Consolas without it.
    pub font_family: Option<SharedString>,
    pub font_size: Pixels,
    /// Whether the cursor blinks while the view has focus.
    pub blink: bool,
    pub theme: TerminalTheme,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            font_family: None,
            font_size: px(14.0),
            blink: true,
            theme: TerminalTheme::burrow(),
        }
    }
}

/// A mouse button held while reporting it to the program, and the cell last reported.
#[derive(Clone, Copy)]
struct HeldReport {
    button: ReportButton,
    cell: (u16, u16),
}

/// One terminal, drawn and taking input.
pub struct TerminalView {
    grid: TerminalGrid,
    options: TerminalOptions,
    focus_handle: FocusHandle,
    metrics: Option<CellMetrics>,
    lines: LineCache,
    /// The grid area of the last frame, in window coordinates.
    bounds: Bounds<Pixels>,
    /// The size last reported, and the one waiting out the debounce.
    reported_size: Option<(u16, u16)>,
    pending_size: Option<(u16, u16)>,
    _resize: Option<Task<()>>,
    scroll: ScrollPosition,
    history_requested: bool,
    selection: Option<Selection>,
    selecting: bool,
    held: Option<HeldReport>,
    /// The cell a motion was last reported for, in any-motion mode.
    moved: Option<(u16, u16)>,
    /// IME composition in progress.
    marked: Option<String>,
    /// The view has focus; the cursor blinks only then.
    focused: bool,
    /// The cursor is in the visible half of its blink, and a task is blinking it.
    blink_on: bool,
    blinking: bool,
    last_activity: Instant,
    stats: Rc<RefCell<FrameStats>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    pub fn new(options: TerminalOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _, cx| {
                this.focus_changed(true, cx)
            }),
            cx.on_blur(&focus_handle, window, |this, _, cx| {
                this.focus_changed(false, cx)
            }),
        ];
        Self {
            grid: TerminalGrid::new(80, 24),
            options,
            focus_handle,
            metrics: None,
            lines: LineCache::default(),
            bounds: Bounds::default(),
            reported_size: None,
            pending_size: None,
            _resize: None,
            scroll: ScrollPosition::default(),
            history_requested: false,
            selection: None,
            selecting: false,
            held: None,
            moved: None,
            marked: None,
            focused: false,
            blink_on: true,
            blinking: false,
            last_activity: Instant::now(),
            stats: Rc::default(),
            _subscriptions: subscriptions,
        }
    }

    /// The model.
    pub fn grid(&self) -> &TerminalGrid {
        &self.grid
    }

    /// The size last reported through [`TerminalEvent::Resize`].
    pub fn size(&self) -> Option<(u16, u16)> {
        self.reported_size
    }

    /// Selects the cells from `from` to `to`, inclusive, as a drag across them would.
    pub fn select(&mut self, from: GridPoint, to: GridPoint, cx: &mut Context<Self>) {
        let (from, to) = if from <= to { (from, to) } else { (to, from) };
        let mut selection = Selection::new(SelectionKind::Cells, from, Side::Left);
        selection.extend(to, Side::Right);
        self.selection = Some(selection);
        cx.notify();
    }

    /// The selected text, if any.
    pub fn selected_text(&self) -> Option<String> {
        let range = self.selection?.range(&self.grid)?;
        Some(range.text(&self.grid))
    }

    /// How long recent frames took to draw.
    pub fn frame_stats(&self) -> FrameSummary {
        self.stats.borrow().summary()
    }

    /// Forgets the frames measured so far.
    pub fn reset_frame_stats(&self) {
        self.stats.borrow_mut().clear();
    }

    /// Replaces everything with `terminal/attach`'s snapshot.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.grid.apply_snapshot(snapshot);
        self.selection = None;
        self.scroll.reset();
        self.history_requested = false;
        self.activity(cx);
        cx.notify();
    }

    /// Applies a `terminal/update`.
    pub fn apply_update(&mut self, update: Update, cx: &mut Context<Self>) {
        let changes = self.grid.apply_update(update);
        if changes.scrolled > 0 {
            if let Some(selection) = &mut self.selection {
                selection.scroll(changes.scrolled);
            }
            // New output returns to the bottom. Lines rewritten in place (a spinner, a clock)
            // don't, so scrollback stays readable while a program animates (D-041).
            self.scroll.reset();
        }
        if let (Some(selection), Some((first, last))) = (&self.selection, changes.rows) {
            let (top, bottom) = selection.lines();
            if top <= i64::from(last) && bottom >= i64::from(first) {
                self.selection = None;
            }
        }
        if changes.any() {
            self.activity(cx);
        }
        cx.notify();
    }

    /// Adds a page of scrollback it asked for. `complete`: nothing older exists.
    pub fn apply_history(
        &mut self,
        lines: Vec<IndexedLine>,
        complete: bool,
        cx: &mut Context<Self>,
    ) {
        self.grid.apply_history(lines, complete);
        self.history_requested = false;
        self.request_history_if_near_top(cx);
        cx.notify();
    }

    /// Sets the input modes the emulator reports.
    pub fn set_modes(&mut self, modes: Modes, cx: &mut Context<Self>) {
        if modes != self.grid.modes() {
            self.grid.set_modes(modes);
            cx.notify();
        }
    }

    /// Pastes `text` into the terminal.
    pub fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let bytes = input::encode_paste(text, &self.grid.modes());
        self.send(bytes, cx);
    }

    /// Sends bytes the user typed: back to the bottom, selection gone, cursor shown.
    fn send(&mut self, bytes: String, cx: &mut Context<Self>) {
        if bytes.is_empty() {
            return;
        }
        self.scroll.reset();
        self.selection = None;
        self.activity(cx);
        cx.emit(TerminalEvent::Input(bytes));
        cx.notify();
    }

    /// Something happened: the cursor shows, and blinks again if the view has focus. One task
    /// blinks it, and stops once the view loses focus or nothing has happened for a while, so an
    /// idle or unfocused terminal doesn't redraw.
    fn activity(&mut self, cx: &mut Context<Self>) {
        self.last_activity = Instant::now();
        self.blink_on = true;
        if !self.options.blink || !self.focused || self.blinking {
            return;
        }
        self.blinking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                let go_on = this.update(cx, |this, cx| {
                    let idle = this.last_activity.elapsed();
                    let stop = !this.focused || idle >= BLINK_IDLE;
                    let on = stop || idle < BLINK_INTERVAL || !this.blink_on;
                    if on != this.blink_on {
                        this.blink_on = on;
                        cx.notify();
                    }
                    this.blinking = !stop;
                    !stop
                });
                if !matches!(go_on, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.focused = focused;
        if let Some(bytes) = input::encode_focus(focused, &self.grid.modes()) {
            cx.emit(TerminalEvent::Input(bytes.to_owned()));
        }
        self.activity(cx);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // While composing, keys belong to the IME; characters AltGr or dead keys make arrive as
        // text through the input handler.
        if self.marked.is_some() || event.prefer_character_input {
            return;
        }
        if let Some(bytes) = input::encode_key(&event.keystroke, &self.grid.modes()) {
            cx.stop_propagation();
            self.send(bytes, cx);
        }
    }

    fn copy(&mut self, _: &CopySelection, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.selected_text() else {
            return;
        };
        let text = if cfg!(windows) {
            text.replace('\n', "\r\n")
        } else {
            text
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn paste(&mut self, _: &PasteClipboard, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(&text, cx);
        }
    }

    /// The cell under a window position, and which half of it.
    fn cell_at(&self, position: Point<Pixels>) -> Option<(u16, u16, Side)> {
        let metrics = self.metrics.as_ref()?;
        let local = position - self.bounds.origin;
        Some(cell_at(
            local.x.into(),
            local.y.into(),
            metrics.cell_size(),
            self.grid.cols(),
            self.grid.rows(),
        ))
    }

    /// Whether the program takes the mouse, unless Shift is held, which always selects.
    fn reports_mouse(&self, shift: bool) -> bool {
        self.grid.modes().mouse != MouseMode::Off && !shift
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let Some((col, row, side)) = self.cell_at(event.position) else {
            return;
        };
        let button = match event.button {
            MouseButton::Left => ReportButton::Left,
            MouseButton::Middle => ReportButton::Middle,
            MouseButton::Right => ReportButton::Right,
            _ => return,
        };
        if self.reports_mouse(event.modifiers.shift) {
            let report = MouseReport::Press(button);
            if let Some(bytes) =
                input::encode_mouse(report, col, row, &event.modifiers, &self.grid.modes())
            {
                self.held = Some(HeldReport {
                    button,
                    cell: (col, row),
                });
                cx.emit(TerminalEvent::Input(bytes));
            }
            cx.stop_propagation();
            return;
        }
        if button != ReportButton::Left {
            return;
        }
        let point = GridPoint::new(self.scroll.line_at(row), usize::from(col));
        match (event.click_count, &mut self.selection) {
            (1, Some(selection)) if event.modifiers.shift => selection.extend(point, side),
            (count, _) => {
                let kind = match count {
                    1 => SelectionKind::Cells,
                    2 => SelectionKind::Words,
                    _ => SelectionKind::Lines,
                };
                self.selection = Some(Selection::new(kind, point, side));
            }
        }
        self.selecting = true;
        cx.stop_propagation();
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((col, row, side)) = self.cell_at(event.position) else {
            return;
        };
        if let Some(held) = self.held {
            if event.pressed_button.is_some() && held.cell != (col, row) {
                let report = MouseReport::Drag(held.button);
                if let Some(bytes) =
                    input::encode_mouse(report, col, row, &event.modifiers, &self.grid.modes())
                {
                    cx.emit(TerminalEvent::Input(bytes));
                }
                self.held = Some(HeldReport {
                    cell: (col, row),
                    ..held
                });
            }
            return;
        }
        if self.selecting {
            // Moves without the button are Windows' synthetic ones, sent when windows repaint
            // under the pointer. The selection ends at mouse up, which GPUI always delivers: it
            // captures the mouse on press.
            if event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            // Dragging past the top or bottom edge scrolls.
            let y = event.position.y;
            if y < self.bounds.top() {
                self.scroll_lines(1.0, cx);
            } else if y > self.bounds.bottom() && self.scroll.offset > 0 {
                self.scroll_lines(-1.0, cx);
            }
            let point = GridPoint::new(self.scroll.line_at(row), usize::from(col));
            if let Some(selection) = &mut self.selection {
                selection.extend(point, side);
            }
            cx.notify();
            return;
        }
        if self.bounds.contains(&event.position)
            && self.grid.modes().mouse == MouseMode::Motion
            && self.moved != Some((col, row))
        {
            self.moved = Some((col, row));
            if let Some(bytes) = input::encode_mouse(
                MouseReport::Move,
                col,
                row,
                &event.modifiers,
                &self.grid.modes(),
            ) {
                cx.emit(TerminalEvent::Input(bytes));
            }
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if let Some(held) = self.held.take() {
            let (col, row, _) =
                self.cell_at(event.position)
                    .unwrap_or((held.cell.0, held.cell.1, Side::Left));
            let report = MouseReport::Release(held.button);
            if let Some(bytes) =
                input::encode_mouse(report, col, row, &event.modifiers, &self.grid.modes())
            {
                cx.emit(TerminalEvent::Input(bytes));
            }
            return;
        }
        if self.selecting {
            self.selecting = false;
            // A click that selected nothing clears the selection.
            if self
                .selection
                .is_some_and(|s| s.range(&self.grid).is_none())
            {
                self.selection = None;
            }
            cx.notify();
        }
    }

    fn scroll_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        let lines = f32::from(event.delta.pixel_delta(metrics.line_height).y)
            / f32::from(metrics.line_height);
        if lines == 0.0 {
            return;
        }
        let modes = self.grid.modes();
        if self.reports_mouse(event.modifiers.shift) {
            let Some((col, row, _)) = self.cell_at(event.position) else {
                return;
            };
            let report = if lines > 0.0 {
                MouseReport::WheelUp
            } else {
                MouseReport::WheelDown
            };
            let count = lines.abs().round().max(1.0) as usize;
            if let Some(bytes) = input::encode_mouse(report, col, row, &event.modifiers, &modes) {
                cx.emit(TerminalEvent::Input(bytes.repeat(count)));
            }
            cx.stop_propagation();
            return;
        }
        let count = lines.abs().round().max(1.0) as usize;
        if let Some(bytes) = input::encode_alternate_scroll(lines > 0.0, count, &modes) {
            cx.emit(TerminalEvent::Input(bytes));
            cx.stop_propagation();
            return;
        }
        self.scroll_lines(lines, cx);
        cx.stop_propagation();
    }

    /// Scrolls back (positive) or forward through the scrollback.
    fn scroll_lines(&mut self, lines: f32, cx: &mut Context<Self>) {
        if self.scroll.scroll_by(lines, self.grid.history_len()) {
            cx.notify();
        }
        if lines > 0.0 {
            self.request_history_if_near_top(cx);
        }
    }

    /// Asks for older scrollback when the view is near the oldest line loaded.
    fn request_history_if_near_top(&mut self, cx: &mut Context<Self>) {
        if self.history_requested || self.grid.history_complete() {
            return;
        }
        let rows = usize::from(self.grid.rows());
        let top_shown = self.scroll.offset + rows;
        if top_shown + HISTORY_PREFETCH < self.grid.history_len() + rows {
            return;
        }
        self.history_requested = true;
        cx.emit(TerminalEvent::RequestHistory {
            before: self.grid.oldest_index(),
            count: HISTORY_PAGE,
        });
    }

    /// Reports the size that fits, at once the first time and debounced after.
    fn fit(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        let size = (cols, rows);
        if self.reported_size.is_none() {
            self.reported_size = Some(size);
            cx.emit(TerminalEvent::Resize { cols, rows });
            return;
        }
        if self.pending_size == Some(size)
            || (self.pending_size.is_none() && self.reported_size == Some(size))
        {
            return;
        }
        self.pending_size = Some(size);
        self._resize = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESIZE_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                if let Some((cols, rows)) = this.pending_size.take()
                    && this.reported_size != Some((cols, rows))
                {
                    this.reported_size = Some((cols, rows));
                    cx.emit(TerminalEvent::Resize { cols, rows });
                }
            });
        }));
    }

    fn metrics(&mut self, window: &Window) -> &CellMetrics {
        let stale = self
            .metrics
            .as_ref()
            .is_none_or(|metrics| metrics.scale != window.scale_factor());
        if stale {
            let family = match &self.options.font_family {
                Some(family) => family.to_string(),
                None => render::pick_family(window).to_owned(),
            };
            self.metrics = Some(CellMetrics::measure(
                &family,
                self.options.font_size,
                window,
            ));
            self.lines.clear();
        }
        self.metrics.as_ref().expect("measured above")
    }

    /// Works out what this frame paints.
    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Frame {
        let started = Instant::now();
        let metrics = self.metrics(window).clone();
        self.bounds = bounds;
        let (cols, rows) = grid_size(
            bounds.size.width.into(),
            bounds.size.height.into(),
            metrics.cell_size(),
        );
        self.fit(cols, rows, cx);

        let theme = &self.options.theme;
        let grid_rows = self.grid.rows();
        let shown = grid_rows.min(rows);
        let scroll = self.scroll;
        let grid = &self.grid;
        let lines = (0..shown).map(|row| {
            let index = scroll.line_at(row);
            Some((grid.line_version(index)?, grid.line(index)?))
        });
        let laid_out = self.lines.frame(lines, theme, &metrics, window);

        let grid_cols = usize::from(self.grid.cols());
        let selection: Vec<(usize, usize, usize)> = self
            .selection
            .and_then(|selection| selection.range(&self.grid))
            .map(|range| {
                (0..shown)
                    .filter_map(|row| {
                        let (first, last) = range.columns(scroll.line_at(row), grid_cols)?;
                        Some((usize::from(row), first, last - first + 1))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let focused = self.focus_handle.is_focused(window);
        let cursor = self.grid.cursor();
        let cursor_row = i64::from(cursor.row) + scroll.offset as i64;
        let cursor_cell = |cols: usize| Bounds {
            origin: point(
                bounds.origin.x + metrics.cell_width * f32::from(cursor.col),
                bounds.origin.y + metrics.line_height * cursor_row as f32,
            ),
            size: size(metrics.cell_width * cols as f32, metrics.line_height),
        };
        let on_screen = cursor_row < i64::from(shown);
        let under_cursor = self.grid.line(i64::from(cursor.row)).and_then(|line| {
            line.cells()
                .into_iter()
                .find(|cell| {
                    cell.width > 0
                        && cell.col <= usize::from(cursor.col)
                        && usize::from(cursor.col) < cell.col + cell.width
                })
                .map(|cell| (cell.text.to_owned(), cell.width, cell.style.flags))
        });
        let cursor_paint = (cursor.visible
            && on_screen
            && self.marked.is_none()
            && (self.blink_on || !focused))
            .then(|| {
                let width = under_cursor.as_ref().map_or(1, |(_, width, _)| *width);
                let text = (focused && cursor.shape == CursorShape::Block)
                    .then_some(under_cursor.as_ref())
                    .flatten()
                    .filter(|(text, _, _)| !text.trim().is_empty())
                    .map(|(text, _, flags)| {
                        render::shape_cell(text, *flags, hsla(theme.cursor_text), &metrics, window)
                    });
                CursorPaint {
                    bounds: cursor_cell(width),
                    shape: cursor.shape,
                    focused,
                    color: hsla(theme.cursor),
                    text,
                }
            });
        let marked = self.marked.as_ref().filter(|_| on_screen).map(|text| {
            let shaped = render::shape_marked(text, hsla(theme.foreground), &metrics, window);
            let width = text.chars().map(grid::char_width).sum::<usize>().max(1);
            MarkedPaint {
                bounds: cursor_cell(width),
                background: hsla(theme.background),
                text: shaped,
            }
        });

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        Frame {
            bounds,
            rows: laid_out,
            selection,
            selection_color: hsla_alpha(theme.selection, SELECTION_ALPHA),
            cursor: cursor_paint,
            marked,
            hitbox,
            prepaint: started.elapsed(),
            laid_out: self.lines.misses,
            metrics,
        }
    }
}

/// Paints a frame and registers the mouse and keyboard input for the next one.
fn paint(frame: Frame, view: &Entity<TerminalView>, window: &mut Window, cx: &mut App) {
    let started = Instant::now();
    let reports_mouse = view.read(cx).grid.modes().mouse != MouseMode::Off;
    window.set_cursor_style(
        if reports_mouse {
            CursorStyle::Arrow
        } else {
            CursorStyle::IBeam
        },
        &frame.hitbox,
    );
    frame.paint(window, cx);

    let hitbox = frame.hitbox.clone();
    let target = view.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
            target.update(cx, |this, cx| this.mouse_down(event, window, cx));
        }
    });
    let target = view.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            target.update(cx, |this, cx| this.mouse_move(event, cx));
        }
    });
    let target = view.clone();
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            target.update(cx, |this, cx| this.mouse_up(event, cx));
        }
    });
    let hitbox = frame.hitbox.clone();
    let target = view.clone();
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
            target.update(cx, |this, cx| this.scroll_wheel(event, cx));
        }
    });

    let focus = view.read(cx).focus_handle.clone();
    window.handle_input(
        &focus,
        ElementInputHandler::new(frame.bounds, view.clone()),
        cx,
    );

    let took = started.elapsed();
    view.read(cx)
        .stats
        .borrow_mut()
        .record(frame.prepaint + took, frame.laid_out);
    tracing::debug!(
        target: "perf",
        rows = frame.rows.len(),
        laid_out = frame.laid_out,
        prepaint_us = frame.prepaint.as_micros() as u64,
        paint_us = took.as_micros() as u64,
        "terminal frame"
    );
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let prepaint_view = view.clone();
        div()
            .id("terminal")
            .key_context(TERMINAL_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .size_full()
            .p(PADDING)
            .bg(hsla(self.options.theme.background))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        prepaint_view.update(cx, |this, cx| this.prepaint(bounds, window, cx))
                    },
                    move |_, frame, window, cx| paint(frame, &view, window, cx),
                )
                .size_full(),
            )
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let marked: Vec<u16> = self.marked.as_ref()?.encode_utf16().collect();
        let range = range.start.min(marked.len())..range.end.min(marked.len());
        *adjusted_range = Some(range.clone());
        String::from_utf16(&marked[range]).ok()
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.marked.as_deref().map_or(0, utf16_len);
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_deref().map(|text| 0..utf16_len(text))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        self.send(text.to_owned(), cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let metrics = self.metrics.as_ref()?;
        let cursor = self.grid.cursor();
        let row = f32::from(cursor.row) + self.scroll.offset as f32;
        Some(Bounds {
            origin: point(
                element_bounds.origin.x + metrics.cell_width * f32::from(cursor.col),
                element_bounds.origin.y + metrics.line_height * row,
            ),
            size: size(metrics.cell_width, metrics.line_height),
        })
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
