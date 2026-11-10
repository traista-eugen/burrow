//! The terminal's colours: the 16 ANSI colours themed to match the app, the rest of xterm's
//! 256-colour palette, and how a run's style turns into the colours it is drawn in.

use super::grid::{Color, Flags, Style};
use crate::theme::palette;

/// The colours a terminal is drawn in, as `0xRRGGBB`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalTheme {
    /// ANSI colours 0–15: black, red, green, yellow, blue, magenta, cyan, white, then the bright
    /// ones.
    pub ansi: [u32; 16],
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    /// The text under a block cursor.
    pub cursor_text: u32,
    pub selection: u32,
}

impl TerminalTheme {
    /// Burrow's dark theme: the app's background and cream text, with ANSI colours warmed to sit
    /// with the icon's tan and gold.
    pub fn burrow() -> Self {
        Self {
            ansi: [
                0x2A2426, // black
                0xE06C5E, // red
                0x9DBF6E, // green
                0xE5B45F, // yellow
                0x6FA3D8, // blue
                0xC88AC4, // magenta
                0x6CC0B8, // cyan
                0xD8CFC6, // white
                0x857A7E, // bright black
                0xF4887A, // bright red
                0xB8D88A, // bright green
                palette::GOLD,
                0x8EBCEB, // bright blue
                0xDDA6D9, // bright magenta
                0x8ED8D0, // bright cyan
                palette::CREAM,
            ],
            foreground: palette::CREAM,
            background: palette::BACKGROUND,
            cursor: palette::GOLD,
            cursor_text: palette::BACKGROUND,
            selection: palette::TAN,
        }
    }

    /// A colour as `0xRRGGBB`.
    pub fn rgb(&self, color: Color) -> u32 {
        match color {
            Color::Rgb(r, g, b) => (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
            Color::Indexed(index) => indexed(index, &self.ansi),
        }
    }

    /// The colours a run of `style` is drawn in.
    pub fn resolve(&self, style: Style) -> CellColors {
        let mut fg = style.fg.map_or(self.foreground, |color| self.rgb(color));
        let mut bg = style.bg.map(|color| self.rgb(color));
        if style.flags.contains(Flags::INVERSE) {
            let swapped = bg.unwrap_or(self.background);
            bg = Some(fg);
            fg = swapped;
        }
        let behind = bg.unwrap_or(self.background);
        if style.flags.contains(Flags::DIM) {
            fg = mix(fg, behind, 0.4);
        }
        CellColors {
            fg,
            bg,
            hidden: style.flags.contains(Flags::HIDDEN),
        }
    }
}

/// The colours one run is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellColors {
    pub fg: u32,
    /// `None` is the terminal's own background, which isn't painted cell by cell.
    pub bg: Option<u32>,
    /// The text isn't drawn.
    pub hidden: bool,
}

/// Entry `index` of xterm's 256-colour palette: the ANSI 16, a 6×6×6 colour cube, then 24 greys.
pub fn indexed(index: u8, ansi: &[u32; 16]) -> u32 {
    match index {
        0..=15 => ansi[usize::from(index)],
        16..=231 => {
            let level = |n: u8| if n == 0 { 0 } else { 55 + 40 * u32::from(n) };
            let n = index - 16;
            (level(n / 36) << 16) | (level(n / 6 % 6) << 8) | level(n % 6)
        }
        232..=255 => {
            let grey = 8 + 10 * u32::from(index - 232);
            (grey << 16) | (grey << 8) | grey
        }
    }
}

/// `a` moved `amount` of the way towards `b`.
fn mix(a: u32, b: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let (a, b) = (((a >> shift) & 0xFF) as f32, ((b >> shift) & 0xFF) as f32);
        ((a + (b - a) * amount).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_is_xterms() {
        let theme = TerminalTheme::burrow();
        let ansi = &theme.ansi;
        assert_eq!(indexed(1, ansi), theme.ansi[1]);
        assert_eq!(indexed(15, ansi), palette::CREAM);
        // The cube: 16 is black, 21 pure blue, 196 pure red, 231 white.
        assert_eq!(indexed(16, ansi), 0x000000);
        assert_eq!(indexed(21, ansi), 0x0000FF);
        assert_eq!(indexed(196, ansi), 0xFF0000);
        assert_eq!(indexed(67, ansi), 0x5F87AF);
        assert_eq!(indexed(231, ansi), 0xFFFFFF);
        // The greys.
        assert_eq!(indexed(232, ansi), 0x080808);
        assert_eq!(indexed(255, ansi), 0xEEEEEE);
        assert_eq!(theme.rgb(Color::Rgb(0x12, 0x34, 0x56)), 0x123456);
    }

    #[test]
    fn styles_resolve_to_colours() {
        let theme = TerminalTheme::burrow();
        let plain = theme.resolve(Style::default());
        assert_eq!(plain.fg, theme.foreground);
        assert_eq!(plain.bg, None);

        let red_on_blue = Style {
            fg: Some(Color::Indexed(1)),
            bg: Some(Color::Indexed(4)),
            flags: Flags::empty(),
        };
        let colors = theme.resolve(red_on_blue);
        assert_eq!((colors.fg, colors.bg), (theme.ansi[1], Some(theme.ansi[4])));

        // Inverse swaps them, and the default background becomes a painted foreground.
        let colors = theme.resolve(Style {
            flags: Flags::INVERSE,
            ..red_on_blue
        });
        assert_eq!((colors.fg, colors.bg), (theme.ansi[4], Some(theme.ansi[1])));
        let colors = theme.resolve(Style {
            flags: Flags::INVERSE,
            ..Style::default()
        });
        assert_eq!(
            (colors.fg, colors.bg),
            (theme.background, Some(theme.foreground))
        );

        // Dim fades towards what is behind the text.
        let dim = theme.resolve(Style {
            fg: Some(Color::Rgb(200, 200, 200)),
            bg: Some(Color::Rgb(0, 0, 0)),
            flags: Flags::DIM,
        });
        assert_eq!(dim.fg, 0x787878);

        assert!(
            theme
                .resolve(Style {
                    flags: Flags::HIDDEN,
                    ..Style::default()
                })
                .hidden
        );
    }
}
