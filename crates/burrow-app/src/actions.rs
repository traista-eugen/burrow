//! Keyboard actions and their default bindings.
//!
//! | Keys | Action |
//! |---|---|
//! | Ctrl+N | [`NewSession`]: open the new session page |
//! | Ctrl+L | [`FocusPrompt`]: focus the prompt box |
//! | Ctrl+I | [`ToggleInbox`]: show or hide the inbox panel |
//! | Ctrl+O | [`AddProject`]: add a folder as a project |
//! | Esc | [`CancelTurn`]: stop the running turn, when the prompt has nothing else to do with Esc |
//! | Ctrl+Shift+] / Ctrl+Shift+[ | [`NextSession`] / [`PreviousSession`] in the sidebar |

use gpui::{App, KeyBinding, actions};

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
    ]
);

/// The key context the session view sets, for bindings that only apply there.
pub const SESSION_CONTEXT: &str = "BurrowSession";

/// Binds the default keys.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-n", NewSession, None),
        KeyBinding::new("ctrl-l", FocusPrompt, None),
        KeyBinding::new("ctrl-i", ToggleInbox, None),
        KeyBinding::new("ctrl-o", AddProject, None),
        KeyBinding::new("ctrl-shift-]", NextSession, None),
        KeyBinding::new("ctrl-shift-[", PreviousSession, None),
        KeyBinding::new("escape", CancelTurn, Some(SESSION_CONTEXT)),
    ]);
}
