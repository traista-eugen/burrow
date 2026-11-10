//! Sizes and positions: how many cells fit an element, which cell the pointer is over, and how
//! far the view is scrolled back. Plain numbers in logical pixels, so they can be tested.

use super::selection::Side;

/// The smallest grid the terminal asks for, as `alacritty_terminal` allows.
pub const MIN_COLS: u16 = 2;
pub const MIN_ROWS: u16 = 1;

/// One cell's size, in logical pixels, taken from the font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellSize {
    pub width: f32,
    pub height: f32,
}

/// The grid that fits `width` × `height`: whole cells only, and at least [`MIN_COLS`] ×
/// [`MIN_ROWS`].
pub fn grid_size(width: f32, height: f32, cell: CellSize) -> (u16, u16) {
    let fit = |space: f32, size: f32, min: u16| {
        if size <= 0.0 || !space.is_finite() {
            return min;
        }
        // A hair of tolerance, so a size that is an exact multiple doesn't lose a cell to
        // rounding.
        let count = ((space + 0.01) / size).floor();
        (count.clamp(0.0, f32::from(u16::MAX)) as u16).max(min)
    };
    (
        fit(width, cell.width, MIN_COLS),
        fit(height, cell.height, MIN_ROWS),
    )
}

/// The cell under a point relative to the grid's top-left corner, clamped to the grid, and which
/// half of it the point is over.
pub fn cell_at(x: f32, y: f32, cell: CellSize, cols: u16, rows: u16) -> (u16, u16, Side) {
    let clamp = |value: f32, count: u16| value.clamp(0.0, f32::from(count.max(1) - 1)) as u16;
    let col_f = x / cell.width;
    let col = clamp(col_f.floor(), cols);
    let row = clamp((y / cell.height).floor(), rows);
    let side = if x < 0.0 {
        Side::Left
    } else if col_f >= f32::from(cols) {
        Side::Right
    } else if col_f - col_f.floor() < 0.5 {
        Side::Left
    } else {
        Side::Right
    };
    (col, row, side)
}

/// How far the view is scrolled back into scrollback, in lines. Row `r` of the view shows line
/// `r - offset`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollPosition {
    /// Lines scrolled back; 0 is the bottom, following the screen.
    pub offset: usize,
    /// Part of a line left over from smooth (pixel) scrolling.
    remainder: f32,
}

impl ScrollPosition {
    /// Scrolls by `lines` (positive is back, towards older lines), within the `history` lines
    /// loaded. Returns whether the offset changed.
    pub fn scroll_by(&mut self, lines: f32, history: usize) -> bool {
        let total = self.remainder + lines;
        let whole = total.trunc();
        self.remainder = total - whole;
        let before = self.offset;
        let target = self.offset as f64 + f64::from(whole);
        self.offset = target.clamp(0.0, history as f64) as usize;
        if self.offset == 0 || self.offset == history {
            self.remainder = 0.0;
        }
        self.offset != before
    }

    /// Back to the bottom.
    pub fn reset(&mut self) -> bool {
        let moved = self.offset != 0;
        *self = Self::default();
        moved
    }

    /// The line row `row` of the view shows.
    pub fn line_at(&self, row: u16) -> i64 {
        i64::from(row) - self.offset as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: CellSize = CellSize {
        width: 8.4,
        height: 18.0,
    };

    #[test]
    fn whole_cells_fit() {
        assert_eq!(grid_size(840.0, 360.0, CELL), (100, 20));
        assert_eq!(grid_size(847.0, 377.0, CELL), (100, 20));
        // An exact multiple that rounding would lose.
        assert_eq!(grid_size(8.4 * 3.0, 18.0 * 3.0, CELL), (3, 3));
        // A 200×60 grid at a common size.
        assert_eq!(grid_size(1680.0, 1080.0, CELL), (200, 60));
        // Never smaller than 2×1.
        assert_eq!(grid_size(0.0, 0.0, CELL), (2, 1));
        assert_eq!(grid_size(-50.0, 5.0, CELL), (2, 1));
        assert_eq!(
            grid_size(
                100.0,
                100.0,
                CellSize {
                    width: 0.0,
                    height: 0.0
                }
            ),
            (2, 1)
        );
    }

    #[test]
    fn points_map_to_cells_and_halves() {
        assert_eq!(cell_at(0.0, 0.0, CELL, 80, 24), (0, 0, Side::Left));
        assert_eq!(cell_at(4.0, 17.9, CELL, 80, 24), (0, 0, Side::Left));
        assert_eq!(cell_at(4.3, 18.0, CELL, 80, 24), (0, 1, Side::Right));
        assert_eq!(cell_at(84.1, 40.0, CELL, 80, 24), (10, 2, Side::Left));
        // Outside the grid: clamped, on the outer side.
        assert_eq!(cell_at(-5.0, -5.0, CELL, 80, 24), (0, 0, Side::Left));
        assert_eq!(
            cell_at(10_000.0, 10_000.0, CELL, 80, 24),
            (79, 23, Side::Right)
        );
    }

    #[test]
    fn scrolling_stays_within_history() {
        let mut scroll = ScrollPosition::default();
        assert!(scroll.scroll_by(3.0, 100));
        assert_eq!(scroll.offset, 3);
        assert_eq!(scroll.line_at(0), -3);
        assert_eq!(scroll.line_at(5), 2);
        // Fractions add up.
        assert!(!scroll.scroll_by(0.6, 100));
        assert!(scroll.scroll_by(0.6, 100));
        assert_eq!(scroll.offset, 4);
        // Clamped at both ends.
        scroll.scroll_by(1_000.0, 100);
        assert_eq!(scroll.offset, 100);
        scroll.scroll_by(-1_000.0, 100);
        assert_eq!(scroll.offset, 0);
        assert!(!scroll.scroll_by(-1.0, 100));
        scroll.scroll_by(5.0, 2);
        assert_eq!(scroll.offset, 2);
        assert!(scroll.reset());
        assert!(!scroll.reset());
    }
}
