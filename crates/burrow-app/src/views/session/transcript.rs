//! The conversation: the session's events as a virtualized list of rows.
//!
//! **Interface.** `Transcript::new(store, state, window, cx)`. It builds its rows from
//! [`SessionLog::events`] and keeps them current from [`SessionStoreEvent`]s: `Appended` applies
//! the new events, `Reset` and `Loaded` rebuild. Inline permission buttons answer through
//! [`crate::views::inbox::respond`]. Nothing else reads this module.
//!
//! **How it stays fast** (D-033):
//! - [`TranscriptModel`] (in [`crate::state::transcript`]) groups events into rows with no GPUI
//!   involved, and reports each [`Change`]: a row added, text appended, a row changed in place.
//! - The list is gpui-component's [`MessageScroller`], a virtualized `gpui::list`: only rows on
//!   screen (plus a margin) are laid out, and their heights are measured as they appear, so rows
//!   of any height work and opening a long session lays out only its end. It follows the tail
//!   while the reader is at the bottom, stops when they scroll up, and then offers "Jump to
//!   latest".
//! - Markdown rows get a [`TextViewState`] the first time they are shown. A growing message gets
//!   only its new text, which the state parses incrementally in the background; nothing else is
//!   re-parsed. Diffs are computed once per tool call revision and cached.
//!
//! The rows are drawn by the submodules: [`message`] (prompts, answers, thoughts), [`tool`]
//! (tool call cards and groups), [`diff`] (diffs), and [`rows`] (plans, permission requests,
//! turn ends, limits, notices).
//!
//! [`SessionLog::events`]: crate::state::session::SessionLog::events

pub mod diff;
mod message;
mod perf;
mod rows;
mod tool;

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, HighlightStyle, Hsla, IntoElement,
    ParentElement as _, Render, SharedString, StyleRefinement, Styled as _, StyledText,
    Subscription, Window, div, px,
};
use gpui_component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_component::spinner::Spinner;
use gpui_component::text::TextViewState;
use gpui_component::{ActiveTheme as _, Sizable as _, Theme, v_flex};
use tracing::debug;

use self::diff::DiffLines;
use self::perf::{FrameStats, Timed};
use crate::state::AppState;
use crate::state::session::{LoadStatus, SessionStore, SessionStoreEvent};
use crate::state::transcript::{Change, Item, TranscriptModel};

/// The widest the conversation's column gets.
const COLUMN_WIDTH: f32 = 820.;

/// Something that can be expanded or collapsed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Toggle {
    /// A row: a thought, a group of tool calls.
    Row(usize),
    /// A tool call's card.
    Tool(String),
    /// One piece of a tool call's content, shown in full or cut short.
    Content(String, usize),
}

impl Toggle {
    /// A string that tells toggles apart, for element ids.
    pub fn id(&self) -> String {
        match self {
            Toggle::Row(row) => format!("row-{row}"),
            Toggle::Tool(id) => format!("tool-{id}"),
            Toggle::Content(id, ix) => format!("content-{id}-{ix}"),
        }
    }
}

/// The theme colours rows use, copied once per frame.
#[derive(Clone)]
pub struct Colors {
    pub foreground: Hsla,
    pub muted_foreground: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub secondary: Hsla,
    pub border: Hsla,
    pub background: Hsla,
    pub primary: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
    pub code_background: Hsla,
    pub radius: gpui::Pixels,
    pub mono_font: SharedString,
}

impl Colors {
    fn new(theme: &Theme) -> Self {
        Self {
            foreground: theme.foreground,
            muted_foreground: theme.muted_foreground,
            muted: theme.muted,
            accent: theme.accent,
            secondary: theme.secondary,
            border: theme.border,
            background: theme.background,
            primary: theme.primary,
            success: theme.success,
            danger: theme.danger,
            warning: theme.warning,
            info: theme.info,
            code_background: theme.muted,
            radius: theme.radius,
            mono_font: theme.mono_font_family.clone(),
        }
    }
}

/// A markdown row's state, and how much of the row's text it has.
struct RichText {
    state: Entity<TextViewState>,
    synced: usize,
    /// The frame it was last shown in.
    shown: u64,
}

/// How many markdown states to keep. Beyond it, the ones shown longest ago are dropped; a row
/// that comes back into view makes a new one, which for most answers parses synchronously.
const KEPT_TEXT_STATES: usize = 240;

/// Diffs by tool call id and content index, with the call's revision they were made from.
type DiffCache = HashMap<(String, usize), (u32, Rc<Vec<DiffLines>>)>;

/// What the rows keep between frames: view state only, never session state.
pub struct RowState {
    /// Markdown states by row, created when a row is first shown.
    texts: HashMap<usize, RichText>,
    /// Counts frames, to tell which markdown states were shown longest ago.
    frame: u64,
    /// Diffs by tool call and content index, with the call's revision they were made from.
    diffs: DiffCache,
    /// What the user expanded or collapsed; everything else has its default.
    expanded: HashMap<Toggle, bool>,
    /// The session's folder, so paths can be shown relative to it.
    cwd: Option<String>,
    colors: Colors,
}

impl RowState {
    /// Whether `toggle` is expanded, `default` unless the user changed it.
    pub fn is_expanded(&self, toggle: &Toggle, default: bool) -> bool {
        self.expanded.get(toggle).copied().unwrap_or(default)
    }

    /// The colours of this frame.
    pub fn colors(&self) -> &Colors {
        &self.colors
    }

    /// The markdown state of row `row`, made from `text` the first time it is asked for.
    pub fn text_state(
        &mut self,
        row: usize,
        text: &str,
        cx: &mut Context<Transcript>,
    ) -> Entity<TextViewState> {
        let rich = self.texts.entry(row).or_insert_with(|| RichText {
            state: cx.new(|cx| TextViewState::markdown(text, cx).selectable(true)),
            synced: text.len(),
            shown: 0,
        });
        rich.shown = self.frame;
        rich.state.clone()
    }

    /// Drops the markdown states shown longest ago, keeping [`KEPT_TEXT_STATES`] at most.
    fn evict_text_states(&mut self) {
        if self.texts.len() <= KEPT_TEXT_STATES {
            return;
        }
        // Down to three quarters, so this runs once in a while rather than every frame.
        let mut shown: Vec<u64> = self.texts.values().map(|rich| rich.shown).collect();
        let drop = self.texts.len() - KEPT_TEXT_STATES * 3 / 4;
        let (_, cutoff, _) = shown.select_nth_unstable(drop - 1);
        let cutoff = *cutoff;
        self.texts.retain(|_, rich| rich.shown > cutoff);
    }

    /// The diffs of a tool call's content `ix`, computed by `make` once per revision.
    pub fn diffs(
        &mut self,
        tool_call_id: &str,
        ix: usize,
        revision: u32,
        make: impl FnOnce() -> Vec<DiffLines>,
    ) -> Rc<Vec<DiffLines>> {
        let key = (tool_call_id.to_owned(), ix);
        match self.diffs.get(&key) {
            Some((cached, diffs)) if *cached == revision => diffs.clone(),
            _ => {
                let diffs = Rc::new(make());
                self.diffs.insert(key, (revision, diffs.clone()));
                diffs
            }
        }
    }

    /// `path` relative to the session's folder when it is inside it.
    pub fn display_path(&self, path: &str) -> String {
        display_path(path, self.cwd.as_deref())
    }

    /// `text` with the session's folder taken out of the paths in it.
    pub fn shorten_paths<'a>(&self, text: &'a str) -> Cow<'a, str> {
        shorten_paths(text, self.cwd.as_deref())
    }
}

/// `text` with every path inside `cwd` made relative to it: agents put absolute paths in titles,
/// such as Claude Code's `Read C:\Users\…\notes.txt`. The folder is matched ignoring ASCII case,
/// and only when a path separator follows it.
pub fn shorten_paths<'a>(text: &'a str, cwd: Option<&str>) -> Cow<'a, str> {
    let Some(cwd) = cwd
        .map(|cwd| cwd.trim_end_matches(['\\', '/']))
        .filter(|cwd| !cwd.is_empty())
    else {
        return Cow::Borrowed(text);
    };
    // ASCII lowercasing keeps every byte where it was, so offsets carry over to `text`.
    let (lower_text, lower_cwd) = (text.to_ascii_lowercase(), cwd.to_ascii_lowercase());
    let mut shortened = String::new();
    let mut copied = 0;
    for (start, _) in lower_text.match_indices(&lower_cwd) {
        let end = start + lower_cwd.len();
        if start >= copied && matches!(text.as_bytes().get(end), Some(b'\\' | b'/')) {
            // A verbatim path's `\\?\` goes with the folder.
            let before = &text[copied..start];
            shortened.push_str(before.strip_suffix(r"\\?\").unwrap_or(before));
            copied = end + 1;
        }
    }
    if copied == 0 {
        return Cow::Borrowed(text);
    }
    shortened.push_str(&text[copied..]);
    Cow::Owned(shortened)
}

/// `path` relative to `cwd` when it is inside it, compared without regard to case or slash
/// direction, as Windows does.
pub fn display_path(path: &str, cwd: Option<&str>) -> String {
    let normalize = |p: &str| {
        p.trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .to_lowercase()
    };
    if let Some(cwd) = cwd {
        let (lower_path, lower_cwd) = (normalize(path), normalize(cwd));
        let lower_cwd = lower_cwd.trim_end_matches('\\');
        if let Some(rest) = lower_path.strip_prefix(lower_cwd)
            && rest.starts_with('\\')
        {
            let start = path.len() - rest.len() + 1;
            if let Some(relative) = path.get(start..) {
                return relative.to_owned();
            }
        }
    }
    path.to_owned()
}

/// `text` with the parts in backticks shown as code, as agents write titles and plan steps:
/// "Run `cargo test`". Unpaired backticks are left as they are.
pub fn code_spans(text: &str, colors: &Colors) -> StyledText {
    if !text.contains('`') || text.matches('`').count() % 2 == 1 {
        return StyledText::new(SharedString::from(text.to_owned()));
    }
    let mut plain = String::with_capacity(text.len());
    let mut code = Vec::new();
    for (ix, part) in text.split('`').enumerate() {
        let start = plain.len();
        plain.push_str(part);
        if ix % 2 == 1 && !part.is_empty() {
            code.push(start..plain.len());
        }
    }
    // The background markdown gives inline code.
    let style = HighlightStyle {
        background_color: Some(colors.accent),
        ..Default::default()
    };
    StyledText::new(SharedString::from(plain))
        .with_highlights(code.into_iter().map(|range| (range, style)))
}

/// How many lines over a cut-off are shown anyway rather than hidden behind a button.
pub const CUT_SLACK: usize = 4;

/// "Show 1 more line", "Show 12 more lines".
pub fn show_more_label(more: usize) -> String {
    if more == 1 {
        "Show 1 more line".to_owned()
    } else {
        format!("Show {more} more lines")
    }
}

/// A small circle the size of an extra-small icon: hollow for something pending, with a dot
/// for something under way. The bundled icon set has neither.
pub fn ring(color: Hsla, dot: bool) -> gpui::Div {
    div()
        .size(px(12.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border(px(1.5))
        .border_color(color)
        .when(dot, |ring| {
            ring.child(div().size(px(5.)).rounded_full().bg(color))
        })
}

pub struct Transcript {
    store: Entity<SessionStore>,
    state: Entity<AppState>,
    model: TranscriptModel,
    scroller: Entity<MessageScrollerState>,
    rows: RowState,
    frame: Rc<FrameStats>,
    _subscriptions: Vec<Subscription>,
}

impl Transcript {
    pub fn new(
        store: Entity<SessionStore>,
        state: Entity<AppState>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.subscribe(
                &store,
                |this, _, event: &SessionStoreEvent, cx| match event {
                    SessionStoreEvent::Appended(range) => this.apply(range.clone(), cx),
                    SessionStoreEvent::Reset | SessionStoreEvent::Loaded => this.rebuild(cx),
                },
            ),
            cx.observe(&state, |_, _, cx| cx.notify()),
        ];
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let mut this = Self {
            store,
            state,
            model: TranscriptModel::new(),
            scroller,
            rows: RowState {
                texts: HashMap::new(),
                frame: 0,
                diffs: HashMap::new(),
                expanded: HashMap::new(),
                cwd: None,
                colors: Colors::new(cx.theme()),
            },
            frame: Rc::default(),
            _subscriptions: subscriptions,
        };
        this.rebuild(cx);
        this
    }

    /// Builds every row again from the store's events.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let started = Instant::now();
        self.model = TranscriptModel::new();
        self.rows.texts.clear();
        self.rows.diffs.clear();
        self.rows.expanded.clear();
        let store = self.store.read(cx);
        self.rows.cwd = store.log().session().map(|session| session.cwd.clone());
        let events = store.log().events();
        let count = events.len();
        let mut changes = Vec::new();
        self.model.apply_all(events, &mut changes);
        let rows = self.model.len();
        self.scroller
            .update(cx, |scroller, cx| scroller.reset(rows, cx));
        debug!(
            target: perf::TARGET,
            events = count,
            rows,
            us = started.elapsed().as_micros() as u64,
            "transcript rebuilt"
        );
        cx.notify();
    }

    /// Applies the store's events in `range`, touching only the rows they change.
    fn apply(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) {
        let started = Instant::now();
        let mut changes = Vec::new();
        let count = range.len();
        self.model
            .apply_all(&self.store.read(cx).log().events()[range], &mut changes);
        let applied = started.elapsed();

        let before = self.scroller.read(cx).item_count();
        let after = self.model.len();
        let mut changed: Vec<usize> = changes
            .iter()
            .filter(|change| !matches!(change, Change::Added(_)))
            .map(Change::ix)
            .filter(|&row| row < before)
            .collect();
        changed.sort_unstable();
        changed.dedup();
        self.scroller.update(cx, |scroller, cx| {
            if after > before {
                scroller.append(after - before, cx);
            }
            for &row in &changed {
                scroller.remeasure_items(row..row + 1, cx);
            }
        });

        // Growing messages get only their new text.
        for change in &changes {
            let Change::Appended(row) = change else {
                continue;
            };
            let Some(rich) = self.rows.texts.get_mut(row) else {
                continue;
            };
            let text = match &self.model.items()[*row] {
                Item::Agent { text, .. } | Item::Thought { text, .. } => text,
                _ => continue,
            };
            if let Some(new) = text.get(rich.synced..)
                && !new.is_empty()
            {
                rich.state.update(cx, |state, cx| state.push_str(new, cx));
                rich.synced = text.len();
            }
        }
        debug!(
            target: perf::TARGET,
            events = count,
            changes = changes.len(),
            model_us = applied.as_micros() as u64,
            us = started.elapsed().as_micros() as u64,
            "transcript applied"
        );
        cx.notify();
    }

    /// Flips `toggle`, which was `current`.
    pub fn toggle(&mut self, toggle: Toggle, current: bool, cx: &mut Context<Self>) {
        let row = match &toggle {
            Toggle::Row(row) => Some(*row),
            Toggle::Tool(id) | Toggle::Content(id, _) => self.model.row_of_tool(id),
        };
        self.rows.expanded.insert(toggle, !current);
        if let Some(row) = row {
            self.scroller.update(cx, |scroller, cx| {
                scroller.remeasure_items(row..row + 1, cx)
            });
        }
        cx.notify();
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.frame.row();
        let Self {
            model, rows, state, ..
        } = self;
        let Some(item) = model.items().get(ix) else {
            return div().into_any_element();
        };
        let live = ix + 1 == model.len() && model.in_turn();
        let element = match item {
            Item::User { text, .. } => message::user(text, rows.colors()),
            Item::Agent { text, .. } => message::agent(ix, text, live, rows, cx),
            Item::Thought { text, .. } => message::thought(ix, text, live, rows, cx),
            Item::Tool(call) => tool::card(call, false, rows, cx),
            Item::ToolGroup(calls) => tool::group(ix, calls, rows, cx),
            Item::Plan { entries } => rows::plan(entries, rows.colors()),
            Item::Permission(request) => {
                let title = rows.shorten_paths(&request.title).into_owned();
                rows::permission(request, title, state, rows.colors())
            }
            Item::TurnEnded(end) => rows::turn_end(end, rows.colors()),
            Item::Limit {
                provider,
                message,
                resets_at,
            } => rows::limit(provider, message, *resets_at, rows.colors()),
            Item::Notice { text, tone } => rows::notice(text, *tone, rows.colors()),
            Item::Unsupported { type_name } => rows::unsupported(type_name, rows.colors()),
        };
        let _ = window;
        div()
            .w_full()
            .max_w(px(COLUMN_WIDTH))
            .mx_auto()
            .px_6()
            .child(element)
            .into_any_element()
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.rows.colors();
        let store = self.store.read(cx);
        let body = match store.status() {
            LoadStatus::Loading => v_flex()
                .items_center()
                .gap_3()
                .child(Spinner::new().small().color(colors.muted_foreground))
                .child(
                    div()
                        .text_color(colors.muted_foreground)
                        .child("Loading the session…"),
                ),
            LoadStatus::Failed(error) => v_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_color(colors.danger)
                        .child("Couldn't open the session"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child(error.clone()),
                ),
            LoadStatus::Ready => {
                let session = store.log().session();
                let agent = session
                    .map(|session| self.state.read(cx).agent_name(&session.agent))
                    .unwrap_or_else(|| "the agent".to_owned());
                // The folder's name: the header has the whole path.
                let cwd = session.map(|session| {
                    let folder = session.cwd.trim_end_matches(['\\', '/']);
                    let name = folder.rsplit(['\\', '/']).next().unwrap_or(folder);
                    format!("in {name}")
                });
                v_flex()
                    .items_center()
                    .gap_3()
                    .max_w(px(420.))
                    .child(crate::views::empty::logo(px(56.)))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("A new session with {agent}")),
                    )
                    .when_some(cwd, |column, cwd| {
                        column.child(
                            div()
                                .text_sm()
                                .text_color(colors.muted_foreground)
                                .child(cwd),
                        )
                    })
                    .child(
                        div()
                            .pt_2()
                            .text_sm()
                            .text_center()
                            .text_color(colors.muted_foreground)
                            .child(
                                "Ask for anything below. Enter sends, Shift+Enter adds a line, \
                                 and Esc stops a running turn.",
                            ),
                    )
            }
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .child(body)
            .into_any_element()
    }
}

impl Render for Transcript {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.rows.colors = Colors::new(cx.theme());
        self.rows.frame += 1;
        self.rows.evict_text_states();
        if self.model.is_empty() {
            return self.render_empty(cx);
        }
        let this = cx.weak_entity();
        let scroller = MessageScroller::new(
            "transcript",
            self.scroller.clone(),
            move |ix, window, cx| {
                this.update(cx, |this, cx| this.render_row(ix, window, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            },
        )
        .with_list_style(StyleRefinement::default().pt(px(20.)).pb(px(28.)))
        .with_row_style(StyleRefinement::default().px(px(0.)).pb(px(14.)))
        .with_bottom_fade(self.rows.colors.background)
        .with_jump_button_label("Jump to latest")
        .with_jump_button_renderer(|button| button.label("Jump to latest").small());
        Timed::new(scroller.into_any_element(), self.frame.clone()).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{display_path, shorten_paths};

    #[test]
    fn paths_inside_the_session_folder_are_shown_relative_to_it() {
        let cwd = Some(r"D:\Repos\burrow");
        assert_eq!(
            display_path(r"D:\Repos\burrow\src\main.rs", cwd),
            r"src\main.rs"
        );
        assert_eq!(display_path(r"d:/repos/burrow/README.md", cwd), "README.md");
        assert_eq!(
            display_path(r"\\?\D:\Repos\burrow\Cargo.toml", Some(r"D:\Repos\burrow\")),
            "Cargo.toml"
        );
        assert_eq!(
            display_path(r"D:\Repos\burrowd\x", cwd),
            r"D:\Repos\burrowd\x"
        );
        assert_eq!(display_path("notes.txt", None), "notes.txt");
    }

    #[test]
    fn titles_lose_the_session_folder() {
        let cwd = Some(r"C:\Users\eugen\work\");
        assert_eq!(
            shorten_paths(r"Edit c:\users\eugen\work\src\lib.rs", cwd),
            r"Edit src\lib.rs"
        );
        assert_eq!(
            shorten_paths(
                r"Move C:\Users\eugen\work\a.rs to C:\Users\eugen\work\b.rs",
                cwd
            ),
            "Move a.rs to b.rs"
        );
        assert_eq!(
            shorten_paths(r"Read \\?\C:\Users\eugen\work\x.rs", cwd),
            "Read x.rs"
        );
        // Only the whole folder, and nothing without a folder.
        assert_eq!(
            shorten_paths(r"Read C:\Users\eugen\workshop\x", cwd),
            r"Read C:\Users\eugen\workshop\x"
        );
        assert_eq!(shorten_paths("cargo test", None), "cargo test");
    }
}
