//! The terminal's plain model: the screen `burrowd` emulates, as lines of styled runs, and the
//! scrollback above it. No GPUI here (D-030).
//!
//! Lines are addressed by index: `0` is the top row of the screen, `rows - 1` the bottom, and
//! negative indices are scrollback, `-1` being the line just above row 0. An update replaces whole
//! lines by index. Every stored line carries a version, new each time it is replaced and kept when
//! it only moves (into scrollback, say), so a view can cache whatever it derives from a line.

use std::collections::VecDeque;
use std::ops::{BitOr, BitOrAssign};

use unicode_width::UnicodeWidthChar as _;

/// How many lines of scrollback the model keeps by default. Older lines are dropped, and can be
/// asked for again.
pub const DEFAULT_HISTORY_LIMIT: usize = 10_000;

/// A colour: one of the 256 palette entries, or RGB. Entries 0–15 are the themeable ANSI colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    /// Parses `#rrggbb`.
    pub fn from_hex(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#')?;
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        Some(Self::Rgb(byte(0)?, byte(2)?, byte(4)?))
    }
}

/// A run's style flags, as the line format's `flags` bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Flags(u16);

impl Flags {
    pub const BOLD: Self = Self(1);
    pub const ITALIC: Self = Self(2);
    pub const UNDERLINE: Self = Self(4);
    pub const INVERSE: Self = Self(8);
    pub const DIM: Self = Self(16);
    pub const STRIKETHROUGH: Self = Self(32);
    pub const HIDDEN: Self = Self(64);
    pub const UNDERCURL: Self = Self(128);

    /// No flags.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Flags from their bits. Unknown bits are kept, and ignored.
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Whether every flag in `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Flags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitOrAssign for Flags {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// A run's style. Absent colours are the terminal's default foreground and background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub flags: Flags,
}

/// A piece of a line with one style.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Run {
    pub text: String,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub flags: Flags,
}

impl Run {
    /// Unstyled text.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    pub fn flags(mut self, flags: Flags) -> Self {
        self.flags = flags;
        self
    }

    pub fn style(&self) -> Style {
        Style {
            fg: self.fg,
            bg: self.bg,
            flags: self.flags,
        }
    }
}

/// One line of the screen or the scrollback.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Line {
    pub runs: Vec<Run>,
}

impl Line {
    pub fn new(runs: Vec<Run>) -> Self {
        Self { runs }
    }

    /// One unstyled run.
    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(vec![Run::new(text)])
    }

    /// The line's text, without styles.
    pub fn text(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }

    /// How many cells the line's text takes.
    pub fn width(&self) -> usize {
        self.cells().last().map_or(0, |cell| cell.col + cell.width)
    }

    /// The line's cells, left to right. A wide character is one cell of width 2; zero-width
    /// characters (combining marks, variation selectors) belong to the cell before them.
    pub fn cells(&self) -> Vec<Cell<'_>> {
        let mut cells: Vec<Cell<'_>> = Vec::new();
        let mut col = 0;
        for (run_index, run) in self.runs.iter().enumerate() {
            let style = run.style();
            for (at, ch) in run.text.char_indices() {
                let width = char_width(ch);
                let end = at + ch.len_utf8();
                if width == 0 {
                    // Joins the cell before it when that cell is in the same run, so the cell's
                    // text stays one slice. At the start of a run it stands alone, drawn over the
                    // cell before it.
                    match cells.last_mut() {
                        Some(last) if last.run == run_index => {
                            last.text = &run.text[last.start..end];
                        }
                        last => {
                            let col = last.map_or(0, |last| last.col);
                            cells.push(Cell {
                                col,
                                width: 0,
                                text: &run.text[at..end],
                                style,
                                run: run_index,
                                start: at,
                            });
                        }
                    }
                    continue;
                }
                cells.push(Cell {
                    col,
                    width,
                    text: &run.text[at..end],
                    style,
                    run: run_index,
                    start: at,
                });
                col += width;
            }
        }
        cells
    }
}

/// One cell of a line: a character and any zero-width characters after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell<'a> {
    /// The first column it takes.
    pub col: usize,
    /// How many columns it takes: 1, 2 for a wide character, or 0 for a zero-width character
    /// with no character before it in its run.
    pub width: usize,
    pub text: &'a str,
    pub style: Style,
    /// Which run it is in, and where in the run's text it starts.
    run: usize,
    start: usize,
}

/// How many cells a character takes: 0, 1 or 2. Control characters take none.
///
/// Per character, as `alacritty_terminal` measures them, so the columns agree with `burrowd`'s.
pub fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(0)
}

/// The cursor's shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

/// Where the cursor is, on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cursor {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
    pub shape: CursorShape,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            visible: true,
            shape: CursorShape::Block,
        }
    }
}

/// Which mouse events the program asked for (DECSET 1000, 1002, 1003).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MouseMode {
    /// None: the mouse selects text.
    #[default]
    Off,
    /// Presses, releases and the wheel.
    Click,
    /// Those, and moves while a button is held.
    Drag,
    /// Those, and every move.
    Motion,
}

/// The terminal modes that change what input is sent, as the emulator reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modes {
    /// Application cursor keys (DECCKM): arrows, Home and End send `ESC O x`.
    pub app_cursor: bool,
    /// Bracketed paste (2004): pasted text is wrapped in `ESC [200~` … `ESC [201~`.
    pub bracketed_paste: bool,
    /// Mouse reporting.
    pub mouse: MouseMode,
    /// SGR mouse encoding (1006); otherwise the original X10 encoding.
    pub mouse_sgr: bool,
    /// The alternate screen is showing (1049); it has no scrollback.
    pub alt_screen: bool,
    /// On the alternate screen without mouse reporting, the wheel sends arrow keys (1007).
    pub alternate_scroll: bool,
    /// Focus changes are reported as `ESC [I` and `ESC [O` (1004).
    pub focus_reporting: bool,
}

/// A line and its index: `0` is the top row of the screen, negative indices are scrollback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedLine {
    pub index: i64,
    pub line: Line,
}

impl IndexedLine {
    pub fn new(index: i64, line: Line) -> Self {
        Self { index, line }
    }
}

/// The whole screen, as `terminal/attach` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    pub cursor: Cursor,
    /// Every line of the screen; missing ones are blank. Negative indices are scrollback.
    pub lines: Vec<IndexedLine>,
}

/// The lines that changed, as `terminal/update` sends them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub lines: Vec<IndexedLine>,
    pub cursor: Option<Cursor>,
    /// How many lines scrolled off the top of the screen into scrollback before `lines` apply.
    /// The screen's lines move up by as many, keeping their versions; the host then sends the
    /// new bottom lines. If more lines scrolled than the screen holds, the ones the client never
    /// saw come as negative indices in `lines`, or stay blank.
    pub scrolled: usize,
    /// The new size, if the terminal was resized.
    pub size: Option<(u16, u16)>,
}

/// What a change did, for the view: what to redraw, and how to move a selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Lines that moved from the screen into scrollback.
    pub scrolled: usize,
    /// The first and last screen rows replaced.
    pub rows: Option<(u16, u16)>,
    /// Scrollback lines were replaced or added.
    pub history: bool,
    /// The size changed.
    pub resized: bool,
}

impl Changes {
    fn touch_row(&mut self, row: u16) {
        self.rows = Some(match self.rows {
            Some((first, last)) => (first.min(row), last.max(row)),
            None => (row, row),
        });
    }

    /// Whether anything on the screen or in scrollback changed.
    pub fn any(&self) -> bool {
        self.scrolled > 0 || self.rows.is_some() || self.history || self.resized
    }
}

#[derive(Clone, Debug, Default)]
struct Slot {
    line: Line,
    version: u64,
}

/// The screen and its scrollback.
#[derive(Clone, Debug)]
pub struct TerminalGrid {
    cols: u16,
    rows: u16,
    screen: Vec<Slot>,
    /// Scrollback, newest first: `history[0]` is line `-1`.
    history: VecDeque<Slot>,
    history_limit: usize,
    /// No older scrollback exists.
    history_complete: bool,
    cursor: Cursor,
    modes: Modes,
    next_version: u64,
}

impl TerminalGrid {
    /// A blank screen.
    pub fn new(cols: u16, rows: u16) -> Self {
        let mut grid = Self {
            cols: cols.max(1),
            rows: rows.max(1),
            screen: Vec::new(),
            history: VecDeque::new(),
            history_limit: DEFAULT_HISTORY_LIMIT,
            history_complete: false,
            cursor: Cursor::default(),
            modes: Modes::default(),
            next_version: 1,
        };
        grid.screen = (0..grid.rows).map(|_| grid.blank()).collect();
        grid
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn modes(&self) -> Modes {
        self.modes
    }

    pub fn set_modes(&mut self, modes: Modes) {
        self.modes = modes;
    }

    /// How many scrollback lines are loaded.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Whether there is no older scrollback than what is loaded.
    pub fn history_complete(&self) -> bool {
        self.history_complete
    }

    /// The index of the oldest line loaded: `-history_len`, or `0` with no scrollback.
    pub fn oldest_index(&self) -> i64 {
        -(self.history.len() as i64)
    }

    /// Keeps at most `limit` lines of scrollback.
    pub fn set_history_limit(&mut self, limit: usize) {
        self.history_limit = limit;
        self.trim_history();
    }

    /// The line at `index`, if loaded.
    pub fn line(&self, index: i64) -> Option<&Line> {
        self.slot(index).map(|slot| &slot.line)
    }

    /// The version of the line at `index`: new whenever the line is replaced.
    pub fn line_version(&self, index: i64) -> Option<u64> {
        self.slot(index).map(|slot| slot.version)
    }

    fn slot(&self, index: i64) -> Option<&Slot> {
        if index >= 0 {
            self.screen.get(usize::try_from(index).ok()?)
        } else {
            self.history.get(usize::try_from(-index - 1).ok()?)
        }
    }

    fn blank(&mut self) -> Slot {
        self.slot_for(Line::default())
    }

    fn slot_for(&mut self, line: Line) -> Slot {
        let version = self.next_version;
        self.next_version += 1;
        Slot { line, version }
    }

    /// Replaces everything with a snapshot. Scrollback is dropped, apart from any lines the
    /// snapshot carries at negative indices; older lines can be asked for again.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) -> Changes {
        self.cols = snapshot.cols.max(1);
        self.rows = snapshot.rows.max(1);
        self.cursor = snapshot.cursor;
        self.screen = (0..self.rows).map(|_| self.blank()).collect();
        self.history.clear();
        self.history_complete = false;
        for IndexedLine { index, line } in snapshot.lines {
            self.set_line(index, line);
        }
        Changes {
            scrolled: 0,
            rows: Some((0, self.rows - 1)),
            history: true,
            resized: true,
        }
    }

    /// Applies an update: a resize, then a scroll, then the replaced lines and the cursor.
    pub fn apply_update(&mut self, update: Update) -> Changes {
        let mut changes = Changes::default();
        if let Some((cols, rows)) = update.size {
            changes.resized = self.resize(cols, rows);
        }
        if update.scrolled > 0 {
            self.scroll_up(update.scrolled);
            changes.scrolled = update.scrolled;
            changes.history = true;
        }
        for IndexedLine { index, line } in update.lines {
            if self.set_line(index, line) {
                match u16::try_from(index) {
                    Ok(row) => changes.touch_row(row),
                    Err(_) => changes.history = true,
                }
            }
        }
        if let Some(cursor) = update.cursor {
            self.cursor = cursor;
        }
        self.trim_history();
        changes
    }

    /// Adds a page of older scrollback, at negative indices. `complete`: nothing older exists.
    pub fn apply_history(&mut self, lines: Vec<IndexedLine>, complete: bool) -> Changes {
        for IndexedLine { index, line } in lines {
            if index < 0 {
                self.set_line(index, line);
            }
        }
        self.history_complete = complete;
        self.trim_history();
        Changes {
            history: true,
            ..Changes::default()
        }
    }

    /// Stores `line` at `index`. Rows below the screen are ignored; scrollback grows as needed,
    /// up to its limit. Returns whether it was stored.
    fn set_line(&mut self, index: i64, line: Line) -> bool {
        if index >= 0 {
            let Ok(row) = usize::try_from(index) else {
                return false;
            };
            if row >= self.screen.len() {
                return false;
            }
            self.screen[row] = self.slot_for(line);
            return true;
        }
        let Ok(at) = usize::try_from(-index - 1) else {
            return false;
        };
        if at >= self.history_limit {
            return false;
        }
        while self.history.len() <= at {
            let blank = self.blank();
            self.history.push_back(blank);
        }
        self.history[at] = self.slot_for(line);
        true
    }

    /// Moves the top `count` screen lines into scrollback and blanks the bottom.
    fn scroll_up(&mut self, count: usize) {
        let rows = self.screen.len();
        let moved = count.min(rows);
        let leaving: Vec<Slot> = self.screen.drain(..moved).collect();
        for slot in leaving {
            self.history.push_front(slot);
        }
        // Lines that scrolled past the screen without ever being on it here are the newest,
        // blank until the host sends them.
        for _ in moved..count {
            let blank = self.blank();
            self.history.push_front(blank);
        }
        for _ in 0..moved {
            let blank = self.blank();
            self.screen.push(blank);
        }
    }

    /// Changes the size: rows are added or dropped at the bottom; lines keep their content. The
    /// host sends the reflowed lines. Returns whether the size changed.
    fn resize(&mut self, cols: u16, rows: u16) -> bool {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if (cols, rows) == (self.cols, self.rows) {
            return false;
        }
        self.cols = cols;
        self.rows = rows;
        let rows = usize::from(rows);
        self.screen.truncate(rows);
        while self.screen.len() < rows {
            let blank = self.blank();
            self.screen.push(blank);
        }
        true
    }

    fn trim_history(&mut self) {
        if self.history.len() > self.history_limit {
            self.history.truncate(self.history_limit);
            // Dropped lines still exist on the host.
            self.history_complete = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(cols: u16, rows: u16, lines: &[(i64, &str)]) -> Snapshot {
        Snapshot {
            cols,
            rows,
            cursor: Cursor::default(),
            lines: lines
                .iter()
                .map(|(index, text)| IndexedLine::new(*index, Line::plain(*text)))
                .collect(),
        }
    }

    fn texts(grid: &TerminalGrid, range: std::ops::Range<i64>) -> Vec<String> {
        range
            .map(|index| grid.line(index).map(Line::text).unwrap_or_default())
            .collect()
    }

    #[test]
    fn colours_parse_from_hex() {
        assert_eq!(Color::from_hex("#ff8000"), Some(Color::Rgb(255, 128, 0)));
        assert_eq!(Color::from_hex("#FF8000"), Some(Color::Rgb(255, 128, 0)));
        assert_eq!(Color::from_hex("ff8000"), None);
        assert_eq!(Color::from_hex("#ff80"), None);
        assert_eq!(Color::from_hex("#gg8000"), None);
        assert_eq!(Color::from_hex("#ff80é"), None);
    }

    #[test]
    fn flags_combine() {
        let flags = Flags::BOLD | Flags::UNDERLINE;
        assert_eq!(flags.bits(), 5);
        assert!(flags.contains(Flags::BOLD));
        assert!(!flags.contains(Flags::ITALIC));
        assert!(Flags::from_bits(128).contains(Flags::UNDERCURL));
    }

    #[test]
    fn cells_measure_display_width() {
        let line = Line::new(vec![
            Run::new("a界"),
            Run::new("e\u{301}x").flags(Flags::BOLD),
            Run::new("\u{301}b"),
        ]);
        let cells = line.cells();
        let summary: Vec<_> = cells.iter().map(|c| (c.col, c.width, c.text)).collect();
        assert_eq!(
            summary,
            vec![
                (0, 1, "a"),
                (1, 2, "界"),
                (3, 1, "e\u{301}"),
                (4, 1, "x"),
                // A combining mark at the start of a run stands alone, over the cell before it.
                (4, 0, "\u{301}"),
                (5, 1, "b"),
            ]
        );
        assert_eq!(cells[3].style.flags, Flags::BOLD);
        assert_eq!(line.width(), 6);
        assert_eq!(line.text(), "a界e\u{301}x\u{301}b");
        assert_eq!(Line::default().width(), 0);
    }

    #[test]
    fn snapshots_replace_everything() {
        let mut grid = TerminalGrid::new(80, 24);
        grid.apply_update(Update {
            scrolled: 3,
            ..Update::default()
        });
        assert_eq!(grid.history_len(), 3);

        let changes = grid.apply_snapshot(snapshot(10, 3, &[(0, "one"), (2, "three"), (5, "x")]));
        assert_eq!((grid.cols(), grid.rows()), (10, 3));
        assert_eq!(texts(&grid, 0..3), ["one", "", "three"]);
        // Rows past the screen are ignored, and scrollback is dropped.
        assert_eq!(grid.line(3), None);
        assert_eq!(grid.history_len(), 0);
        assert!(!grid.history_complete());
        assert!(changes.resized);
        assert_eq!(changes.rows, Some((0, 2)));

        // A snapshot may carry scrollback.
        grid.apply_snapshot(snapshot(10, 2, &[(-2, "older"), (-1, "old"), (0, "now")]));
        assert_eq!(texts(&grid, -2..2), ["older", "old", "now", ""]);
        assert_eq!(grid.oldest_index(), -2);
    }

    #[test]
    fn updates_replace_lines_by_index() {
        let mut grid = TerminalGrid::new(10, 3);
        grid.apply_snapshot(snapshot(10, 3, &[(0, "a"), (1, "b"), (2, "c")]));
        let versions: Vec<_> = (0..3).map(|i| grid.line_version(i).unwrap()).collect();

        let changes = grid.apply_update(Update {
            lines: vec![IndexedLine::new(1, Line::plain("B"))],
            cursor: Some(Cursor {
                row: 1,
                col: 1,
                visible: false,
                shape: CursorShape::Bar,
            }),
            ..Update::default()
        });
        assert_eq!(texts(&grid, 0..3), ["a", "B", "c"]);
        assert_eq!(changes.rows, Some((1, 1)));
        assert_eq!(changes.scrolled, 0);
        assert_eq!(grid.cursor().shape, CursorShape::Bar);
        assert!(!grid.cursor().visible);
        // Only the replaced line has a new version.
        assert_eq!(grid.line_version(0), Some(versions[0]));
        assert_ne!(grid.line_version(1), Some(versions[1]));
        assert_eq!(grid.line_version(2), Some(versions[2]));

        // An update without a cursor keeps it.
        grid.apply_update(Update {
            lines: vec![IndexedLine::new(7, Line::plain("ignored"))],
            ..Update::default()
        });
        assert_eq!(grid.cursor().col, 1);
        assert_eq!(texts(&grid, 0..3), ["a", "B", "c"]);
    }

    #[test]
    fn scrolling_moves_lines_into_history() {
        let mut grid = TerminalGrid::new(10, 3);
        grid.apply_snapshot(snapshot(10, 3, &[(0, "a"), (1, "b"), (2, "c")]));
        let version_b = grid.line_version(1).unwrap();

        let changes = grid.apply_update(Update {
            scrolled: 2,
            lines: vec![
                IndexedLine::new(1, Line::plain("d")),
                IndexedLine::new(2, Line::plain("e")),
            ],
            ..Update::default()
        });
        assert_eq!(texts(&grid, -2..3), ["a", "b", "c", "d", "e"]);
        assert_eq!(changes.scrolled, 2);
        assert_eq!(changes.rows, Some((1, 2)));
        // A line that only moved keeps its version.
        assert_eq!(grid.line_version(-1), Some(version_b));

        // More lines than the screen holds: the ones never seen arrive at negative indices.
        grid.apply_update(Update {
            scrolled: 5,
            lines: vec![
                IndexedLine::new(-2, Line::plain("f")),
                IndexedLine::new(-1, Line::plain("g")),
                IndexedLine::new(0, Line::plain("h")),
                IndexedLine::new(1, Line::plain("i")),
                IndexedLine::new(2, Line::plain("j")),
            ],
            ..Update::default()
        });
        assert_eq!(
            texts(&grid, -7..3),
            ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]
        );
    }

    #[test]
    fn history_pages_extend_upwards() {
        let mut grid = TerminalGrid::new(10, 2);
        grid.apply_snapshot(snapshot(10, 2, &[(-1, "h1"), (0, "s0"), (1, "s1")]));
        assert_eq!(grid.oldest_index(), -1);

        let changes = grid.apply_history(
            vec![
                IndexedLine::new(-3, Line::plain("h3")),
                IndexedLine::new(-2, Line::plain("h2")),
                // A screen line in a history page is ignored.
                IndexedLine::new(0, Line::plain("nope")),
            ],
            false,
        );
        assert!(changes.history);
        assert_eq!(texts(&grid, -3..2), ["h3", "h2", "h1", "s0", "s1"]);
        assert!(!grid.history_complete());

        grid.apply_history(vec![IndexedLine::new(-4, Line::plain("h4"))], true);
        assert_eq!(grid.oldest_index(), -4);
        assert!(grid.history_complete());
    }

    #[test]
    fn history_is_limited() {
        let mut grid = TerminalGrid::new(10, 2);
        grid.set_history_limit(3);
        grid.apply_history(vec![IndexedLine::new(-1, Line::plain("h1"))], true);
        assert!(grid.history_complete());
        grid.apply_snapshot(snapshot(10, 2, &[(0, "a"), (1, "b")]));
        for text in ["c", "d", "e", "f"] {
            grid.apply_update(Update {
                scrolled: 1,
                lines: vec![IndexedLine::new(1, Line::plain(text))],
                ..Update::default()
            });
        }
        assert_eq!(grid.history_len(), 3);
        assert_eq!(texts(&grid, -3..2), ["b", "c", "d", "e", "f"]);
        // Lines past the limit are dropped, not stored.
        grid.apply_history(vec![IndexedLine::new(-9, Line::plain("x"))], true);
        assert_eq!(grid.history_len(), 3);
    }

    #[test]
    fn resizing_keeps_lines_and_reports_it() {
        let mut grid = TerminalGrid::new(10, 3);
        grid.apply_snapshot(snapshot(10, 3, &[(0, "a"), (1, "b"), (2, "c")]));
        let changes = grid.apply_update(Update {
            size: Some((20, 2)),
            ..Update::default()
        });
        assert!(changes.resized);
        assert_eq!((grid.cols(), grid.rows()), (20, 2));
        assert_eq!(texts(&grid, 0..3), ["a", "b", ""]);
        assert_eq!(grid.line(2), None);

        grid.apply_update(Update {
            size: Some((20, 4)),
            ..Update::default()
        });
        assert_eq!(texts(&grid, 0..4), ["a", "b", "", ""]);
        // The same size again is no change.
        assert!(
            !grid
                .apply_update(Update {
                    size: Some((20, 4)),
                    ..Update::default()
                })
                .any()
        );
    }
}
