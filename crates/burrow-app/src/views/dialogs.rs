//! Dialogs the header and the sidebar share: renaming a session, and confirming an archive.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, AppContext as _, Entity, ParentElement as _, Styled as _, Window, div, px};
use gpui_component::button::ButtonVariant;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, WindowExt as _};

use super::report_failure;
use crate::state::AppState;

/// Something to do with the window, shared by a dialog's callbacks.
type WindowFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// Asks for a new title and renames the session (`session/rename`). Enter or OK renames; an
/// empty or unchanged title does nothing.
pub fn rename_session(
    state: Entity<AppState>,
    session_id: String,
    current: String,
    window: &mut Window,
    cx: &mut App,
) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(current.clone())
            .placeholder("Session title")
    });
    let rename: WindowFn = Rc::new({
        let input = input.clone();
        move |window, cx| {
            let title = input.read(cx).value().trim().to_owned();
            if title.is_empty() || title == current {
                return;
            }
            let call = state.read(cx).rename(session_id.clone(), title, cx);
            report_failure("Couldn't rename the session", call, window, cx);
        }
    });
    // Enter in the field renames too. The subscription lives as long as the dialog.
    let on_enter = Rc::new(window.subscribe(&input, cx, {
        let rename = rename.clone();
        move |_, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                rename(window, cx);
                window.close_dialog(cx);
            }
        }
    }));
    let focus_once = Cell::new(true);
    window.open_alert_dialog(cx, move |dialog, window, cx| {
        let _keep = &on_enter;
        if focus_once.replace(false) {
            input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
        }
        let rename = rename.clone();
        dialog
            .title("Rename session")
            .width(px(440.))
            .child(div().pt_2().child(Input::new(&input)))
            .confirm()
            .ok_text("Rename")
            .on_ok(move |_, window, cx| {
                rename(window, cx);
                true
            })
    });
}

/// Asks before archiving a session (`session/archive`), which stops it first if it is live.
pub fn confirm_archive(
    state: Entity<AppState>,
    session_id: String,
    title: String,
    live: bool,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |dialog, _window, cx| {
        let state = state.clone();
        let session_id = session_id.clone();
        let detail = if live {
            "The agent is stopped first. The session leaves the sidebar; its transcript is kept, \
             under \"Show archived\"."
        } else {
            "The session leaves the sidebar; its transcript is kept, under \"Show archived\"."
        };
        dialog
            .title(format!("Archive \u{201c}{title}\u{201d}?"))
            .width(px(440.))
            .description(div().text_color(cx.theme().muted_foreground).child(detail))
            .confirm()
            .ok_text(if live { "Stop and archive" } else { "Archive" })
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                let call = state.update(cx, |state, cx| state.archive(session_id.clone(), cx));
                report_failure("Couldn't archive the session", call, window, cx);
                true
            })
    });
}
