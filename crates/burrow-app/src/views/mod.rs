//! The app's views.
//!
//! # Layout
//!
//! ```text
//! ┌────────────┬──────────────────────────────────────────────┬─────────────┐
//! │ Sidebar    │ Main area                                    │ Inbox panel │
//! │            │                                              │ (Ctrl+I)    │
//! │ + New      │  one of:                                     │             │
//! │ PROJECTS + │  - the empty page        (empty.rs)          │ open items, │
//! │  ▾ burrow  │  - the new session page  (new_session.rs)    │ newest      │
//! │    ● ses   │  - a session             (session/)          │ first, with │
//! │  NO PROJECT│      ┌ header ─────────── session/header.rs  │ their       │
//! │ RUNS       │      │ transcript ─────── session/transcript │ action      │
//! │            │      │                                       │ buttons     │
//! │ ● status   │      └ prompt box ─────── session/prompt.rs  │             │
//! │ 🔔 inbox  3 │                                              │             │
//! └────────────┴──────────────────────────────────────────────┴─────────────┘
//! ```
//!
//! - [`workspace::Workspace`] is the window's root view. It owns what the main area shows and
//!   whether the inbox is open, and handles the global keyboard actions ([`crate::actions`]).
//!   The sidebar ([`sidebar`]) and the inbox panel ([`inbox`]) are render functions of it.
//! - [`session::SessionView`] is one open session. It owns a [`SessionStore`] and three child
//!   entities that each read the store:
//!   - [`session::header::SessionHeader`]: title, agent, state, and the session's controls;
//!   - [`session::transcript::Transcript`]: the conversation;
//!   - [`session::prompt::PromptBox`]: the multi-line prompt box.
//!
//! # Boundaries for the next work
//!
//! The transcript and the session controls are replaced separately, so each lives behind a
//! narrow interface and touches no one else's files:
//!
//! - **Transcript** (`session/transcript.rs`, plus any `session/transcript/` submodules):
//!   `Transcript::new(store, window, cx)`. It reads [`SessionLog::events`] and listens for
//!   [`SessionStoreEvent`]s. Answering a permission request goes through
//!   [`AppState::respond`].
//! - **Header and controls** (`session/header.rs`, plus `session/header/` submodules):
//!   `SessionHeader::new(store, window, cx)`. It reads [`SessionLog::session`] (state, model,
//!   agent modes, usage, limit) and the session's index entry for its title. Its calls go
//!   through [`AppState`], or [`crate::backend::Backend::call`] for ones `AppState` doesn't
//!   wrap yet.
//! - Shared state lives in [`crate::state`]; views keep only view state (selection, scroll,
//!   focus).
//!
//! [`SessionStore`]: crate::state::session::SessionStore
//! [`SessionLog::events`]: crate::state::session::SessionLog::events
//! [`SessionLog::session`]: crate::state::session::SessionLog::session
//! [`SessionStoreEvent`]: crate::state::session::SessionStoreEvent
//! [`AppState`]: crate::state::AppState
//! [`AppState::respond`]: crate::state::AppState::respond

pub mod empty;
pub mod inbox;
pub mod new_session;
pub mod session;
pub mod sidebar;
pub mod workspace;

use std::time::{SystemTime, UNIX_EPOCH};

use burrow_client::CallError;
use burrow_protocol::Timestamp;
use gpui::{App, SharedString, Window};
use gpui_component::WindowExt as _;
use gpui_component::notification::Notification;

use crate::state::describe_error;

/// Shows a failed call as an error notification.
pub fn notify_error(what: &str, error: &CallError, window: &mut Window, cx: &mut App) {
    let message: SharedString = format!("{what}: {}", describe_error(error)).into();
    window.push_notification(Notification::error(message), cx);
}

/// "just now", "5m", "3h", "2d": how long ago `at` was.
pub fn relative_time(at: Timestamp) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let seconds = (now - at.as_unix_millis()).max(0) / 1000;
    match seconds {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let ago = |seconds: i64| {
            relative_time(Timestamp::from_unix_millis(now - seconds * 1000).unwrap())
        };
        assert_eq!(ago(5), "now");
        assert_eq!(ago(300), "5m");
        assert_eq!(ago(7_200), "2h");
        assert_eq!(ago(200_000), "2d");
    }
}
