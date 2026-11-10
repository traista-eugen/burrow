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
//! - [`workspace::Workspace`] is the window's root view. It owns what the main area shows,
//!   the last few sessions' views ([`workspace::KEEP_OPEN`]), whether the inbox and archived
//!   sessions show, and the clock that redraws relative times; it handles the global keyboard
//!   actions ([`crate::actions`]). The sidebar ([`sidebar`]) and the inbox panel ([`inbox`]) are
//!   render functions of it. [`dialogs`] has the rename and archive dialogs the header and the
//!   sidebar share.
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
//! - **Transcript** (`session/transcript.rs`, plus its `session/transcript/` submodules):
//!   `Transcript::new(store, state, window, cx)`. It reads [`SessionLog::events`], listens for
//!   [`SessionStoreEvent`]s, and groups the events into rows with
//!   [`crate::state::transcript::TranscriptModel`]. Answering a permission request goes through
//!   [`AppState::respond`].
//! - **Header and controls** (`session/header.rs`, plus `session/header/` submodules):
//!   `SessionHeader::new(store, window, cx)`. It reads [`SessionLog::session`] (state, model,
//!   agent modes, usage, limit), [`SessionLog::turn_usage`], the session's index entry for its
//!   title, and [`AppState::usage`] for a limit's reset time. Its calls go through [`AppState`]
//!   (`set_model`, `set_agent_mode`, `rename`, `stop`, `archive`, `cancel`).
//! - **Prompt box** (`session/prompt.rs`): reads the state and [`SessionLog::prompts`] for its
//!   history; sends through [`AppState::prompt`].
//! - **Terminal** (`terminal/`, M4): [`terminal::TerminalView`] draws a terminal `burrowd`
//!   emulates. It isn't in the layout yet; M4's phase 2 shows it for terminal-mode sessions. Its
//!   interface (snapshots and updates in, input, resizes and history requests out) is described
//!   in `terminal/mod.rs`.
//! - Shared state lives in [`crate::state`]; views keep only view state (selection, scroll,
//!   focus).
//!
//! [`SessionStore`]: crate::state::session::SessionStore
//! [`SessionLog::events`]: crate::state::session::SessionLog::events
//! [`SessionLog::session`]: crate::state::session::SessionLog::session
//! [`SessionLog::turn_usage`]: crate::state::session::SessionLog::turn_usage
//! [`SessionLog::prompts`]: crate::state::session::SessionLog::prompts
//! [`AppState::usage`]: crate::state::AppState::usage
//! [`AppState::prompt`]: crate::state::AppState::prompt
//! [`SessionStoreEvent`]: crate::state::session::SessionStoreEvent
//! [`AppState`]: crate::state::AppState
//! [`AppState::respond`]: crate::state::AppState::respond

pub mod dialogs;
pub mod empty;
pub mod inbox;
pub mod new_session;
pub mod session;
pub mod sidebar;
// The session view uses it from M4's phase 2; until then only its tests and the demo do.
#[allow(dead_code)]
pub mod terminal;
pub mod workspace;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use burrow_client::CallError;
use burrow_protocol::{Empty, Timestamp};
use gpui::{App, SharedString, Task, Window};
use gpui_component::WindowExt as _;
use gpui_component::notification::Notification;

use crate::state::describe_error;

/// Shows a failed call as an error notification.
pub fn notify_error(what: &str, error: &CallError, window: &mut Window, cx: &mut App) {
    let message: SharedString = format!("{what}: {}", describe_error(error)).into();
    window.push_notification(Notification::error(message), cx);
}

/// Waits for a call whose answer is `{}`, showing a notification if it fails.
pub fn report_failure(
    what: &'static str,
    call: Task<Result<Empty, CallError>>,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .spawn(cx, async move |cx| {
            if let Err(error) = call.await {
                let _ = cx.update(|window, cx| notify_error(what, &error, window, cx));
            }
        })
        .detach();
}

/// Milliseconds since the Unix epoch, now.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// "now", "5m", "3h", "2d": how long ago `at` was.
pub fn relative_time(at: Timestamp) -> String {
    relative_time_at(at.as_unix_millis(), now_millis())
}

fn relative_time_at(at: i64, now: i64) -> String {
    let seconds = (now - at).max(0) / 1000;
    match seconds {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// How long until [`relative_time`] of `at` reads differently, so a list of them can be redrawn
/// exactly when one changes instead of on a fixed tick: within the minute for "now" and "5m",
/// on the hour for "3h", on the day for "2d".
pub fn relative_time_changes_in(at: Timestamp, now: i64) -> Duration {
    let elapsed = (now - at.as_unix_millis()).max(0);
    let unit = match elapsed / 1000 {
        0..3_600 => 60_000,
        3_600..86_400 => 3_600_000,
        _ => 86_400_000,
    };
    Duration::from_millis((unit - elapsed % unit) as u64)
}

/// A countdown: "45s", "12:05" (minutes and seconds), "3h 05m", "2d 4h".
pub fn countdown(remaining: Duration) -> String {
    let seconds = remaining.as_secs();
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3_600 => format!("{}:{:02}", seconds / 60, seconds % 60),
        3_600..86_400 => format!("{}h {:02}m", seconds / 3_600, seconds % 3_600 / 60),
        _ => format!("{}d {}h", seconds / 86_400, seconds % 86_400 / 3_600),
    }
}

/// `at` as a local wall-clock time, "15:04", with the date too when it isn't today:
/// "Mon 10 Nov 15:04". `None` if the OS can't convert it.
pub fn local_time(at: Timestamp) -> Option<String> {
    let (then, today) = (
        clock::local(at.as_unix_millis())?,
        clock::local(now_millis())?,
    );
    let time = format!("{:02}:{:02}", then.hour, then.minute);
    if (then.year, then.month, then.day) == (today.year, today.month, today.day) {
        Some(time)
    } else {
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        Some(format!(
            "{} {} {} {time}",
            DAYS.get(usize::from(then.weekday))?,
            then.day,
            MONTHS.get(usize::from(then.month).checked_sub(1)?)?
        ))
    }
}

/// Local wall-clock time from the OS's time zone rules.
mod clock {
    /// A local date and time.
    pub struct Local {
        pub year: u16,
        pub month: u16,
        pub day: u16,
        pub weekday: u16,
        pub hour: u16,
        pub minute: u16,
    }

    #[cfg(windows)]
    pub fn local(unix_millis: i64) -> Option<Local> {
        use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows_sys::Win32::System::Time::{
            FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime,
        };

        // FILETIME counts 100 ns from 1601; the Unix epoch is 11 644 473 600 s later.
        let ticks = u64::try_from(unix_millis)
            .ok()?
            .checked_mul(10_000)?
            .checked_add(116_444_736_000_000_000)?;
        let file_time = FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        // SAFETY: SYSTEMTIME is plain data; all-zero is a valid value to overwrite.
        let (mut utc, mut local): (SYSTEMTIME, SYSTEMTIME) =
            unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
        // SAFETY: the pointers point at live locals; a null time zone means the current one.
        let ok = unsafe {
            FileTimeToSystemTime(&file_time, &mut utc) != 0
                && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0
        };
        ok.then_some(Local {
            year: local.wYear,
            month: local.wMonth,
            day: local.wDay,
            weekday: local.wDayOfWeek,
            hour: local.wHour,
            minute: local.wMinute,
        })
    }

    #[cfg(not(windows))]
    pub fn local(_unix_millis: i64) -> Option<Local> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        let now = now_millis();
        let ago = |seconds: i64| {
            relative_time(Timestamp::from_unix_millis(now - seconds * 1000).unwrap())
        };
        assert_eq!(ago(5), "now");
        assert_eq!(ago(300), "5m");
        assert_eq!(ago(7_200), "2h");
        assert_eq!(ago(200_000), "2d");
    }

    #[test]
    fn relative_times_are_redrawn_when_they_change() {
        let at = Timestamp::from_unix_millis(1_800_000_000_000).unwrap();
        let base = at.as_unix_millis();
        let changes = |elapsed_ms: i64| relative_time_changes_in(at, base + elapsed_ms);
        // "now" turns into "1m" at one minute.
        assert_eq!(changes(5_000), Duration::from_secs(55));
        // "5m" into "6m" at six minutes.
        assert_eq!(changes(5 * 60_000 + 1_500), Duration::from_millis(58_500));
        // "2h" into "3h" on the hour.
        assert_eq!(
            changes(2 * 3_600_000 + 60_000),
            Duration::from_secs(59 * 60)
        );
        // "2d" into "3d" on the day.
        assert_eq!(changes(2 * 86_400_000), Duration::from_secs(86_400));
        // At each of those moments the label really changes, and not a moment before.
        for elapsed in [5_000, 301_500, 7_260_000, 172_800_000] {
            let next = changes(elapsed).as_millis() as i64;
            let label = |ms| relative_time_at(base, base + ms);
            assert_eq!(label(elapsed), label(elapsed + next - 1));
            assert_ne!(label(elapsed), label(elapsed + next));
        }
        // A time in the future (clock skew) reads "now" and is checked again within a minute.
        assert!(relative_time_changes_in(at, base - 10_000) <= Duration::from_secs(60));
    }

    #[test]
    fn countdowns() {
        assert_eq!(countdown(Duration::from_secs(45)), "45s");
        assert_eq!(countdown(Duration::from_secs(12 * 60 + 5)), "12:05");
        assert_eq!(
            countdown(Duration::from_secs(3 * 3_600 + 5 * 60 + 9)),
            "3h 05m"
        );
        assert_eq!(
            countdown(Duration::from_secs(2 * 86_400 + 4 * 3_600)),
            "2d 4h"
        );
    }

    #[cfg(windows)]
    #[test]
    fn local_times_are_hours_and_minutes_today() {
        let now = Timestamp::from_unix_millis(now_millis()).unwrap();
        let text = local_time(now).unwrap();
        // "15:04", or with a date if the test runs across midnight.
        assert_eq!(&text[text.len() - 3..text.len() - 2], ":", "{text}");
        let far = Timestamp::from_unix_millis(now.as_unix_millis() + 3 * 86_400_000).unwrap();
        assert!(local_time(far).unwrap().len() > 5);
    }
}
