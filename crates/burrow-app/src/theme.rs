//! Burrow's look: gpui-component's dark theme, recoloured from the dachshund icon's palette
//! (warm near-black, tan, cream), plus the colours the app uses for session states.

use burrow_protocol::SessionState;
use gpui::{App, Hsla, Rgba, rgb};
use gpui_component::{Theme, ThemeMode};

/// The icon's palette.
pub mod palette {
    /// The dachshund's tan.
    pub const TAN: u32 = 0xC9783E;
    /// A lighter tan for hover.
    pub const TAN_LIGHT: u32 = 0xD98C52;
    /// The icon's gold highlight.
    pub const GOLD: u32 = 0xF9D27E;
    /// Cream, for text on dark.
    pub const CREAM: u32 = 0xF4EDE4;
    /// The window background.
    pub const BACKGROUND: u32 = 0x141113;
    /// The sidebar and panels, a step darker.
    pub const PANEL: u32 = 0x0F0D0E;
    /// Raised surfaces: inputs, cards.
    pub const RAISED: u32 = 0x1C1819;
    /// Hover on raised surfaces.
    pub const HOVER: u32 = 0x262022;
    /// Borders.
    pub const BORDER: u32 = 0x2B2528;
    /// Secondary text.
    pub const MUTED: u32 = 0x948A8E;
}

fn color(hex: u32) -> Hsla {
    Hsla::from(rgb(hex))
}

fn rgba(hex: u32, alpha: f32) -> Hsla {
    let mut color: Rgba = rgb(hex);
    color.a = alpha;
    Hsla::from(color)
}

/// Applies Burrow's theme. Call once, after `gpui_component::init`.
pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        theme.radius = gpui::px(6.);
        let c = &mut theme.colors;
        c.background = color(palette::BACKGROUND);
        c.foreground = color(palette::CREAM);
        c.border = color(palette::BORDER);
        c.input = color(0x3A3236);
        c.ring = color(palette::TAN);
        c.caret = color(palette::GOLD);
        c.selection = rgba(palette::TAN, 0.35);
        c.muted = color(palette::RAISED);
        c.muted_foreground = color(palette::MUTED);
        c.accent = color(palette::HOVER);
        c.accent_foreground = color(palette::CREAM);
        c.primary = color(palette::TAN);
        c.primary_hover = color(palette::TAN_LIGHT);
        c.primary_active = color(0xB56A33);
        c.primary_foreground = color(0x140E0A);
        c.secondary = color(palette::RAISED);
        c.secondary_hover = color(palette::HOVER);
        c.secondary_active = color(0x2E2729);
        c.secondary_foreground = color(palette::CREAM);
        c.button_primary = c.primary;
        c.button_primary_hover = c.primary_hover;
        c.button_primary_active = c.primary_active;
        c.button_primary_foreground = c.primary_foreground;
        c.link = color(0xF09A5C);
        c.link_hover = color(palette::GOLD);
        c.link_active = color(palette::TAN);
        c.popover = color(palette::RAISED);
        c.popover_foreground = color(palette::CREAM);
        c.sidebar = color(palette::PANEL);
        c.sidebar_foreground = color(palette::CREAM);
        c.sidebar_border = color(palette::BORDER);
        c.sidebar_accent = color(0x1F1A1C);
        c.sidebar_accent_foreground = color(palette::CREAM);
        c.sidebar_primary = color(palette::TAN);
        c.sidebar_primary_foreground = color(0x140E0A);
        c.list = color(palette::PANEL);
        c.list_hover = color(palette::RAISED);
        c.list_active = rgba(palette::TAN, 0.16);
        c.list_active_border = color(palette::TAN);
        c.scrollbar = rgba(palette::PANEL, 0.0);
        c.scrollbar_thumb = rgba(0x5A5054, 0.8);
        c.scrollbar_thumb_hover = color(0x6A5F64);
        c.title_bar = color(palette::PANEL);
        c.title_bar_border = color(palette::BORDER);
        c.window_border = color(palette::BORDER);
        c.overlay = rgba(0x000000, 0.45);
        c.danger = color(0xE5675B);
        c.danger_hover = color(0xEC7E73);
        c.danger_active = color(0xCF5649);
        c.danger_foreground = color(0x140E0A);
        c.button_danger = c.danger;
        c.button_danger_hover = c.danger_hover;
        c.button_danger_active = c.danger_active;
        c.button_danger_foreground = c.danger_foreground;
        c.warning = color(palette::GOLD);
        c.success = color(0x7FB77E);
        c.info = color(0x7AA6C2);
        c.tab_bar = color(palette::PANEL);
        c.skeleton = color(palette::RAISED);
    });
}

/// The colour that marks a session state in lists and headers.
pub fn state_color(state: &SessionState, cx: &App) -> Hsla {
    let theme = Theme::global(cx);
    match state {
        SessionState::Working => color(palette::GOLD),
        SessionState::Waiting => color(palette::TAN),
        SessionState::Idle => theme.success,
        SessionState::Starting => theme.info,
        SessionState::Limited => theme.danger.opacity(0.85),
        SessionState::Failed => theme.danger,
        SessionState::Stopped | SessionState::Other(_) => theme.muted_foreground.opacity(0.6),
    }
}

/// A short label for a session state.
pub fn state_label(state: &SessionState) -> String {
    match state {
        SessionState::Starting => "starting".into(),
        SessionState::Idle => "ready".into(),
        SessionState::Working => "working".into(),
        SessionState::Waiting => "needs you".into(),
        SessionState::Limited => "limited".into(),
        SessionState::Stopped => "stopped".into(),
        SessionState::Failed => "failed".into(),
        SessionState::Other(other) => other.clone(),
    }
}
