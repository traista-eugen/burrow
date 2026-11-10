//! One terminal's emulator: `alacritty_terminal`'s `Term` fed with the program's output, and the
//! screen rendered into protocol lines (D-039).
//!
//! [`Screen`] knows nothing about processes or clients. [`Screen::feed`] parses output and
//! returns the replies the program asked for (cursor position reports, colour queries), which
//! the caller writes back to it. [`Screen::frame`] renders the screen as a [`Frame`]: rows that
//! didn't change keep their `Arc`, so comparing two frames row by row is cheap, and
//! [`Screen::diff`] turns two frames into a `terminal/update`.
//!
//! # Counting scrolled lines
//!
//! An update says how many lines scrolled into the scrollback since the client's previous one,
//! and scrollback pages are addressed relative to the screen the client has. Both need the
//! number of lines that ever scrolled off the top, which `alacritty_terminal` doesn't keep. So
//! the screen counts them itself (`top`): it remembers the heap address of the primary screen's
//! top row, and after each piece of output finds that row again in the scrollback. Rows keep
//! their buffers while they scroll, so the row found `k` lines up means `k` lines scrolled.
//! Output is fed in pieces of at most [`FEED_PIECE`] bytes, fewer lines than the scrollback
//! holds, so the remembered row can't scroll out and be reused within one piece.
//!
//! When the count can't be kept (a resize rewraps every line, the program cleared the
//! scrollback, or the row wasn't found), the screen starts a new epoch: clients get a `reset`
//! and page the scrollback afresh.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape as AnsiCursorShape, NamedColor, Processor, Rgb, StdSyncHandler,
};
use burrow_protocol::{
    Color, CursorShape, MouseTracking, TerminalCursor, TerminalLine, TerminalModes, TerminalRun,
    TerminalScrollbackResult, TerminalSize, TerminalUpdate,
};

/// Scrollback lines kept per terminal.
pub const SCROLLBACK: usize = 10_000;
/// The most output parsed between two line counts; well under [`SCROLLBACK`] lines.
pub const FEED_PIECE: usize = 4096;
/// The cell size reported to programs that ask for the window's size in pixels.
const CELL_WIDTH: u16 = 8;
const CELL_HEIGHT: u16 = 16;
/// The colours reported to programs that ask (OSC 10, 11, 4): a dark theme, since the client's
/// own theme isn't known here.
const DEFAULT_FG: Rgb = Rgb {
    r: 0xcc,
    g: 0xcc,
    b: 0xcc,
};
const DEFAULT_BG: Rgb = Rgb {
    r: 0x1e,
    g: 0x1e,
    b: 0x1e,
};

/// A rendered line: its runs, and whether it wraps onto the next.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Content {
    pub runs: Vec<TerminalRun>,
    pub wrapped: bool,
}

impl Content {
    fn is_blank(&self) -> bool {
        self.runs.is_empty() && !self.wrapped
    }

    fn line(&self, index: i64) -> TerminalLine {
        TerminalLine {
            index,
            runs: self.runs.clone(),
            wrapped: self.wrapped,
        }
    }
}

/// A rendered row, shared between frames while it doesn't change.
pub type Rendered = Arc<Content>;

/// The screen at one moment, as clients see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    pub lines: Vec<Rendered>,
    pub cursor: TerminalCursor,
    pub modes: TerminalModes,
    pub title: Option<String>,
    /// Lines that scrolled off the top in this epoch.
    pub top: u64,
    /// Bumped whenever scrolled lines can't be counted.
    pub epoch: u64,
    pub scrollback: u64,
    /// How many times the bell rang, ever.
    pub bells: u64,
}

impl Frame {
    /// Row `row` as a protocol line.
    pub fn line(&self, row: usize) -> TerminalLine {
        self.lines[row].line(row as i64)
    }

    /// Every row, as protocol lines.
    pub fn all_lines(&self) -> Vec<TerminalLine> {
        (0..self.lines.len()).map(|row| self.line(row)).collect()
    }
}

/// What the listener heard while output was parsed.
struct Heard {
    replies: Vec<u8>,
    title: Option<String>,
    bells: u64,
    size: WindowSize,
}

/// Collects the emulator's events. It is called while the emulator is borrowed, so it only
/// records; [`Screen::feed`] picks the records up.
#[derive(Clone)]
struct Listener(Arc<Mutex<Heard>>);

impl Listener {
    fn heard(&self) -> std::sync::MutexGuard<'_, Heard> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let mut heard = self.heard();
        match event {
            Event::PtyWrite(text) => heard.replies.extend_from_slice(text.as_bytes()),
            Event::Title(title) => heard.title = Some(title),
            Event::ResetTitle => heard.title = None,
            Event::Bell => heard.bells += 1,
            Event::ColorRequest(index, format) => {
                let reply = format(default_color(index));
                heard.replies.extend_from_slice(reply.as_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                let reply = format(heard.size);
                heard.replies.extend_from_slice(reply.as_bytes());
            }
            // OSC 52: clipboard writes aren't passed on yet, and reads are never answered.
            Event::ClipboardStore(..) | Event::ClipboardLoad(..) => {}
            Event::MouseCursorDirty
            | Event::CursorBlinkingChange
            | Event::Wakeup
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }
}

struct Size(TerminalSize);

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.0.rows)
    }

    fn columns(&self) -> usize {
        usize::from(self.0.cols)
    }
}

/// One terminal's emulator and its rendered screen.
pub struct Screen {
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    listener: Listener,
    size: TerminalSize,
    /// Rendered rows, and which of them changed since.
    rows: Vec<Rendered>,
    dirty: Vec<bool>,
    /// Lines that scrolled off the top in this epoch; see the module docs.
    top: u64,
    epoch: u64,
    /// The address of the primary screen's top row after the last count.
    anchor: usize,
    /// The primary screen's scrollback length after the last count.
    history: usize,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen")
            .field("size", &self.size)
            .field("top", &self.top)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

impl Screen {
    /// A blank screen of `size`.
    pub fn new(size: TerminalSize) -> Self {
        let listener = Listener(Arc::new(Mutex::new(Heard {
            replies: Vec::new(),
            title: None,
            bells: 0,
            size: window_size(size),
        })));
        let config = Config {
            scrolling_history: SCROLLBACK,
            // Programs fall back to the encodings `terminal/input` documents.
            kitty_keyboard: false,
            ..Config::default()
        };
        let term = Term::new(config, &Size(size), listener.clone());
        let rows = usize::from(size.rows);
        let mut screen = Self {
            term,
            parser: Processor::new(),
            listener,
            size,
            rows: vec![Arc::default(); rows],
            dirty: vec![true; rows],
            top: 0,
            epoch: 0,
            anchor: 0,
            history: 0,
        };
        screen.anchor = screen.top_row_address();
        screen
    }

    /// The size.
    pub fn size(&self) -> TerminalSize {
        self.size
    }

    /// Parses the program's output. Returns what the program asked to be told, to write back
    /// to it.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        for piece in bytes.chunks(FEED_PIECE) {
            self.parser.advance(&mut self.term, piece);
            self.after_output();
        }
        std::mem::take(&mut self.listener.heard().replies)
    }

    /// When a synchronized update (mode 2026) that hasn't ended must be shown anyway.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// Ends a synchronized update that timed out, showing what it buffered. Returns replies,
    /// as [`Screen::feed`] does.
    pub fn end_sync(&mut self) -> Vec<u8> {
        if self.sync_deadline().is_some() {
            self.parser.stop_sync(&mut self.term);
            self.after_output();
        }
        std::mem::take(&mut self.listener.heard().replies)
    }

    /// Changes the size. Lines are rewrapped, so a new epoch starts.
    pub fn resize(&mut self, size: TerminalSize) {
        if size == self.size {
            return;
        }
        self.term.resize(Size(size));
        self.size = size;
        self.listener.heard().size = window_size(size);
        let rows = usize::from(size.rows);
        self.rows = vec![Arc::default(); rows];
        self.dirty = vec![true; rows];
        self.new_epoch();
    }

    /// The screen now: renders the rows that changed.
    pub fn frame(&mut self) -> Frame {
        let grid = self.term.grid();
        let colors = self.term.colors();
        for (index, dirty) in self.dirty.iter_mut().enumerate() {
            if !*dirty {
                continue;
            }
            *dirty = false;
            let content = content(&grid[Line(index as i32)], grid.columns(), colors);
            if *self.rows[index] != content {
                self.rows[index] = Arc::new(content);
            }
        }
        let heard = self.listener.heard();
        Frame {
            cols: self.size.cols,
            rows: self.size.rows,
            lines: self.rows.clone(),
            cursor: self.cursor(),
            modes: self.modes(),
            title: heard.title.clone(),
            top: self.top,
            epoch: self.epoch,
            scrollback: self.term.grid().history_size() as u64,
            bells: heard.bells,
        }
    }

    /// Scrollback line `number`: lines are numbered from 0 as they scroll off the top in this
    /// epoch, so line `top - 1` is the one just above the screen. `None` once it isn't kept.
    fn history_line(&self, number: u64) -> Option<Content> {
        let grid = self.term.grid();
        let up = self.top.checked_sub(number)?;
        if up == 0 || up > grid.history_size() as u64 {
            return None;
        }
        Some(content(
            &grid[Line(-(up as i32))],
            grid.columns(),
            self.term.colors(),
        ))
    }

    /// A page of scrollback for a client whose screen is `seen` (a frame it was sent): up to
    /// `limit` lines above index `before`, at indexes relative to that screen. Nothing when the
    /// frame is from another epoch; the client gets a reset next.
    pub fn page(&self, seen: &Frame, before: i64, limit: usize) -> TerminalScrollbackResult {
        if seen.epoch != self.epoch {
            return TerminalScrollbackResult {
                lines: Vec::new(),
                complete: false,
            };
        }
        let history = self.term.grid().history_size() as u64;
        let first = self.top.saturating_sub(history);
        // The client's index -1 is line `seen.top - 1`, the last to scroll off before its top
        // row.
        let end = seen.top.saturating_add_signed(before.min(0)).min(self.top);
        let start = end.saturating_sub(limit as u64).max(first);
        let lines = (start..end)
            .filter_map(|number| {
                Some(
                    self.history_line(number)?
                        .line(number as i64 - seen.top as i64),
                )
            })
            .collect();
        TerminalScrollbackResult {
            lines,
            complete: start <= first,
        }
    }

    /// What changed between `sent` (what a client has) and `now`, as an update for it; `None`
    /// when nothing did. `now` must be this screen's latest frame.
    pub fn diff(&self, terminal_id: &str, sent: &Frame, now: &Frame) -> Option<TerminalUpdate> {
        let reset = sent.epoch != now.epoch || (sent.cols, sent.rows) != (now.cols, now.rows);
        let mut lines = Vec::new();
        let mut scrolled = 0;
        if reset {
            lines = now.all_lines();
        } else {
            scrolled = now.top - sent.top;
            let shift = scrolled as usize;
            // Lines that went into the scrollback, where the client's copy (its old rows, or
            // blank lines it never saw) differs.
            let first = now.top.saturating_sub(now.scrollback);
            for number in sent.top.max(first)..now.top {
                let Some(actual) = self.history_line(number) else {
                    continue;
                };
                let held = sent.lines.get((number - sent.top) as usize);
                let differs = match held {
                    Some(held) => **held != actual,
                    None => !actual.is_blank(),
                };
                if differs {
                    lines.push(actual.line(number as i64 - now.top as i64));
                }
            }
            // Rows that differ after the client moved its rows up.
            for (row, line) in now.lines.iter().enumerate() {
                let differs = match sent.lines.get(row + shift) {
                    Some(held) => !Arc::ptr_eq(held, line) && **held != **line,
                    None => !line.is_blank(),
                };
                if differs {
                    lines.push(now.line(row));
                }
            }
        }
        let update = TerminalUpdate {
            terminal_id: terminal_id.to_owned(),
            reset,
            cols: reset.then_some(now.cols),
            rows: reset.then_some(now.rows),
            scrolled,
            lines,
            cursor: now.cursor.clone(),
            modes: (sent.modes != now.modes).then(|| now.modes.clone()),
            title: (sent.title != now.title).then(|| now.title.clone().unwrap_or_default()),
            scrollback: (sent.scrollback != now.scrollback).then_some(now.scrollback),
            bell: sent.bells != now.bells,
        };
        let changed = reset
            || scrolled > 0
            || !update.lines.is_empty()
            || sent.cursor != now.cursor
            || update.modes.is_some()
            || update.title.is_some()
            || update.scrollback.is_some()
            || update.bell;
        changed.then_some(update)
    }

    /// After each piece of output: marks changed rows, and counts lines that scrolled into the
    /// scrollback.
    fn after_output(&mut self) {
        match self.term.damage() {
            TermDamage::Full => self.dirty.iter_mut().for_each(|dirty| *dirty = true),
            TermDamage::Partial(lines) => {
                for damage in lines {
                    if let Some(dirty) = self.dirty.get_mut(damage.line) {
                        *dirty = true;
                    }
                }
            }
        }
        self.term.reset_damage();
        // The alternate screen has no scrollback; the primary's waits, unchanged, behind it.
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let grid = self.term.grid();
        let history = grid.history_size();
        let scrolled =
            (0..=history).find(|&up| row_address(&grid[Line(-(up as i32))]) == self.anchor);
        match scrolled {
            // The scrollback only shrinks when the program clears it.
            Some(up) if history >= self.history => {
                self.top += up as u64;
                self.anchor = self.top_row_address();
                self.history = history;
            }
            _ => self.new_epoch(),
        }
    }

    /// Starts counting afresh: clients get a reset. The scrollback kept so far is numbered as
    /// the first lines of the new epoch.
    fn new_epoch(&mut self) {
        self.epoch += 1;
        self.top = self.term.grid().history_size() as u64;
        if !self.term.mode().contains(TermMode::ALT_SCREEN) {
            self.anchor = self.top_row_address();
            self.history = self.term.grid().history_size();
        }
    }

    fn top_row_address(&self) -> usize {
        row_address(&self.term.grid()[Line(0)])
    }

    fn cursor(&self) -> TerminalCursor {
        let grid = self.term.grid();
        let point = grid.cursor.point;
        let row = point.line.0.clamp(0, i32::from(self.size.rows) - 1);
        let mut col = point.column.0.min(usize::from(self.size.cols) - 1);
        // On a wide character's second cell, the cursor is on the character.
        if col > 0
            && grid[Line(row)][Column(col)]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
        {
            col -= 1;
        }
        let style = self.term.cursor_style();
        let visible = self.term.mode().contains(TermMode::SHOW_CURSOR)
            && style.shape != AnsiCursorShape::Hidden;
        TerminalCursor {
            row: row as u16,
            col: col as u16,
            visible,
            shape: match style.shape {
                AnsiCursorShape::Underline => CursorShape::Underline,
                AnsiCursorShape::Beam => CursorShape::Bar,
                AnsiCursorShape::Block | AnsiCursorShape::HollowBlock | AnsiCursorShape::Hidden => {
                    CursorShape::Block
                }
            },
        }
    }

    fn modes(&self) -> TerminalModes {
        let mode = *self.term.mode();
        let mouse = if mode.contains(TermMode::MOUSE_MOTION) {
            Some(MouseTracking::Motion)
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            Some(MouseTracking::Drag)
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            Some(MouseTracking::Click)
        } else {
            None
        };
        TerminalModes {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            app_keypad: mode.contains(TermMode::APP_KEYPAD),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            mouse,
            mouse_sgr: mode.contains(TermMode::SGR_MOUSE),
            focus_events: mode.contains(TermMode::FOCUS_IN_OUT),
            alt_screen: mode.contains(TermMode::ALT_SCREEN),
            alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
        }
    }
}

/// One row: its runs, and whether alacritty marked it as wrapping onto the next.
fn content(row: &Row<Cell>, columns: usize, colors: &Colors) -> Content {
    let wrapped = columns > 0 && row[Column(columns - 1)].flags.contains(Flags::WRAPLINE);
    Content {
        runs: render(row, columns, colors),
        wrapped,
    }
}

/// A row's identity: the address of its cells, which stays put while the row scrolls.
fn row_address(row: &Row<Cell>) -> usize {
    std::ptr::from_ref(&row[Column(0)]) as usize
}

fn window_size(size: TerminalSize) -> WindowSize {
    WindowSize {
        num_lines: size.rows,
        num_cols: size.cols,
        cell_width: CELL_WIDTH,
        cell_height: CELL_HEIGHT,
    }
}

/// One row as runs: cells with the same style joined, trailing blank cells dropped.
fn render(row: &Row<Cell>, columns: usize, colors: &Colors) -> Vec<TerminalRun> {
    let mut end = columns.min(row.len());
    while end > 0 && is_blank(&row[Column(end - 1)]) {
        end -= 1;
    }
    let mut runs: Vec<TerminalRun> = Vec::new();
    for column in 0..end {
        let cell = &row[Column(column)];
        // A wide character's second cell holds nothing of its own.
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let (fg, bg, flags) = style(cell, colors);
        let run = match runs.last_mut() {
            Some(run) if run.fg == fg && run.bg == bg && run.flags == flags => run,
            _ => {
                runs.push(TerminalRun {
                    text: String::new(),
                    fg,
                    bg,
                    flags,
                });
                runs.last_mut().expect("just pushed")
            }
        };
        // A wide character that didn't fit leaves an empty last cell.
        if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) || matches!(cell.c, '\t' | '\0') {
            run.text.push(' ');
        } else {
            run.text.push(cell.c);
        }
        if let Some(marks) = cell.zerowidth() {
            run.text.extend(marks);
        }
    }
    runs
}

/// A cell that shows nothing: a space on the default background, with no line drawn through or
/// under it and not inverted.
fn is_blank(cell: &Cell) -> bool {
    let drawn = Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT;
    matches!(cell.c, ' ' | '\t' | '\0')
        && cell.bg == AnsiColor::Named(NamedColor::Background)
        && !cell.flags.intersects(drawn)
        && cell.zerowidth().is_none_or(<[char]>::is_empty)
}

fn style(cell: &Cell, colors: &Colors) -> (Option<Color>, Option<Color>, u16) {
    let flags = cell.flags;
    let mut bits = 0;
    for (flag, bit) in [
        (Flags::BOLD, TerminalRun::BOLD),
        (Flags::ITALIC, TerminalRun::ITALIC),
        (
            Flags::UNDERLINE
                | Flags::DOUBLE_UNDERLINE
                | Flags::DOTTED_UNDERLINE
                | Flags::DASHED_UNDERLINE,
            TerminalRun::UNDERLINE,
        ),
        (Flags::INVERSE, TerminalRun::INVERSE),
        (Flags::DIM, TerminalRun::DIM),
        (Flags::STRIKEOUT, TerminalRun::STRIKETHROUGH),
        (Flags::HIDDEN, TerminalRun::HIDDEN),
        (Flags::UNDERCURL, TerminalRun::UNDERCURL),
    ] {
        if flags.intersects(flag) {
            bits |= bit;
        }
    }
    (color(cell.fg, colors), color(cell.bg, colors), bits)
}

/// A cell colour as the protocol writes it: `None` for the theme's default.
fn color(color: AnsiColor, colors: &Colors) -> Option<Color> {
    let index = match color {
        AnsiColor::Spec(rgb) => return Some(Color::Rgb(rgb.r, rgb.g, rgb.b)),
        AnsiColor::Indexed(index) => index,
        AnsiColor::Named(named) => match named as usize {
            index @ 0..=15 => index as u8,
            _ => match named {
                NamedColor::DimBlack => 0,
                NamedColor::DimRed => 1,
                NamedColor::DimGreen => 2,
                NamedColor::DimYellow => 3,
                NamedColor::DimBlue => 4,
                NamedColor::DimMagenta => 5,
                NamedColor::DimCyan => 6,
                NamedColor::DimWhite => 7,
                // The default foreground and background, and their variants.
                _ => return None,
            },
        },
    };
    // A palette entry the program redefined (OSC 4) is sent as the colour it is now.
    Some(match colors[usize::from(index)] {
        Some(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        None => Color::Indexed(index),
    })
}

/// The colour reported for palette entry or special colour `index` (OSC 4, 10, 11, 12).
fn default_color(index: usize) -> Rgb {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x31, 0x31),
        (0x0d, 0xbc, 0x79),
        (0xe5, 0xe5, 0x10),
        (0x24, 0x72, 0xc8),
        (0xbc, 0x3f, 0xbc),
        (0x11, 0xa8, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x66, 0x66, 0x66),
        (0xf1, 0x4c, 0x4c),
        (0x23, 0xd1, 0x8b),
        (0xf5, 0xf5, 0x43),
        (0x3b, 0x8e, 0xea),
        (0xd6, 0x70, 0xd6),
        (0x29, 0xb8, 0xdb),
        (0xe5, 0xe5, 0xe5),
    ];
    let rgb = |(r, g, b): (u8, u8, u8)| Rgb { r, g, b };
    match index {
        0..=15 => rgb(ANSI[index]),
        16..=231 => {
            let level = |n: usize| if n == 0 { 0 } else { (55 + n * 40) as u8 };
            let n = index - 16;
            rgb((level(n / 36), level(n / 6 % 6), level(n % 6)))
        }
        232..=255 => {
            let grey = (8 + (index - 232) * 10) as u8;
            rgb((grey, grey, grey))
        }
        i if i == NamedColor::Background as usize => DEFAULT_BG,
        _ => DEFAULT_FG,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(cols: u16, rows: u16) -> Screen {
        Screen::new(TerminalSize { cols, rows })
    }

    fn texts(frame: &Frame) -> Vec<String> {
        frame.all_lines().iter().map(TerminalLine::text).collect()
    }

    fn page_texts(screen: &Screen, seen: &Frame, before: i64, limit: usize) -> Vec<(i64, String)> {
        screen
            .page(seen, before, limit)
            .lines
            .iter()
            .map(|line| (line.index, line.text()))
            .collect()
    }

    /// A client's copy of the screen and scrollback, built from updates the way the protocol
    /// says, to check updates against.
    struct Client {
        rows: Vec<TerminalLine>,
        /// `history[0]` is index -1.
        history: Vec<TerminalLine>,
    }

    impl Client {
        fn new(frame: &Frame) -> Self {
            Self {
                rows: frame.all_lines(),
                history: Vec::new(),
            }
        }

        fn apply(&mut self, update: TerminalUpdate) {
            if update.reset {
                self.rows = vec![TerminalLine::default(); usize::from(update.rows.unwrap())];
                self.history.clear();
            }
            for _ in 0..update.scrolled {
                let top = self.rows.remove(0);
                self.history.insert(0, top);
                self.rows.push(TerminalLine::default());
            }
            for line in update.lines {
                if line.index >= 0 {
                    let row = line.index as usize;
                    self.rows[row] = line;
                } else {
                    let at = (-line.index - 1) as usize;
                    if self.history.len() <= at {
                        self.history.resize(at + 1, TerminalLine::default());
                    }
                    self.history[at] = line;
                }
            }
        }

        fn rows(&self) -> Vec<String> {
            self.rows.iter().map(TerminalLine::text).collect()
        }

        fn history(&self) -> Vec<String> {
            self.history.iter().rev().map(TerminalLine::text).collect()
        }
    }

    #[test]
    fn text_colours_and_styles_become_runs() {
        let mut screen = screen(20, 3);
        screen.feed(b"\x1b[1;31mred\x1b[0m plain \x1b[38;2;1;2;3mrgb\x1b[0m\r\n\x1b[7;4mx\x1b[0m");
        let frame = screen.frame();
        let first = frame.line(0);
        assert_eq!(first.text(), "red plain rgb");
        assert_eq!(
            first.runs[0],
            TerminalRun {
                text: "red".into(),
                fg: Some(Color::Indexed(1)),
                bg: None,
                flags: TerminalRun::BOLD,
            }
        );
        assert_eq!(first.runs[1], TerminalRun::plain(" plain "));
        assert_eq!(first.runs[2].fg, Some(Color::Rgb(1, 2, 3)));
        let second = frame.line(1);
        assert_eq!(
            second.runs[0].flags,
            TerminalRun::INVERSE | TerminalRun::UNDERLINE
        );
        assert!(frame.line(2).runs.is_empty());
        assert_eq!(
            frame.cursor,
            TerminalCursor {
                row: 1,
                col: 1,
                visible: true,
                shape: CursorShape::Block
            }
        );
    }

    #[test]
    fn wide_characters_are_written_once_and_a_coloured_background_is_kept() {
        let mut screen = screen(10, 2);
        screen.feed("漢字!\x1b[42m  \x1b[0m".as_bytes());
        let frame = screen.frame();
        let line = frame.line(0);
        assert_eq!(line.runs[0].text, "漢字!");
        assert_eq!(line.runs[1].text, "  ");
        assert_eq!(line.runs[1].bg, Some(Color::Indexed(2)));
        assert_eq!(frame.cursor.col, 7);
    }

    #[test]
    fn soft_wrapped_lines_say_so() {
        let mut screen = screen(5, 3);
        screen.feed(b"abcdefg\r\nhi");
        let frame = screen.frame();
        assert_eq!(texts(&frame), ["abcde", "fg", "hi"]);
        assert!(frame.line(0).wrapped);
        assert!(!frame.line(1).wrapped && !frame.line(2).wrapped);
    }

    #[test]
    fn unchanged_rows_keep_their_arc() {
        let mut screen = screen(10, 3);
        screen.feed(b"a\r\nb\r\nc");
        let before = screen.frame();
        screen.feed(b"\x1b[2;1Hx");
        let after = screen.frame();
        assert!(Arc::ptr_eq(&before.lines[0], &after.lines[0]));
        assert!(!Arc::ptr_eq(&before.lines[1], &after.lines[1]));
        assert_eq!(texts(&after), ["a", "x", "c"]);
    }

    #[test]
    fn updates_carry_only_what_changed() {
        let mut screen = screen(10, 3);
        screen.feed(b"a\r\nb");
        let first = screen.frame();
        assert_eq!(screen.diff("trm_1", &first, &first), None);

        screen.feed(b"c");
        let second = screen.frame();
        let update = screen.diff("trm_1", &first, &second).unwrap();
        let rows: Vec<i64> = update.lines.iter().map(|line| line.index).collect();
        assert_eq!(rows, [1]);
        assert_eq!(update.lines[0].text(), "bc");
        assert_eq!(update.cursor.col, 2);
        assert!(!update.reset && update.scrolled == 0);
        assert!(update.modes.is_none() && update.title.is_none() && !update.bell);

        screen.feed(b"\x1b]0;t\x07\x07\x1b[?2004h");
        let third = screen.frame();
        let update = screen.diff("trm_1", &second, &third).unwrap();
        assert!(update.lines.is_empty());
        assert_eq!(update.title.as_deref(), Some("t"));
        assert!(update.bell);
        assert!(update.modes.unwrap().bracketed_paste);
    }

    #[test]
    fn a_scroll_moves_the_clients_rows_and_sends_only_the_new_ones() {
        let mut screen = screen(10, 4);
        screen.feed(b"1\r\n2\r\n3\r\nstatus");
        let first = screen.frame();
        let mut client = Client::new(&first);
        // Two lines scroll; the status line is rewritten at the bottom.
        screen.feed(b"\r\x1b[K4\r\n5\r\nstatus");
        let second = screen.frame();
        let update = screen.diff("trm_1", &first, &second).unwrap();
        assert_eq!(update.scrolled, 2);
        let sent: Vec<(i64, String)> = update
            .lines
            .iter()
            .map(|line| (line.index, line.text()))
            .collect();
        // Row 0 ("3") came up from below unchanged; rows 1 to 3 differ after the move. The
        // scrolled lines are as the client had them, so no scrollback line is sent.
        assert_eq!(
            sent,
            [(1, "4".into()), (2, "5".into()), (3, "status".into())],
            "{update:?}"
        );
        client.apply(update);
        assert_eq!(client.rows(), texts(&second));
        assert_eq!(client.history(), ["1", "2"]);
        assert_eq!(
            page_texts(&screen, &second, 0, 10),
            [(-2, "1".into()), (-1, "2".into())]
        );
    }

    #[test]
    fn a_row_changed_just_before_it_scrolled_off_is_corrected() {
        let mut screen = screen(10, 2);
        screen.feed(b"old\r\n");
        let first = screen.frame();
        let mut client = Client::new(&first);
        // The top row changes, then scrolls off before the client hears of it.
        screen.feed(b"\x1b[1;1Hnew\x1b[2;1H\r\nx");
        let second = screen.frame();
        let update = screen.diff("trm_1", &first, &second).unwrap();
        assert_eq!(update.scrolled, 1);
        assert!(
            update
                .lines
                .iter()
                .any(|line| line.index == -1 && line.text() == "new")
        );
        client.apply(update);
        assert_eq!(client.history(), ["new"]);
        assert_eq!(client.rows(), texts(&second));
    }

    #[test]
    fn more_lines_than_the_screen_holds_come_as_scrollback_lines() {
        let mut screen = screen(10, 3);
        let first = screen.frame();
        let mut client = Client::new(&first);
        let output: String = (1..=10).map(|n| format!("line {n}\r\n")).collect();
        screen.feed(output.as_bytes());
        let second = screen.frame();
        let update = screen.diff("trm_1", &first, &second).unwrap();
        assert_eq!(update.scrolled, 8);
        client.apply(update);
        assert_eq!(client.rows(), ["line 9", "line 10", ""]);
        let expected: Vec<String> = (1..=8).map(|n| format!("line {n}")).collect();
        assert_eq!(client.history(), expected);
    }

    #[test]
    fn scrollback_pages_are_relative_to_the_clients_screen() {
        let mut screen = screen(10, 3);
        let output: String = (1..=25).map(|n| format!("line {n}\r\n")).collect();
        screen.feed(output.as_bytes());
        let seen = screen.frame();
        // 25 lines and an empty one: 23 scrolled off.
        assert_eq!(texts(&seen), ["line 24", "line 25", ""]);
        assert_eq!(seen.scrollback, 23);

        let page = screen.page(&seen, 0, 5);
        assert!(!page.complete);
        let numbers: Vec<i64> = page.lines.iter().map(|line| line.index).collect();
        assert_eq!(numbers, [-5, -4, -3, -2, -1]);
        assert_eq!(page.lines[4].text(), "line 23");
        let older = screen.page(&seen, -5, 100);
        assert!(older.complete);
        assert_eq!(older.lines.len(), 18);
        assert_eq!(older.lines[0].text(), "line 1");
        assert_eq!(older.lines[0].index, -23);
        let none = screen.page(&seen, -23, 10);
        assert!(none.lines.is_empty() && none.complete);

        // More output scrolls; a client that hasn't heard yet pages from where it is.
        screen.feed(b"more\r\nmore\r\n");
        assert_eq!(page_texts(&screen, &seen, -18, 1), [(-19, "line 5".into())]);
        let now = screen.frame();
        assert_eq!(page_texts(&screen, &now, -20, 1), [(-21, "line 5".into())]);
    }

    #[test]
    fn counting_survives_a_full_scrollback() {
        let mut screen = screen(12, 2);
        let first = screen.frame();
        for round in 0..3 {
            let output: String = (0..5000)
                .map(|n| format!("{}\r\n", round * 5000 + n))
                .collect();
            screen.feed(output.as_bytes());
        }
        // Line n says n: the first line was printed on the top row, so 14,999 scrolled off.
        let frame = screen.frame();
        assert_eq!(frame.scrollback, SCROLLBACK as u64);
        assert_eq!(frame.epoch, first.epoch);
        assert_eq!(frame.top, 14_999);
        assert_eq!(texts(&frame), ["14999", ""]);
        assert_eq!(page_texts(&screen, &frame, 0, 1), [(-1, "14998".into())]);
        let oldest = screen.page(&frame, -9_999, 10);
        assert!(oldest.complete);
        assert_eq!(oldest.lines.len(), 1);
        assert_eq!(oldest.lines[0].text(), "4999");
    }

    #[test]
    fn a_resize_or_a_cleared_scrollback_resets_the_client() {
        let mut screen = screen(10, 3);
        screen.feed(b"1\r\n2\r\n3\r\n4\r\n5");
        let first = screen.frame();
        assert_eq!(first.scrollback, 2);
        screen.feed(b"\x1b[3J");
        let cleared = screen.frame();
        assert_eq!(cleared.scrollback, 0);
        let update = screen.diff("trm_1", &first, &cleared).unwrap();
        assert!(update.reset);
        assert_eq!(update.lines.len(), 3);

        screen.resize(TerminalSize { cols: 8, rows: 4 });
        let resized = screen.frame();
        let update = screen.diff("trm_1", &cleared, &resized).unwrap();
        assert!(update.reset);
        assert_eq!((update.cols, update.rows), (Some(8), Some(4)));
        assert_eq!(update.lines.len(), 4);
        // A page asked for from before the reset is empty, but not complete.
        let stale = screen.page(&cleared, 0, 10);
        assert!(stale.lines.is_empty() && !stale.complete);
    }

    #[test]
    fn the_alternate_screen_has_no_scrollback_and_leaving_it_keeps_the_count() {
        let mut screen = screen(10, 2);
        screen.feed(b"a\r\nb\r\nc\r\n");
        let main = screen.frame();
        assert_eq!((main.top, main.scrollback), (2, 2));
        screen.feed(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?1hALT\r\n\r\n\r\n");
        let alt = screen.frame();
        assert!(alt.modes.alt_screen && alt.modes.bracketed_paste && alt.modes.app_cursor);
        assert_eq!(alt.modes.mouse, Some(MouseTracking::Click));
        assert!(alt.modes.mouse_sgr);
        assert_eq!((alt.top, alt.scrollback, alt.epoch), (2, 0, main.epoch));
        screen.feed(b"\x1b[?1049l\x1b[?1000l\x1b[?2004l");
        let back = screen.frame();
        assert_eq!((back.top, back.scrollback, back.epoch), (2, 2, main.epoch));
        assert!(!back.modes.alt_screen && !back.modes.bracketed_paste);
        let update = screen.diff("trm_1", &main, &back);
        assert!(update.is_none_or(|update| !update.reset && update.scrolled == 0));
    }

    #[test]
    fn queries_are_answered_and_titles_and_bells_are_heard() {
        let mut screen = screen(10, 2);
        let replies = screen.feed(b"ab\x1b[6n\x1b]11;?\x07\x1b]0;hello\x07\x07");
        let replies = String::from_utf8(replies).unwrap();
        assert!(replies.starts_with("\x1b[1;3R"), "{replies:?}");
        assert!(replies.contains("]11;rgb:1e1e/1e1e/1e1e"), "{replies:?}");
        let frame = screen.frame();
        assert_eq!(frame.title.as_deref(), Some("hello"));
        assert_eq!(frame.bells, 1);
    }

    #[test]
    fn a_synchronized_update_waits_for_its_end_or_its_timeout() {
        let mut screen = screen(10, 2);
        screen.feed(b"\x1b[?2026hhidden");
        assert!(screen.sync_deadline().is_some());
        assert_eq!(texts(&screen.frame()), ["", ""]);
        screen.end_sync();
        assert_eq!(texts(&screen.frame()), ["hidden", ""]);
        screen.feed(b"\x1b[?2026h!\x1b[?2026l");
        assert_eq!(texts(&screen.frame()), ["hidden!", ""]);
        assert!(screen.sync_deadline().is_none());
    }

    #[test]
    fn default_colours_follow_xterm() {
        assert_eq!(default_color(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            default_color(231),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(default_color(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(default_color(257), DEFAULT_BG);
        assert_eq!(default_color(256), DEFAULT_FG);
    }
}
