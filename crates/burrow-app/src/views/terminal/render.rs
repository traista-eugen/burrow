//! Drawing the grid (D-042).
//!
//! A terminal's text is monospace and sits on a fixed grid, so it isn't shaped a line at a time:
//! each character's glyph is looked up once per font style (shaping that one character, which
//! also finds a fallback font for it) and painted at its cell. Only cells of several characters,
//! such as a letter with combining marks, are shaped as text. Underlines, undercurls and
//! strikethroughs are drawn per run of cells.
//!
//! Each line is laid out once per version: its background spans, glyphs and decorations. A frame
//! paints what is on screen from those layouts, so a frame where one line changed lays out one
//! line. GPUI rebuilds the window's scene every frame, so painting itself isn't incremental; the
//! session view embeds the terminal as a cached view, so frames for other parts of the window
//! reuse the terminal's last scene.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, BorderStyle, Bounds, ContentMask, Font, FontFallbacks, FontId, FontStyle, FontWeight,
    GlyphId, Hitbox, Hsla, Pixels, Point, Rgba, ShapedLine, SharedString, StrikethroughStyle,
    TextAlign, TextRun, UnderlineStyle, Window, fill, outline, point, px, rgb, size,
    underline_y_offset,
};

use super::grid::{CursorShape, Flags, Line};
use super::metrics::CellSize;
use super::palette::TerminalTheme;

/// The fonts tried, in order, for the terminal: Windows Terminal's font, then the one every
/// Windows has.
pub const FONT_FAMILIES: [&str; 2] = ["Cascadia Mono", "Consolas"];

pub fn hsla(hex: u32) -> Hsla {
    Hsla::from(rgb(hex))
}

pub fn hsla_alpha(hex: u32, alpha: f32) -> Hsla {
    let mut color: Rgba = rgb(hex);
    color.a = alpha;
    Hsla::from(color)
}

/// The font and the cell size it gives.
#[derive(Clone, Debug)]
pub struct CellMetrics {
    /// Regular, bold, italic, bold italic.
    fonts: [Font; 4],
    pub font_size: Pixels,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    /// The regular font's ascent and descent, as GPUI lays text out.
    ascent: Pixels,
    descent: Pixels,
    /// The window's scale factor these were measured at.
    pub scale: f32,
}

impl CellMetrics {
    /// Measures `family` at `font_size`: the cell is the advance of `m` wide and the font's
    /// ascent plus descent high, rounded up to whole device pixels so rows don't blur or gap.
    pub fn measure(family: &str, font_size: Pixels, window: &Window) -> Self {
        let font = |weight, style| Font {
            family: SharedString::from(family.to_owned()),
            features: Default::default(),
            fallbacks: Some(FontFallbacks::from_fonts(
                FONT_FAMILIES.iter().map(|f| (*f).to_owned()).collect(),
            )),
            weight,
            style,
        };
        let fonts = [
            font(FontWeight::NORMAL, FontStyle::Normal),
            font(FontWeight::BOLD, FontStyle::Normal),
            font(FontWeight::NORMAL, FontStyle::Italic),
            font(FontWeight::BOLD, FontStyle::Italic),
        ];
        let text_system = window.text_system();
        let id = text_system.resolve_font(&fonts[0]);
        let cell_width = text_system
            .advance(id, font_size, 'm')
            .map(|advance| advance.width)
            .unwrap_or(font_size * 0.6);
        let sample = text_system.shape_line(
            SharedString::from("m"),
            font_size,
            &[TextRun {
                len: 1,
                font: fonts[0].clone(),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let (ascent, descent) = (sample.ascent, sample.descent);
        let scale = window.scale_factor();
        let line_height = px((f32::from(ascent + descent) * scale).ceil() / scale);
        Self {
            fonts,
            font_size,
            cell_width,
            line_height,
            ascent,
            descent,
            scale,
        }
    }

    pub fn cell_size(&self) -> CellSize {
        CellSize {
            width: self.cell_width.into(),
            height: self.line_height.into(),
        }
    }

    fn variant(flags: Flags) -> usize {
        usize::from(flags.contains(Flags::BOLD)) + 2 * usize::from(flags.contains(Flags::ITALIC))
    }

    fn font(&self, flags: Flags) -> &Font {
        &self.fonts[Self::variant(flags)]
    }

    /// From the top of a row to the text's baseline.
    fn baseline(&self) -> Pixels {
        (self.line_height - self.ascent - self.descent) / 2. + self.ascent
    }

    /// From the top of a row to an underline.
    fn underline(&self) -> Pixels {
        underline_y_offset(self.line_height, self.ascent, self.descent)
    }

    /// From the top of a row to a strikethrough: GPUI's own place for it.
    fn strikethrough(&self) -> Pixels {
        (self.ascent * 0.5 + self.baseline()) * 0.5
    }
}

/// The first of [`FONT_FAMILIES`] installed.
pub fn pick_family(window: &Window) -> &'static str {
    let installed = window.text_system().all_font_names();
    FONT_FAMILIES
        .iter()
        .find(|family| installed.iter().any(|name| name == *family))
        .copied()
        .unwrap_or(FONT_FAMILIES[FONT_FAMILIES.len() - 1])
}

/// One character's glyph, in whichever font has it.
#[derive(Clone, Copy, Debug)]
struct Glyph {
    font_id: FontId,
    id: GlyphId,
    emoji: bool,
}

/// A glyph placed in a line.
struct PlacedGlyph {
    col: usize,
    glyph: Glyph,
    color: Hsla,
}

#[derive(Clone, Copy, PartialEq)]
enum DecorationKind {
    Underline,
    Undercurl,
    Strikethrough,
}

/// A line under, through or curling under a span of cells.
struct Decoration {
    col: usize,
    cols: usize,
    kind: DecorationKind,
    color: Hsla,
}

/// A line, ready to paint.
pub struct LaidOutLine {
    /// Spans with their own background: first column, columns, colour.
    backgrounds: Vec<(usize, usize, Hsla)>,
    glyphs: Vec<PlacedGlyph>,
    decorations: Vec<Decoration>,
    /// Cells of several characters, shaped, at their columns.
    shaped: Vec<(usize, ShapedLine)>,
}

/// Laid-out lines by version, kept while they are on screen, and glyphs by character.
#[derive(Default)]
pub struct LineCache {
    lines: HashMap<u64, Rc<LaidOutLine>>,
    /// Each character's glyph per font style; `None` when it doesn't shape to one glyph.
    glyphs: HashMap<(char, usize), Option<Glyph>>,
    /// Lines laid out by the last frame: cache misses.
    pub misses: usize,
}

impl LineCache {
    /// The layouts for `lines` (version and line), reusing what the last frame had. Layouts of
    /// lines no longer shown are dropped.
    pub fn frame<'a>(
        &mut self,
        lines: impl Iterator<Item = Option<(u64, &'a Line)>>,
        theme: &TerminalTheme,
        metrics: &CellMetrics,
        window: &Window,
    ) -> Vec<Option<Rc<LaidOutLine>>> {
        let mut previous = std::mem::take(&mut self.lines);
        self.misses = 0;
        let mut frame = Vec::new();
        for line in lines {
            let Some((version, line)) = line else {
                frame.push(None);
                continue;
            };
            let laid_out = match previous
                .remove(&version)
                .or_else(|| self.lines.get(&version).cloned())
            {
                Some(laid_out) => laid_out,
                None => {
                    self.misses += 1;
                    Rc::new(self.lay_out(line, theme, metrics, window))
                }
            };
            self.lines.insert(version, laid_out.clone());
            frame.push(Some(laid_out));
        }
        frame
    }

    /// Forgets every layout and glyph, after the font or colours change.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.glyphs.clear();
    }

    /// The glyph for one character in one font style, shaping it the first time.
    fn glyph(
        &mut self,
        ch: char,
        flags: Flags,
        metrics: &CellMetrics,
        window: &Window,
    ) -> Option<Glyph> {
        let variant = CellMetrics::variant(flags);
        *self.glyphs.entry((ch, variant)).or_insert_with(|| {
            let shaped = shape(
                &ch.to_string(),
                flags,
                Hsla::default(),
                None,
                metrics,
                window,
            );
            let mut glyphs = shaped.runs.iter().flat_map(|run| {
                run.glyphs.iter().map(move |glyph| Glyph {
                    font_id: run.font_id,
                    id: glyph.id,
                    emoji: glyph.is_emoji,
                })
            });
            match (glyphs.next(), glyphs.next()) {
                (Some(glyph), None) => Some(glyph),
                _ => None,
            }
        })
    }

    /// Lays out one line: merges its backgrounds and decorations into spans, and places a glyph
    /// in each cell that has a character to draw.
    fn lay_out(
        &mut self,
        line: &Line,
        theme: &TerminalTheme,
        metrics: &CellMetrics,
        window: &Window,
    ) -> LaidOutLine {
        let mut backgrounds: Vec<(usize, usize, u32)> = Vec::new();
        let mut glyphs = Vec::new();
        let mut decorations: Vec<Decoration> = Vec::new();
        let mut shaped = Vec::new();
        // Colours are converted once per run of one style.
        let mut last_fg: Option<(u32, Hsla)> = None;

        for cell in line.cells() {
            let colors = theme.resolve(cell.style);
            let flags = cell.style.flags;
            let width = cell.width.max(1);
            if let Some(bg) = colors.bg
                && cell.width > 0
            {
                match backgrounds.last_mut() {
                    Some((col, cols, color)) if *color == bg && *col + *cols == cell.col => {
                        *cols += cell.width;
                    }
                    _ => backgrounds.push((cell.col, cell.width, bg)),
                }
            }
            let color = match last_fg {
                Some((fg, color)) if fg == colors.fg => color,
                _ => {
                    let color = hsla(colors.fg);
                    last_fg = Some((colors.fg, color));
                    color
                }
            };

            let kinds = [
                (flags.contains(Flags::UNDERCURL), DecorationKind::Undercurl),
                (
                    flags.contains(Flags::UNDERLINE) && !flags.contains(Flags::UNDERCURL),
                    DecorationKind::Underline,
                ),
                (
                    flags.contains(Flags::STRIKETHROUGH),
                    DecorationKind::Strikethrough,
                ),
            ];
            for (on, kind) in kinds {
                if !on || colors.hidden {
                    continue;
                }
                match decorations
                    .iter_mut()
                    .rev()
                    .find(|d| d.kind == kind && d.col + d.cols == cell.col && d.color == color)
                {
                    Some(decoration) => decoration.cols += width,
                    None => decorations.push(Decoration {
                        col: cell.col,
                        cols: width,
                        kind,
                        color,
                    }),
                }
            }

            if colors.hidden || cell.text == " " {
                continue;
            }
            let mut chars = cell.text.chars();
            let single = match (chars.next(), chars.next()) {
                (Some(ch), None) => Some(ch),
                _ => None,
            };
            match single.and_then(|ch| self.glyph(ch, flags, metrics, window)) {
                Some(glyph) => glyphs.push(PlacedGlyph {
                    col: cell.col,
                    glyph,
                    color,
                }),
                None => shaped.push((
                    cell.col,
                    shape(cell.text, flags, color, None, metrics, window),
                )),
            }
        }

        LaidOutLine {
            backgrounds: backgrounds
                .into_iter()
                .map(|(col, cols, color)| (col, cols, hsla(color)))
                .collect(),
            glyphs,
            decorations,
            shaped,
        }
    }
}

/// Shapes `text` in one style.
fn shape(
    text: &str,
    flags: Flags,
    color: Hsla,
    underline: Option<UnderlineStyle>,
    metrics: &CellMetrics,
    window: &Window,
) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: metrics.font(flags).clone(),
        color,
        background_color: None,
        underline,
        strikethrough: None,
    };
    window.text_system().shape_line(
        SharedString::from(text.to_owned()),
        metrics.font_size,
        &[run],
        None,
    )
}

/// The cursor, as drawn this frame.
pub struct CursorPaint {
    pub bounds: Bounds<Pixels>,
    pub shape: CursorShape,
    pub focused: bool,
    pub color: Hsla,
    /// The character under a focused block cursor, in the cursor's text colour.
    pub text: Option<ShapedLine>,
}

/// IME text being composed, drawn at the cursor.
pub struct MarkedPaint {
    pub bounds: Bounds<Pixels>,
    pub background: Hsla,
    pub text: ShapedLine,
}

/// Everything one frame paints, worked out in prepaint.
pub struct Frame {
    pub bounds: Bounds<Pixels>,
    pub metrics: CellMetrics,
    pub rows: Vec<Option<Rc<LaidOutLine>>>,
    /// Selected cells per row: row, first column, columns.
    pub selection: Vec<(usize, usize, usize)>,
    pub selection_color: Hsla,
    pub cursor: Option<CursorPaint>,
    pub marked: Option<MarkedPaint>,
    pub hitbox: Hitbox,
    /// How long prepaint took, and how many lines it laid out.
    pub prepaint: Duration,
    pub laid_out: usize,
}

impl Frame {
    pub fn paint(&self, window: &mut Window, cx: &mut App) {
        let metrics = &self.metrics;
        let origin = self.bounds.origin;
        let (cw, lh) = (metrics.cell_width, metrics.line_height);
        let at = |row: usize, col: usize| -> Point<Pixels> {
            point(origin.x + cw * col as f32, origin.y + lh * row as f32)
        };
        let cells = |row: usize, col: usize, cols: usize| Bounds {
            origin: at(row, col),
            size: size(cw * cols as f32, lh),
        };
        window.with_content_mask(
            Some(ContentMask {
                bounds: self.bounds,
            }),
            |window| {
                let lines = || {
                    self.rows
                        .iter()
                        .enumerate()
                        .filter_map(|(row, line)| Some((row, line.as_ref()?)))
                };
                // Inside a paint layer, primitives share a draw order instead of each being placed
                // among the others, which costs far more for thousands of glyphs (D-042).
                // Backgrounds and the selection in one layer, then the text in one per row; the
                // cursor goes over both.
                window.paint_layer(self.bounds, |window| {
                    for (row, line) in lines() {
                        for (col, cols, color) in &line.backgrounds {
                            window.paint_quad(fill(cells(row, *col, *cols), *color));
                        }
                    }
                    for (row, col, cols) in &self.selection {
                        window.paint_quad(fill(cells(*row, *col, *cols), self.selection_color));
                    }
                });
                self.paint_text(window, cx);
                if let Some(cursor) = &self.cursor {
                    paint_cursor(cursor, window, cx);
                }
                if let Some(marked) = &self.marked {
                    window.paint_quad(fill(marked.bounds, marked.background));
                    let _ = marked.text.paint(
                        marked.bounds.origin,
                        lh,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            },
        );
    }

    /// Paints every row's glyphs, shaped cells and decorations.
    fn paint_text(&self, window: &mut Window, cx: &mut App) {
        let metrics = &self.metrics;
        let origin = self.bounds.origin;
        let (cw, lh) = (metrics.cell_width, metrics.line_height);
        let at = |row: usize, col: usize| -> Point<Pixels> {
            point(origin.x + cw * col as f32, origin.y + lh * row as f32)
        };
        let baseline = metrics.baseline();
        let lines = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(row, line)| Some((row, line.as_ref()?)));
        for (row, line) in lines {
            let row_bounds = Bounds {
                origin: at(row, 0),
                size: size(self.bounds.size.width, lh),
            };
            window.paint_layer(row_bounds, |window| {
                for placed in &line.glyphs {
                    let mut origin = at(row, placed.col);
                    origin.y += baseline;
                    let glyph = placed.glyph;
                    // Painting only fails when a glyph can't be rasterized; it is skipped then.
                    let _ = if glyph.emoji {
                        window.paint_emoji(origin, glyph.font_id, glyph.id, metrics.font_size)
                    } else {
                        window.paint_glyph(
                            origin,
                            glyph.font_id,
                            glyph.id,
                            metrics.font_size,
                            placed.color,
                        )
                    };
                }
                for (col, shaped) in &line.shaped {
                    let _ = shaped.paint(at(row, *col), lh, TextAlign::Left, None, window, cx);
                }
                for decoration in &line.decorations {
                    let mut origin = at(row, decoration.col);
                    let width = cw * decoration.cols as f32;
                    match decoration.kind {
                        DecorationKind::Underline | DecorationKind::Undercurl => {
                            origin.y += metrics.underline();
                            window.paint_underline(
                                origin,
                                width,
                                &UnderlineStyle {
                                    thickness: px(1.0),
                                    color: Some(decoration.color),
                                    wavy: decoration.kind == DecorationKind::Undercurl,
                                },
                            );
                        }
                        DecorationKind::Strikethrough => {
                            origin.y += metrics.strikethrough();
                            window.paint_strikethrough(
                                origin,
                                width,
                                &StrikethroughStyle {
                                    thickness: px(1.0),
                                    color: Some(decoration.color),
                                },
                            );
                        }
                    }
                }
            });
        }
    }
}

fn paint_cursor(cursor: &CursorPaint, window: &mut Window, cx: &mut App) {
    let bounds = cursor.bounds;
    let thickness = px(2.0);
    match (cursor.shape, cursor.focused) {
        (CursorShape::Block, true) => {
            window.paint_quad(fill(bounds, cursor.color));
            if let Some(text) = &cursor.text {
                let _ = text.paint(
                    bounds.origin,
                    bounds.size.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }
        (CursorShape::Block, false) => {
            window.paint_quad(outline(bounds, cursor.color, BorderStyle::Solid));
        }
        (CursorShape::Underline, _) => {
            window.paint_quad(fill(
                Bounds {
                    origin: point(
                        bounds.origin.x,
                        bounds.origin.y + bounds.size.height - thickness,
                    ),
                    size: size(bounds.size.width, thickness),
                },
                cursor.color,
            ));
        }
        (CursorShape::Bar, _) => {
            window.paint_quad(fill(
                Bounds {
                    origin: bounds.origin,
                    size: size(thickness, bounds.size.height),
                },
                cursor.color,
            ));
        }
    }
}

/// Shapes one cell's text, for the character under a block cursor.
pub fn shape_cell(
    text: &str,
    flags: Flags,
    color: Hsla,
    metrics: &CellMetrics,
    window: &Window,
) -> ShapedLine {
    shape(text, flags, color, None, metrics, window)
}

/// Shapes IME composition text, underlined.
pub fn shape_marked(text: &str, color: Hsla, metrics: &CellMetrics, window: &Window) -> ShapedLine {
    let underline = UnderlineStyle {
        thickness: px(1.0),
        color: None,
        wavy: false,
    };
    shape(
        text,
        Flags::empty(),
        color,
        Some(underline),
        metrics,
        window,
    )
}

/// How long recent frames took to draw the terminal, for measuring.
#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    /// Prepaint (layout) plus paint, per frame, newest last.
    samples: VecDeque<Duration>,
    /// Lines laid out (cache misses) per frame, alongside.
    laid_out: VecDeque<usize>,
}

/// A summary of [`FrameStats`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameSummary {
    pub frames: usize,
    pub mean: Duration,
    pub p95: Duration,
    pub max: Duration,
    /// Lines laid out per frame, on average.
    pub laid_out: f64,
}

impl FrameStats {
    const KEEP: usize = 2_000;

    pub fn record(&mut self, took: Duration, laid_out: usize) {
        if self.samples.len() == Self::KEEP {
            self.samples.pop_front();
            self.laid_out.pop_front();
        }
        self.samples.push_back(took);
        self.laid_out.push_back(laid_out);
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.laid_out.clear();
    }

    pub fn summary(&self) -> FrameSummary {
        if self.samples.is_empty() {
            return FrameSummary::default();
        }
        let mut sorted: Vec<Duration> = self.samples.iter().copied().collect();
        sorted.sort();
        let n = sorted.len();
        FrameSummary {
            frames: n,
            mean: sorted.iter().sum::<Duration>() / n as u32,
            p95: sorted[(n * 95 / 100).min(n - 1)],
            max: sorted[n - 1],
            laid_out: self.laid_out.iter().sum::<usize>() as f64 / n as f64,
        }
    }
}
