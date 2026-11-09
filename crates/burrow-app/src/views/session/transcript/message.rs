//! Prompts, answers and thoughts.
//!
//! - A prompt is plain text in a bubble on the right.
//! - An answer is markdown, selectable, with highlighted code blocks that each have a copy
//!   button. The answer still streaming fades its new text in.
//! - A thought is collapsed to one line, "Thinking" with its latest line while it streams; a
//!   click shows it all.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px, relative,
};
use gpui_component::clipboard::Clipboard;
use gpui_component::spinner::Spinner;
use gpui_component::text::TextView;
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

use super::{Colors, RowState, Toggle, Transcript};

/// The user's prompt.
pub fn user(text: &str, colors: &Colors) -> AnyElement {
    div()
        .w_full()
        .flex()
        .justify_end()
        .child(
            div()
                .max_w(relative(0.85))
                .px_4()
                .py_2()
                .rounded(colors.radius * 2.)
                .bg(colors.secondary)
                .border_1()
                .border_color(colors.border)
                .child(SharedString::from(text.trim().to_owned())),
        )
        .into_any_element()
}

/// The agent's answer, as markdown.
pub fn agent(
    row: usize,
    text: &str,
    live: bool,
    rows: &mut RowState,
    cx: &mut Context<Transcript>,
) -> AnyElement {
    let state = rows.text_state(row, text, cx);
    div()
        .w_full()
        .child(markdown(&state).stream_fade(live))
        .into_any_element()
}

/// A selectable markdown view whose code blocks have a copy button.
fn markdown(state: &gpui::Entity<gpui_component::text::TextViewState>) -> TextView {
    TextView::new(state)
        .selectable(true)
        .code_block_actions(|block, _, cx| {
            let muted = cx.theme().muted_foreground;
            h_flex()
                .gap_1()
                .items_center()
                .when_some(block.lang().filter(|lang| !lang.is_empty()), |row, lang| {
                    row.child(div().px_1().text_xs().text_color(muted).child(lang))
                })
                .child(
                    Clipboard::new("copy")
                        .value(block.code())
                        .tooltip("Copy code"),
                )
        })
}

/// The agent's reasoning: one line until expanded.
pub fn thought(
    row: usize,
    text: &str,
    live: bool,
    rows: &mut RowState,
    cx: &mut Context<Transcript>,
) -> AnyElement {
    let toggle = Toggle::Row(row);
    let expanded = rows.is_expanded(&toggle, false);
    let colors = rows.colors().clone();
    let preview = if expanded {
        None
    } else {
        latest_line(text).map(|line| super::code_spans(line, &colors))
    };
    let header = h_flex()
        .id(ElementId::NamedInteger("thought".into(), row as u64))
        .gap_2()
        .items_center()
        .text_sm()
        .text_color(colors.muted_foreground)
        .cursor_pointer()
        .hover(|style| style.text_color(colors.foreground))
        .child(
            Icon::new(if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall(),
        )
        .child(if live {
            Spinner::new()
                .xsmall()
                .color(colors.muted_foreground)
                .into_any_element()
        } else {
            Icon::new(IconName::Bot).xsmall().into_any_element()
        })
        .child(
            div()
                .flex_shrink_0()
                .child(if live { "Thinking" } else { "Thought" }),
        )
        .when_some(preview, |header, preview| {
            header.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .italic()
                    .text_color(colors.muted_foreground.opacity(0.75))
                    .child(preview),
            )
        })
        .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), expanded, cx)));
    let body = expanded.then(|| {
        let state = rows.text_state(row, text, cx);
        div()
            .ml(px(7.))
            .pl_4()
            .border_l_2()
            .border_color(colors.border)
            .text_sm()
            .text_color(colors.muted_foreground)
            .child(markdown(&state))
    });
    v_flex()
        .w_full()
        .gap_2()
        .child(header)
        .when_some(body, |column, body| column.child(body))
        .into_any_element()
}

/// The last line of `text` with something in it, without markdown emphasis, for a preview.
fn latest_line(text: &str) -> Option<&str> {
    text.rsplit('\n')
        .map(|line| line.trim().trim_matches('*').trim())
        .find(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::latest_line;

    #[test]
    fn a_thoughts_preview_is_its_latest_line() {
        assert_eq!(
            latest_line("\n\n**Creating hello.txt**\nRunning it now.\n\n"),
            Some("Running it now.")
        );
        assert_eq!(latest_line("**Planning**"), Some("Planning"));
        assert_eq!(latest_line("  \n"), None);
    }
}
