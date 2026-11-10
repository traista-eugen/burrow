//! Keyboard actions and their default bindings.
//!
//! | Keys | Action |
//! |---|---|
//! | Ctrl+N | [`NewSession`]: open the new session page |
//! | Ctrl+L | [`FocusPrompt`]: focus the prompt box, or a terminal session's terminal |
//! | Ctrl+I | [`ToggleInbox`]: show or hide the inbox panel |
//! | Ctrl+O | [`AddProject`]: add a folder as a project |
//! | Esc | [`CancelTurn`]: stop the running turn, when the prompt has nothing else to do with Esc |
//! | Ctrl+Shift+] / Ctrl+Shift+[ | [`NextSession`] / [`PreviousSession`] in the sidebar |
//! | Up / Down in the prompt box | [`HistoryPrevious`] / [`HistoryNext`]: earlier prompts, when the box is empty or shows one |
//! | Esc while renaming in the header | [`CancelRename`]: keep the old title |
//!
//! A focused terminal takes Ctrl+N, Ctrl+L, Ctrl+I, Ctrl+O and Esc for its program (D-041).
//! There, as in Windows Terminal, the app's keys take Shift (D-043):
//!
//! | Keys in a terminal | Action |
//! |---|---|
//! | Shift+Esc | [`LeaveTerminal`]: give the keyboard back to the app; Ctrl+L returns it |
//! | Ctrl+Shift+N | [`NewSession`] |
//! | Ctrl+Shift+I | [`ToggleInbox`] |
//! | Ctrl+Shift+O | [`AddProject`] |
//! | Ctrl+Shift+] / Ctrl+Shift+[ | [`NextSession`] / [`PreviousSession`], as anywhere |
//!
//! Programs can't tell these from the keys without Shift (xterm's encodings have no Shift for
//! Esc or Ctrl+letter), so they lose nothing.
//!
//! The prompt box's and the rename field's bindings name the text field's own `Input` context
//! under theirs, so they are as specific as the field's Up, Down and Esc. Bound after
//! gpui-component's, they are tried first; a handler that doesn't want the key propagates, and
//! the field's own binding runs.

use gpui::{App, KeyBinding, actions};

use crate::views::terminal::TERMINAL_CONTEXT;

actions!(
    burrow,
    [
        NewSession,
        FocusPrompt,
        ToggleInbox,
        AddProject,
        CancelTurn,
        NextSession,
        PreviousSession,
        HistoryPrevious,
        HistoryNext,
        CancelRename,
        LeaveTerminal,
    ]
);

/// The key context the session view sets, for bindings that only apply there.
pub const SESSION_CONTEXT: &str = "BurrowSession";
/// The key context around the prompt box's text field.
pub const PROMPT_CONTEXT: &str = "BurrowPrompt";
/// The key context around the header's rename field.
pub const RENAME_CONTEXT: &str = "BurrowRename";

/// Binds the default keys. Call after `gpui_component::init`.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-n", NewSession, None),
        KeyBinding::new("ctrl-l", FocusPrompt, None),
        KeyBinding::new("ctrl-i", ToggleInbox, None),
        KeyBinding::new("ctrl-o", AddProject, None),
        KeyBinding::new("ctrl-shift-]", NextSession, None),
        KeyBinding::new("ctrl-shift-[", PreviousSession, None),
        KeyBinding::new("escape", CancelTurn, Some(SESSION_CONTEXT)),
        KeyBinding::new("up", HistoryPrevious, Some("BurrowPrompt > Input")),
        KeyBinding::new("down", HistoryNext, Some("BurrowPrompt > Input")),
        KeyBinding::new("escape", CancelRename, Some("BurrowRename > Input")),
        KeyBinding::new("shift-escape", LeaveTerminal, Some(TERMINAL_CONTEXT)),
        KeyBinding::new("ctrl-shift-n", NewSession, Some(TERMINAL_CONTEXT)),
        KeyBinding::new("ctrl-shift-i", ToggleInbox, Some(TERMINAL_CONTEXT)),
        KeyBinding::new("ctrl-shift-o", AddProject, Some(TERMINAL_CONTEXT)),
    ]);
}
