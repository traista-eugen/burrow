//! Terminals: the `terminal/*` methods and notifications, and the screen they carry (M4).
//!
//! `burrowd` runs the emulator; clients only draw a grid of cells. A screen is a list of
//! [`TerminalLine`]s, each a list of [`TerminalRun`]s: a piece of text in one style. Several
//! clients can attach to one terminal; each gets a [`TerminalSnapshot`] from `terminal/attach`,
//! then [`TerminalUpdate`]s with only the lines that changed since its previous one.
//!
//! # Lines and cells
//!
//! - A line's `index` says where it is: `0` is the top row of the screen, `rows - 1` the bottom
//!   row, and negative indexes are the scrollback above the screen: `-1` is the line just above
//!   the top row. Scrollback indexes are relative to the screen as the client last got it, so
//!   they move as lines scroll ([`TerminalUpdate::scrolled`]).
//! - A line's runs cover its cells from the left, in order. They may end before the last
//!   column: the rest of the row is blank, in the default colours. A blank line has no runs.
//! - A wide character, such as `漢` or most emoji, occupies two cells. Its run's text holds it
//!   once, and the client measures its display width (Unicode East Asian Width, as
//!   `unicode-width` 0.2 does). Combining marks follow the character they belong to.
//!
//! # Colours
//!
//! A [`Color`] is a palette index (`0`–`255`) or `"#rrggbb"`. Indexes `0`–`15` are the ANSI
//! colours, which the client's theme decides; `16`–`255` are xterm's 6×6×6 cube and grey ramp.
//! An absent `fg` or `bg` is the theme's default foreground or background.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A colour: a palette index, or a 24-bit colour written `"#rrggbb"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    /// A palette index. `0`–`15` are the themeable ANSI colours: black, red, green, yellow,
    /// blue, magenta, cyan, white, then their bright forms.
    Indexed(u8),
    /// A 24-bit colour.
    Rgb(u8, u8, u8),
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Indexed(index) => write!(f, "{index}"),
            Self::Rgb(r, g, b) => write!(f, "#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Indexed(index) => serializer.serialize_u8(*index),
            Self::Rgb(..) => serializer.collect_str(self),
        }
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ColorVisitor;

        impl Visitor<'_> for ColorVisitor {
            type Value = Color;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a palette index from 0 to 255, or a colour as \"#rrggbb\"")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Color, E> {
                u8::try_from(value)
                    .map(Color::Indexed)
                    .map_err(|_| E::invalid_value(de::Unexpected::Unsigned(value), &self))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Color, E> {
                u8::try_from(value)
                    .map(Color::Indexed)
                    .map_err(|_| E::invalid_value(de::Unexpected::Signed(value), &self))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Color, E> {
                let invalid = || E::invalid_value(de::Unexpected::Str(value), &self);
                let hex = value.strip_prefix('#').ok_or_else(invalid)?;
                if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(invalid());
                }
                let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16);
                match (channel(0), channel(2), channel(4)) {
                    (Ok(r), Ok(g), Ok(b)) => Ok(Color::Rgb(r, g, b)),
                    _ => Err(invalid()),
                }
            }
        }

        deserializer.deserialize_any(ColorVisitor)
    }
}

impl JsonSchema for Color {
    fn schema_name() -> Cow<'static, str> {
        "Color".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Color").into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A colour: a palette index (0–15 are the themeable ANSI colours, 16–255 xterm's cube and grey ramp), or a 24-bit colour as \"#rrggbb\".",
            "oneOf": [
                { "type": "integer", "minimum": 0, "maximum": 255 },
                { "type": "string", "pattern": "^#[0-9a-fA-F]{6}$" },
            ],
        })
    }
}

/// A piece of a line's text in one style.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRun {
    /// The text. A wide character is written once and takes two cells.
    pub text: String,
    /// The foreground colour; the theme's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<Color>,
    /// The background colour; the theme's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<Color>,
    /// Style bits: 1 bold, 2 italic, 4 underline, 8 inverse, 16 dim, 32 strikethrough,
    /// 64 hidden, 128 undercurl. Absent means none. Unknown bits must be ignored.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub flags: u16,
}

impl TerminalRun {
    /// Bold.
    pub const BOLD: u16 = 1;
    /// Italic.
    pub const ITALIC: u16 = 2;
    /// Underlined, in any underline style but undercurl.
    pub const UNDERLINE: u16 = 4;
    /// Foreground and background swapped.
    pub const INVERSE: u16 = 8;
    /// Dim (faint).
    pub const DIM: u16 = 16;
    /// Struck through.
    pub const STRIKETHROUGH: u16 = 32;
    /// Hidden (concealed): draw the background only.
    pub const HIDDEN: u16 = 64;
    /// A curly underline, as spell checkers draw.
    pub const UNDERCURL: u16 = 128;

    /// Plain text in the default style.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

/// One line of a terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalLine {
    /// Where the line is: `0` is the top row of the screen; `-1` is the scrollback line just
    /// above it, `-2` the one above that, and so on.
    pub index: i64,
    /// The line's text, left to right. The cells after the last run are blank. Empty for a
    /// blank line.
    pub runs: Vec<TerminalRun>,
    /// The line is soft-wrapped: its text goes on in the next line, so copying joins the two
    /// without a line break.
    #[serde(default, skip_serializing_if = "is_false")]
    pub wrapped: bool,
}

impl TerminalLine {
    /// The line's text, without styles. Wide characters appear once.
    pub fn text(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }
}

open_enum! {
    /// How the cursor is drawn.
    pub enum CursorShape {
        /// A filled block over the cell.
        Block = "block",
        /// A line under the cell.
        Underline = "underline",
        /// A vertical bar before the cell.
        Bar = "bar",
    }
}

/// Where the cursor is and how it looks.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCursor {
    /// The row, from `0` at the top of the screen.
    pub row: u16,
    /// The column, from `0` at the left. On a wide character, its first cell.
    pub col: u16,
    /// Whether the program shows it.
    pub visible: bool,
    /// Its shape.
    pub shape: CursorShape,
}

open_enum! {
    /// Which mouse events the program wants reported.
    pub enum MouseTracking {
        /// Presses and releases (mode 1000).
        Click = "click",
        /// Presses, releases, and motion while a button is held (mode 1002).
        Drag = "drag",
        /// Presses, releases and all motion (mode 1003).
        Motion = "motion",
    }
}

/// The modes that change what a client sends as input. Each is off when absent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalModes {
    /// Application cursor keys (DECCKM): the arrow keys, Home and End send `ESC O A` and so on,
    /// instead of `ESC [ A`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub app_cursor: bool,
    /// Application keypad (DECKPAM): the numeric keypad sends `ESC O …` sequences.
    #[serde(default, skip_serializing_if = "is_false")]
    pub app_keypad: bool,
    /// Bracketed paste: wrap pasted text in `ESC [200~` and `ESC [201~`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bracketed_paste: bool,
    /// Which mouse events to report; none when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse: Option<MouseTracking>,
    /// Report the mouse in SGR encoding (mode 1006, `ESC [ < b ; x ; y M`); otherwise in the
    /// default X10 encoding.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mouse_sgr: bool,
    /// Report focus changes: `ESC [ I` when the terminal gains focus, `ESC [ O` when it loses it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub focus_events: bool,
    /// The alternate screen is shown: it has no scrollback.
    #[serde(default, skip_serializing_if = "is_false")]
    pub alt_screen: bool,
    /// On the alternate screen, the mouse wheel should send Up and Down keys (mode 1007).
    #[serde(default, skip_serializing_if = "is_false")]
    pub alternate_scroll: bool,
}

/// How a terminal's program ended.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExit {
    /// The program's exit code. Absent when Burrow ended it, for example on `session/stop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Params naming one terminal: `terminal/attach`, `terminal/detach`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalIdParams {
    /// The terminal's id (`trm_…`), from `terminalOpened` or `Session.terminalId`.
    pub terminal_id: String,
}

/// The whole screen, as `terminal/attach` returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshot {
    /// The terminal.
    pub terminal_id: String,
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// The cursor.
    pub cursor: TerminalCursor,
    /// Every row of the screen, top to bottom: `index` 0 to `rows - 1`. No scrollback: page it
    /// with `terminal/scrollback`.
    pub lines: Vec<TerminalLine>,
    /// The modes that change what the client sends.
    pub modes: TerminalModes,
    /// The title the program set, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// How many scrollback lines `burrowd` keeps above the screen: lines `-scrollback` to `-1`.
    /// At most 10,000. Zero on the alternate screen.
    pub scrollback: u64,
    /// Present once the program has exited; no updates follow then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<TerminalExit>,
}

/// `terminal/update`: what changed since the client's previous snapshot or update. At most one
/// per client every 16 ms or so; a burst of output is coalesced, and a client that reads slowly
/// gets the latest state, never a backlog.
///
/// A client applies it in this order:
///
/// 1. With `reset`: the size is `cols` × `rows`; the client drops its screen and its scrollback,
///    and `lines` holds every row. The scrollback can be paged again.
/// 2. Otherwise, with `scrolled`: the top `scrolled` rows move into the scrollback (the old top
///    row becomes line `-scrolled`, the old row `scrolled - 1` line `-1`), the rows below move
///    up, and blank rows fill the bottom. Scrolled lines the client never had (more than the
///    screen held) are blank until `lines` fills them.
/// 3. Each of `lines` replaces the line at its index: rows that differ after the move, and
///    scrollback lines that differ from the client's copy, such as a row that changed just
///    before it scrolled off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalUpdate {
    /// The terminal.
    pub terminal_id: String,
    /// The screen and the scrollback start afresh: after a resize, or when `burrowd` lost track
    /// of which lines scrolled. `cols`, `rows` and every row come with it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reset: bool,
    /// Columns, with `reset`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    /// Rows, with `reset`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    /// How many lines scrolled off the top of the screen into the scrollback since the previous
    /// update, before `lines` apply. Absent means none.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub scrolled: u64,
    /// Lines that changed, each replaced whole: screen rows and scrollback lines.
    pub lines: Vec<TerminalLine>,
    /// The cursor, always.
    pub cursor: TerminalCursor,
    /// The modes, when any changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<TerminalModes>,
    /// The title, when it changed; empty when the program cleared it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// How many scrollback lines `burrowd` keeps, when that changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrollback: Option<u64>,
    /// The bell rang since the previous update.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bell: bool,
}

/// `terminal/closed`: the terminal's program exited. Sent after the last `terminal/update`. The
/// attachment stays until `terminal/detach`, and the scrollback can still be read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalClosedParams {
    /// The terminal.
    pub terminal_id: String,
    /// The program's exit code. Absent when Burrow ended it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// `terminal/input` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInputParams {
    /// The terminal.
    pub terminal_id: String,
    /// What the user typed, as a terminal sends it: UTF-8 text, with escape sequences for
    /// special keys, encoded for the terminal's current modes (`TerminalModes`).
    pub data: String,
}

/// `terminal/resize` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResizeParams {
    /// The terminal.
    pub terminal_id: String,
    /// Columns, 2 to 1000.
    pub cols: u16,
    /// Rows, 1 to 500.
    pub rows: u16,
}

/// `terminal/scrollback` params: a page of the lines above the screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalScrollbackParams {
    /// The terminal.
    pub terminal_id: String,
    /// Lines above this index: `0` (the default) pages back from the newest scrollback line,
    /// and the oldest index a client has pages on from there. For a client attached to the
    /// terminal, indexes are relative to the screen as that client has it: the answer comes in
    /// order with its updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i64>,
    /// At most this many. Default 100, at most 1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// `terminal/scrollback` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalScrollbackResult {
    /// The lines, oldest first, at their (negative) indexes: the `limit` lines just above
    /// `before`, or fewer.
    pub lines: Vec<TerminalLine>,
    /// No older line is kept: the oldest of `lines` is the oldest there is, or there are none.
    pub complete: bool,
}

/// `session/create`'s terminal size, for terminal sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSize {
    /// Columns, 2 to 1000.
    pub cols: u16,
    /// Rows, 1 to 500.
    pub rows: u16,
}

impl TerminalSize {
    /// The fewest columns a terminal can have.
    pub const MIN_COLS: u16 = 2;
    /// The most columns a terminal can have.
    pub const MAX_COLS: u16 = 1000;
    /// The fewest rows a terminal can have.
    pub const MIN_ROWS: u16 = 1;
    /// The most rows a terminal can have.
    pub const MAX_ROWS: u16 = 500;
    /// The size of a terminal whose client didn't say.
    pub const DEFAULT: Self = Self {
        cols: 120,
        rows: 32,
    };

    /// Whether `burrowd` accepts this size.
    pub fn is_valid(&self) -> bool {
        (Self::MIN_COLS..=Self::MAX_COLS).contains(&self.cols)
            && (Self::MIN_ROWS..=Self::MAX_ROWS).contains(&self.rows)
    }
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &u16) -> bool {
    *value == 0
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn colours_are_indexes_or_hex() {
        assert_eq!(serde_json::to_value(Color::Indexed(9)).unwrap(), json!(9));
        assert_eq!(
            serde_json::to_value(Color::Rgb(255, 0, 0x1e)).unwrap(),
            json!("#ff001e")
        );
        assert_eq!(
            serde_json::from_value::<Color>(json!("#A0b1C2")).unwrap(),
            Color::Rgb(0xa0, 0xb1, 0xc2)
        );
        assert_eq!(
            serde_json::from_value::<Color>(json!(255)).unwrap(),
            Color::Indexed(255)
        );
        for bad in [
            json!(256),
            json!(-1),
            json!("red"),
            json!("#12345"),
            json!("#12345g"),
        ] {
            assert!(
                serde_json::from_value::<Color>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn runs_and_modes_leave_out_their_defaults() {
        let run = TerminalRun::plain("hi");
        assert_eq!(serde_json::to_value(&run).unwrap(), json!({"text": "hi"}));
        let run = TerminalRun {
            text: "x".into(),
            fg: Some(Color::Indexed(1)),
            bg: Some(Color::Rgb(0, 0, 0)),
            flags: TerminalRun::BOLD | TerminalRun::UNDERCURL,
        };
        assert_eq!(
            serde_json::to_value(&run).unwrap(),
            json!({"text": "x", "fg": 1, "bg": "#000000", "flags": 129})
        );
        assert_eq!(
            serde_json::to_value(TerminalModes::default()).unwrap(),
            json!({})
        );
        let modes = TerminalModes {
            app_cursor: true,
            mouse: Some(MouseTracking::Drag),
            mouse_sgr: true,
            ..TerminalModes::default()
        };
        assert_eq!(
            serde_json::to_value(&modes).unwrap(),
            json!({"appCursor": true, "mouse": "drag", "mouseSgr": true})
        );
    }

    #[test]
    fn sizes_have_bounds() {
        assert!(TerminalSize::DEFAULT.is_valid());
        assert!(TerminalSize { cols: 2, rows: 1 }.is_valid());
        assert!(!TerminalSize { cols: 1, rows: 24 }.is_valid());
        assert!(!TerminalSize { cols: 80, rows: 0 }.is_valid());
        assert!(
            !TerminalSize {
                cols: 1001,
                rows: 24
            }
            .is_valid()
        );
    }
}
