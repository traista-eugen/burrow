//! Selecting text with the mouse, and copying it.
//!
//! A selection runs between two points in the grid, each on one side of a cell, as a mouse drag
//! leaves them: a cell is selected once the pointer has crossed its middle. Double and triple
//! clicks select words and lines, and dragging after them extends by words or lines. A selection
//! always takes whole wide characters.

use super::grid::{Line, TerminalGrid};

/// A cell's position: a line index (negative for scrollback) and a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GridPoint {
    pub line: i64,
    pub col: usize,
}

impl GridPoint {
    pub fn new(line: i64, col: usize) -> Self {
        Self { line, col }
    }
}

/// Which half of a cell the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    Left,
    Right,
}

/// What a selection grows by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    /// Cells, from a drag.
    Cells,
    /// Words, from a double click.
    Words,
    /// Lines, from a triple click.
    Lines,
}

/// A selection in progress or made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub kind: SelectionKind,
    anchor: (GridPoint, Side),
    head: (GridPoint, Side),
}

impl Selection {
    /// A selection started at `point`. A cell selection is empty until it is extended.
    pub fn new(kind: SelectionKind, point: GridPoint, side: Side) -> Self {
        Self {
            kind,
            anchor: (point, side),
            head: (point, side),
        }
    }

    /// Moves the end that follows the pointer.
    pub fn extend(&mut self, point: GridPoint, side: Side) {
        self.head = (point, side);
    }

    /// Follows the grid's lines as `lines` of them scroll into scrollback.
    pub fn scroll(&mut self, lines: usize) {
        let lines = lines as i64;
        self.anchor.0.line -= lines;
        self.head.0.line -= lines;
    }

    /// The first and last lines it touches, before expanding to words.
    pub fn lines(&self) -> (i64, i64) {
        let (a, b) = (self.anchor.0.line, self.head.0.line);
        (a.min(b), a.max(b))
    }

    /// The cells selected, or `None` if none are.
    pub fn range(&self, grid: &TerminalGrid) -> Option<SelectionRange> {
        let cols = usize::from(grid.cols());
        let (start, end) = if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        let (mut first, mut last) = match self.kind {
            SelectionKind::Cells => {
                if start == end {
                    return None;
                }
                let mut first = start.0;
                if start.1 == Side::Right {
                    first.col += 1;
                    if first.col >= cols {
                        first = GridPoint::new(first.line + 1, 0);
                    }
                }
                let mut last = end.0;
                if end.1 == Side::Left {
                    if last.col == 0 {
                        last = GridPoint::new(last.line - 1, cols.saturating_sub(1));
                    } else {
                        last.col -= 1;
                    }
                }
                (first, last)
            }
            SelectionKind::Words => {
                let line = |point: GridPoint| grid.line(point.line);
                let (first, _) = word_at(line(start.0), start.0.col, cols);
                let (_, last) = word_at(line(end.0), end.0.col, cols);
                (
                    GridPoint::new(start.0.line, first),
                    GridPoint::new(end.0.line, last),
                )
            }
            SelectionKind::Lines => (
                GridPoint::new(start.0.line, 0),
                GridPoint::new(end.0.line, cols.saturating_sub(1)),
            ),
        };
        if first > last {
            return None;
        }
        // Whole wide characters.
        if let Some(cell) = cell_span(grid.line(first.line), first.col) {
            first.col = cell.0;
        }
        if let Some(cell) = cell_span(grid.line(last.line), last.col) {
            last.col = cell.1;
        }
        Some(SelectionRange {
            start: first,
            end: last,
        })
    }
}

/// The columns a cell at `col` takes, first and last, if a character is there.
fn cell_span(line: Option<&Line>, col: usize) -> Option<(usize, usize)> {
    line?
        .cells()
        .iter()
        .find(|cell| cell.width > 0 && cell.col <= col && col < cell.col + cell.width)
        .map(|cell| (cell.col, cell.col + cell.width - 1))
}

/// Selected cells, from `start` to `end` inclusive, in reading order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: GridPoint,
    pub end: GridPoint,
}

impl SelectionRange {
    /// The columns selected on `line`, first and last, if any. `cols` is the grid's width.
    pub fn columns(&self, line: i64, cols: usize) -> Option<(usize, usize)> {
        if line < self.start.line || line > self.end.line || cols == 0 {
            return None;
        }
        let first = if line == self.start.line {
            self.start.col
        } else {
            0
        };
        let last = if line == self.end.line {
            self.end.col
        } else {
            cols - 1
        };
        (first <= last).then_some((first, last))
    }

    /// The selected text: one line per grid line, without trailing spaces, joined by `\n`.
    pub fn text(&self, grid: &TerminalGrid) -> String {
        let cols = usize::from(grid.cols());
        let mut lines = Vec::new();
        for index in self.start.line..=self.end.line {
            let mut text = String::new();
            if let (Some(line), Some((first, last))) = (grid.line(index), self.columns(index, cols))
            {
                for cell in line.cells() {
                    let end = cell.col + cell.width.max(1) - 1;
                    if cell.col <= last && end >= first {
                        text.push_str(cell.text);
                    }
                }
            }
            lines.push(text.trim_end_matches(' ').to_owned());
        }
        lines.join("\n")
    }
}

/// Characters that end a word, besides whitespace: brackets, quotes and the like, but not `:`,
/// `/`, `\`, `.`, `-` or `=`, so paths, URLs and `file.rs:12` select whole.
const WORD_SEPARATORS: &str = ",│`|\"'()[]{}<>;";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Separator,
    Word,
}

fn class_of(text: &str) -> Class {
    match text.chars().next() {
        None => Class::Space,
        Some(ch) if ch.is_whitespace() => Class::Space,
        Some(ch) if WORD_SEPARATORS.contains(ch) => Class::Separator,
        Some(_) => Class::Word,
    }
}

/// The word around column `col`: the columns of the run of word characters, or of spaces, it is
/// in. A separator is a word of its own.
pub fn word_at(line: Option<&Line>, col: usize, cols: usize) -> (usize, usize) {
    let cols = cols.max(col + 1);
    // Each column's class; a wide character fills both of its columns.
    let mut classes = vec![Class::Space; cols];
    if let Some(line) = line {
        for cell in line.cells() {
            let class = class_of(cell.text);
            let end = (cell.col + cell.width).min(cols);
            if let Some(columns) = classes.get_mut(cell.col..end) {
                columns.fill(class);
            }
        }
    }
    let class = classes[col];
    if class == Class::Separator {
        return cell_span(line, col).unwrap_or((col, col));
    }
    let mut first = col;
    while first > 0 && classes[first - 1] == class {
        first -= 1;
    }
    let mut last = col;
    while last + 1 < cols && classes[last + 1] == class {
        last += 1;
    }
    (first, last)
}

#[cfg(test)]
mod tests {
    use super::super::grid::{Cursor, IndexedLine, Run, Snapshot};
    use super::*;

    fn grid(cols: u16, lines: &[&str]) -> TerminalGrid {
        let mut grid = TerminalGrid::new(cols, lines.len() as u16);
        grid.apply_snapshot(Snapshot {
            cols,
            rows: lines.len() as u16,
            cursor: Cursor::default(),
            lines: lines
                .iter()
                .enumerate()
                .map(|(i, text)| IndexedLine::new(i as i64, Line::plain(*text)))
                .collect(),
        });
        grid
    }

    fn drag(grid: &TerminalGrid, from: (i64, usize, Side), to: (i64, usize, Side)) -> String {
        let mut selection =
            Selection::new(SelectionKind::Cells, GridPoint::new(from.0, from.1), from.2);
        selection.extend(GridPoint::new(to.0, to.1), to.2);
        selection
            .range(grid)
            .map(|range| range.text(grid))
            .unwrap_or_default()
    }

    #[test]
    fn dragging_selects_cells_crossed_past_their_middle() {
        let grid = grid(20, &["hello world", "second line   ", "third"]);
        use Side::{Left, Right};
        // From the left of "w" to the right of "d".
        assert_eq!(drag(&grid, (0, 6, Left), (0, 10, Right)), "world");
        // From the right half of "o" only starts after it.
        assert_eq!(drag(&grid, (0, 4, Right), (0, 10, Right)), " world");
        // Ending on the left half of a cell stops before it.
        assert_eq!(drag(&grid, (0, 0, Left), (0, 4, Left)), "hell");
        // Backwards gives the same.
        assert_eq!(drag(&grid, (0, 10, Right), (0, 6, Left)), "world");
        // A click, or a move within one half, selects nothing.
        assert_eq!(drag(&grid, (0, 3, Left), (0, 3, Left)), "");
        // Within one cell, from its left half to its right half, selects it.
        assert_eq!(drag(&grid, (0, 3, Left), (0, 3, Right)), "l");
    }

    #[test]
    fn multi_line_selections_trim_trailing_spaces() {
        let grid = grid(20, &["hello world", "second line   ", "third"]);
        assert_eq!(
            drag(&grid, (0, 6, Side::Left), (2, 2, Side::Right)),
            "world\nsecond line\nthi"
        );
        // Past the end of the text, on empty columns.
        assert_eq!(
            drag(&grid, (1, 7, Side::Left), (1, 19, Side::Right)),
            "line"
        );
        // Ending at the left edge of a line ends at the end of the line before.
        assert_eq!(drag(&grid, (0, 6, Side::Left), (1, 0, Side::Left)), "world");
    }

    #[test]
    fn wide_characters_are_selected_whole() {
        // "a界b": 界 takes columns 1 and 2.
        let grid = grid(10, &["a界b", "界界"]);
        // Starting on its second half takes it whole.
        assert_eq!(drag(&grid, (0, 2, Side::Left), (0, 3, Side::Right)), "界b");
        // Ending on its first half too.
        assert_eq!(drag(&grid, (0, 0, Side::Left), (0, 1, Side::Right)), "a界");
        assert_eq!(drag(&grid, (1, 1, Side::Left), (1, 2, Side::Right)), "界界");
        let mut selection = Selection::new(SelectionKind::Cells, GridPoint::new(0, 2), Side::Left);
        selection.extend(GridPoint::new(0, 2), Side::Right);
        let range = selection.range(&grid).unwrap();
        assert_eq!((range.start.col, range.end.col), (1, 2));
        assert_eq!(range.text(&grid), "界");
    }

    #[test]
    fn double_clicks_select_words() {
        let grid = grid(
            40,
            &["cargo build --release", "see src/views/mod.rs:12 (now)"],
        );
        let word = |line: i64, col: usize| {
            Selection::new(SelectionKind::Words, GridPoint::new(line, col), Side::Left)
                .range(&grid)
                .unwrap()
                .text(&grid)
        };
        assert_eq!(word(0, 8), "build");
        assert_eq!(word(0, 6), "build");
        assert_eq!(word(0, 14), "--release");
        assert_eq!(word(1, 10), "src/views/mod.rs:12");
        assert_eq!(word(1, 24), "(");
        assert_eq!(word(1, 26), "now");
        // Spaces select the run of spaces, which copies as nothing.
        assert_eq!(word(0, 5), "");
        // Dragging after a double click extends by words.
        let mut selection = Selection::new(SelectionKind::Words, GridPoint::new(0, 7), Side::Left);
        selection.extend(GridPoint::new(0, 15), Side::Left);
        assert_eq!(
            selection.range(&grid).unwrap().text(&grid),
            "build --release"
        );
    }

    #[test]
    fn words_count_wide_characters() {
        let line = Line::new(vec![Run::new("ab 日本語 cd")]);
        // 日本語 takes columns 3–8.
        assert_eq!(word_at(Some(&line), 5, 20), (3, 8));
        assert_eq!(word_at(Some(&line), 0, 20), (0, 1));
        // Past the text: the trailing spaces, up to the edge.
        assert_eq!(word_at(Some(&line), 15, 20), (12, 19));
        assert_eq!(word_at(None, 3, 10), (0, 9));
    }

    #[test]
    fn triple_clicks_select_lines() {
        let grid = grid(20, &["first   ", "second", "third"]);
        let mut selection = Selection::new(SelectionKind::Lines, GridPoint::new(0, 3), Side::Left);
        assert_eq!(selection.range(&grid).unwrap().text(&grid), "first");
        selection.extend(GridPoint::new(1, 0), Side::Left);
        assert_eq!(selection.range(&grid).unwrap().text(&grid), "first\nsecond");
    }

    #[test]
    fn selections_follow_scrolling_and_report_their_columns() {
        let grid = grid(10, &["abc", "def"]);
        let mut selection = Selection::new(SelectionKind::Cells, GridPoint::new(1, 0), Side::Left);
        selection.extend(GridPoint::new(2, 1), Side::Right);
        selection.scroll(1);
        assert_eq!(selection.lines(), (0, 1));
        let range = selection.range(&grid).unwrap();
        assert_eq!(range.text(&grid), "abc\nde");
        assert_eq!(range.columns(0, 10), Some((0, 9)));
        assert_eq!(range.columns(1, 10), Some((0, 1)));
        assert_eq!(range.columns(2, 10), None);
    }
}
