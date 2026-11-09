//! Tool call cards, and groups of reads and searches.
//!
//! A card is one line: an icon for the kind, the title, the file it touches, and the status.
//! A click opens what the call produced:
//! - **diffs**, drawn by [`super::diff`]: ACP's old and new text, and the unified diff Muse
//!   prints after an edit;
//! - **command output** in a monospace block; Muse's JSON result shows its output and exit code;
//! - **text**, as markdown, since agents fence their output (Claude Code's console blocks);
//! - a **terminal** reference, which Burrow can't show until M4.
//!
//! Edits with a diff, and failures, start open; everything else starts closed. Long output and
//! long diffs are cut short, with a button for the rest.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::spinner::Spinner;
use gpui_component::text::TextView;
use gpui_component::{Icon, IconName, Sizable as _, h_flex, v_flex};

use burrow_protocol::{ContentBlock, ToolCallContent, ToolCallStatus, ToolKind};
use serde_json::Value;

use super::diff::{self, DiffLines};
use super::{Colors, RowState, Toggle, Transcript};
use crate::state::transcript::ToolCallItem;

/// Lines of output shown before "Show all".
const COLLAPSED_OUTPUT_LINES: usize = 16;

/// The icon for a tool kind.
pub fn kind_icon(kind: &ToolKind) -> IconName {
    match kind {
        ToolKind::Read => IconName::FileText,
        ToolKind::Edit => IconName::Replace,
        ToolKind::Delete => IconName::Delete,
        ToolKind::Move => IconName::ArrowRight,
        ToolKind::Search => IconName::Search,
        ToolKind::Execute => IconName::SquareTerminal,
        ToolKind::Think => IconName::Bot,
        ToolKind::Fetch => IconName::Globe,
        ToolKind::SwitchMode => IconName::Settings2,
        ToolKind::Misc | ToolKind::Other(_) => IconName::Asterisk,
    }
}

/// Whether a card starts open: edits that carry a diff, and failures that say why.
fn opens_by_default(call: &ToolCallItem) -> bool {
    let has_diff = call.content.iter().any(|content| match content {
        ToolCallContent::Diff { .. } => true,
        ToolCallContent::Content {
            content: ContentBlock::Text { text },
        } => matches!(call.kind, ToolKind::Edit) && diff::looks_unified(text),
        _ => false,
    });
    has_diff || (call.status == ToolCallStatus::Failed && !call.content.is_empty())
}

/// The status mark at the end of a card's line.
fn status_mark(status: &ToolCallStatus, colors: &Colors) -> AnyElement {
    match status {
        ToolCallStatus::InProgress => Spinner::new()
            .xsmall()
            .color(colors.warning)
            .into_any_element(),
        ToolCallStatus::Completed => Icon::new(IconName::CircleCheck)
            .xsmall()
            .text_color(colors.success)
            .into_any_element(),
        ToolCallStatus::Failed => Icon::new(IconName::CircleX)
            .xsmall()
            .text_color(colors.danger)
            .into_any_element(),
        ToolCallStatus::Pending => super::ring(colors.muted_foreground, false).into_any_element(),
        ToolCallStatus::Other(other) => div()
            .text_xs()
            .text_color(colors.muted_foreground)
            .child(other.clone())
            .into_any_element(),
    }
}

/// Where a call works, when its title doesn't already say: its first location, relative to
/// the session's folder, plus how many more.
fn location_label(call: &ToolCallItem, rows: &RowState) -> Option<String> {
    let first = call.locations.first()?;
    let path = rows.display_path(&first.path);
    let file_name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
    if call.title.contains(file_name) && call.locations.len() == 1 {
        return None;
    }
    let mut label = match first.line {
        Some(line) => format!("{path}:{line}"),
        None => path,
    };
    if call.locations.len() > 1 {
        label.push_str(&format!(" +{}", call.locations.len() - 1));
    }
    Some(label)
}

/// One tool call: its line, and its content when open. `nested` draws it inside a group,
/// without its own border.
pub fn card(
    call: &ToolCallItem,
    nested: bool,
    rows: &mut RowState,
    cx: &mut Context<Transcript>,
) -> AnyElement {
    let colors = rows.colors().clone();
    let toggle = Toggle::Tool(call.tool_call_id.clone());
    let openable = !call.content.is_empty();
    let open = openable && rows.is_expanded(&toggle, !nested && opens_by_default(call));
    let location = location_label(call, rows);
    let header = h_flex()
        .id(ElementId::Name(
            format!("tool-{}", call.tool_call_id).into(),
        ))
        .w_full()
        .gap_2()
        .px_3()
        .py(px(5.))
        .items_center()
        .text_sm()
        .child(
            Icon::new(kind_icon(&call.kind))
                .small()
                .text_color(colors.muted_foreground),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .when(!open, |title| title.truncate())
                .child(super::code_spans(&rows.shorten_paths(&call.title), &colors)),
        )
        .when_some(location, |header, location| {
            header.child(
                div()
                    .max_w(px(260.))
                    .flex_shrink(1.)
                    .truncate()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(location),
            )
        })
        .child(status_mark(&call.status, &colors))
        .when(openable, |header| {
            header
                .cursor_pointer()
                .hover(|style| style.bg(colors.muted.opacity(0.6)))
                .child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .xsmall()
                    .text_color(colors.muted_foreground),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), open, cx)))
        });
    // Muse reports an edit twice: the replaced fragment, and a unified diff of the file with its
    // line numbers. Show only the second.
    let has_unified = call.content.iter().any(|content| {
        matches!(content, ToolCallContent::Content { content: ContentBlock::Text { text } }
            if diff::looks_unified(text))
    });
    let body = open.then(|| {
        v_flex().w_full().gap_2().px_3().pb_3().pt_1().children(
            call.content
                .iter()
                .enumerate()
                .filter(|(_, content)| {
                    !(has_unified && matches!(content, ToolCallContent::Diff { .. }))
                })
                .map(|(ix, content)| self::content(call, ix, content, rows, cx)),
        )
    });
    v_flex()
        .w_full()
        .when(!nested, |card| {
            card.rounded(colors.radius)
                .border_1()
                .border_color(colors.border)
                .bg(colors.background)
                .overflow_hidden()
        })
        .child(header)
        .when_some(body, |card, body| card.child(body))
        .into_any_element()
}

/// One piece of a call's content.
fn content(
    call: &ToolCallItem,
    ix: usize,
    content: &ToolCallContent,
    rows: &mut RowState,
    cx: &mut Context<Transcript>,
) -> AnyElement {
    let colors = rows.colors().clone();
    let toggle = Toggle::Content(call.tool_call_id.clone(), ix);
    let full = rows.is_expanded(&toggle, false);
    match content {
        ToolCallContent::Diff {
            path,
            old_text,
            new_text,
        } => {
            let first_line = call
                .locations
                .iter()
                .find(|location| location.path == *path)
                .and_then(|location| location.line);
            let diffs = rows.diffs(&call.tool_call_id, ix, call.revision, || {
                vec![diff::from_texts(
                    Some(path),
                    old_text.as_deref(),
                    new_text,
                    first_line,
                )]
            });
            diff_list(&diffs, full, toggle, rows, cx)
        }
        ToolCallContent::Content {
            content: ContentBlock::Text { text },
        } => {
            if diff::looks_unified(text) {
                let diffs = rows.diffs(&call.tool_call_id, ix, call.revision, || {
                    diff::parse_unified(text).unwrap_or_default()
                });
                if !diffs.is_empty() {
                    return diff_list(&diffs, full, toggle, rows, cx);
                }
            }
            if call.kind == ToolKind::Execute {
                if let Some((output, exit_code)) = muse_command_result(text) {
                    return command_output(&output, exit_code, full, toggle, &colors, cx);
                }
                if !text.contains("```") {
                    return command_output(text, None, full, toggle, &colors, cx);
                }
            }
            markdown_output(call, ix, text, full, toggle, &colors, cx)
        }
        ToolCallContent::Content {
            content: ContentBlock::Resource { uri },
        } => quiet_line(uri, &colors),
        ToolCallContent::Content {
            content: ContentBlock::Image { .. },
        } => quiet_line("An image", &colors),
        ToolCallContent::Terminal { terminal_id } => quiet_line(
            &format!("Ran in terminal {terminal_id}; Burrow shows terminals from M4."),
            &colors,
        ),
    }
}

fn diff_list(
    diffs: &[DiffLines],
    full: bool,
    toggle: Toggle,
    rows: &RowState,
    cx: &mut Context<Transcript>,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_2()
        .children(diffs.iter().map(|lines| {
            let path = lines.path.as_deref().map(|path| rows.display_path(path));
            diff::render(lines, path, full, toggle.clone(), rows.colors(), cx)
        }))
        .into_any_element()
}

/// Muse's shell result, a JSON object with the command's `output` and `exit_code`.
fn muse_command_result(text: &str) -> Option<(String, Option<i64>)> {
    if !text.trim_start().starts_with('{') {
        return None;
    }
    let value: Value = serde_json::from_str(text).ok()?;
    let output = value.get("output")?.as_str()?.to_owned();
    let exit_code = value.get("exit_code").and_then(Value::as_i64);
    Some((output, exit_code))
}

/// The first `limit` lines of `text`, and how many more there are. Text that is only a few lines
/// longer than `limit` isn't cut: a button to show them would take as much room.
fn first_lines(text: &str, limit: usize) -> (&str, usize) {
    let mut end = text.len();
    let mut lines = 0;
    for (ix, _) in text.match_indices('\n') {
        lines += 1;
        if lines == limit {
            end = ix;
            break;
        }
    }
    if end == text.len() {
        return (text, 0);
    }
    let rest = text[end..].trim_end_matches('\n');
    match rest.matches('\n').count() {
        more if more <= super::CUT_SLACK => (text, 0),
        more => (&text[..end], more),
    }
}

/// Command output in a monospace block, cut short unless `full`.
fn command_output(
    text: &str,
    exit_code: Option<i64>,
    full: bool,
    toggle: Toggle,
    colors: &Colors,
    cx: &mut Context<Transcript>,
) -> AnyElement {
    let text = text.trim_end_matches(['\n', '\r']);
    let (shown, more) = if full {
        (text, 0)
    } else {
        first_lines(text, COLLAPSED_OUTPUT_LINES)
    };
    let total_more = first_lines(text, COLLAPSED_OUTPUT_LINES).1;
    v_flex()
        .w_full()
        .gap_1()
        .when(!text.is_empty(), |column| {
            column.child(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded(colors.radius)
                    .bg(colors.code_background)
                    .border_1()
                    .border_color(colors.border)
                    .font_family(colors.mono_font.clone())
                    .text_xs()
                    .line_height(px(18.))
                    .child(SharedString::from(shown.replace('\t', "    "))),
            )
        })
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_xs()
                .text_color(colors.muted_foreground)
                .when(text.is_empty(), |row| row.child("No output"))
                .when_some(exit_code, |row, code| {
                    row.child(
                        div()
                            .when(code != 0, |d| d.text_color(colors.danger))
                            .child(format!("Exit code {code}")),
                    )
                })
                .when(total_more > 0, |row| {
                    row.child(show_more(&toggle, full, more, cx))
                }),
        )
        .into_any_element()
}

/// Text output as markdown, cut short unless `full`.
fn markdown_output(
    call: &ToolCallItem,
    ix: usize,
    text: &str,
    full: bool,
    toggle: Toggle,
    colors: &Colors,
    cx: &mut Context<Transcript>,
) -> AnyElement {
    let (shown, more) = if full {
        (text, 0)
    } else {
        first_lines(text, COLLAPSED_OUTPUT_LINES)
    };
    let total_more = first_lines(text, COLLAPSED_OUTPUT_LINES).1;
    let mut shown = shown.to_owned();
    // Cutting inside a code fence would turn the rest into code: close it.
    if shown.matches("```").count() % 2 == 1 {
        shown.push_str("\n```");
    }
    v_flex()
        .w_full()
        .gap_1()
        .text_sm()
        .text_color(colors.muted_foreground)
        .child(
            TextView::markdown(
                ElementId::Name(format!("tool-text-{}-{ix}-{full}", call.tool_call_id).into()),
                shown,
            )
            .selectable(true),
        )
        .when(total_more > 0, |column| {
            column.child(h_flex().child(show_more(&toggle, full, more, cx)))
        })
        .into_any_element()
}

fn show_more(toggle: &Toggle, full: bool, more: usize, cx: &mut Context<Transcript>) -> Button {
    let label = if full {
        "Show less".to_owned()
    } else {
        super::show_more_label(more)
    };
    let toggle = toggle.clone();
    Button::new(ElementId::Name(format!("more-{}", toggle.id()).into()))
        .ghost()
        .xsmall()
        .label(label)
        .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), full, cx)))
}

fn quiet_line(text: &str, colors: &Colors) -> AnyElement {
    div()
        .text_xs()
        .text_color(colors.muted_foreground)
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

/// A run of reads and searches: one line that says how many, opening to their cards.
pub fn group(
    row: usize,
    calls: &[ToolCallItem],
    rows: &mut RowState,
    cx: &mut Context<Transcript>,
) -> AnyElement {
    let colors = rows.colors().clone();
    let toggle = Toggle::Row(row);
    let open = rows.is_expanded(&toggle, false);
    let reads = calls.iter().filter(|c| c.kind == ToolKind::Read).count();
    let searches = calls.iter().filter(|c| c.kind == ToolKind::Search).count();
    let others = calls.len() - reads - searches;
    let mut parts = Vec::new();
    if reads > 0 {
        parts.push(format!("Read {reads} file{}", plural(reads)));
    }
    if searches > 0 {
        parts.push(format!(
            "{searches} search{}",
            if searches == 1 { "" } else { "es" }
        ));
    }
    if others > 0 {
        parts.push(format!("{others} other call{}", plural(others)));
    }
    let failed = calls
        .iter()
        .filter(|c| c.status == ToolCallStatus::Failed)
        .count();
    let running = calls.iter().find(|c| {
        matches!(
            c.status,
            ToolCallStatus::InProgress | ToolCallStatus::Pending
        )
    });
    let status = match running {
        Some(call) => status_mark(&call.status, &colors),
        None if failed > 0 => div()
            .text_xs()
            .text_color(colors.danger)
            .child(format!("{failed} failed"))
            .into_any_element(),
        None => status_mark(&ToolCallStatus::Completed, &colors),
    };
    let header = h_flex()
        .id(ElementId::NamedInteger("tool-group".into(), row as u64))
        .w_full()
        .gap_2()
        .px_3()
        .py(px(5.))
        .items_center()
        .text_sm()
        .cursor_pointer()
        .hover(|style| style.bg(colors.muted.opacity(0.6)))
        .child(
            Icon::new(IconName::Search)
                .small()
                .text_color(colors.muted_foreground),
        )
        .child(div().flex_shrink_0().child(parts.join(", ")))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(colors.muted_foreground)
                .when_some(running.filter(|_| !open), |latest, call| {
                    latest.child(super::code_spans(&rows.shorten_paths(&call.title), &colors))
                }),
        )
        .child(status)
        .child(
            Icon::new(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall()
            .text_color(colors.muted_foreground),
        )
        .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), open, cx)));
    let body = open.then(|| {
        v_flex()
            .w_full()
            .ml_5()
            .pr_5()
            .pb_2()
            .border_l_1()
            .border_color(colors.border)
            .children(calls.iter().map(|call| card(call, true, rows, cx)))
    });
    v_flex()
        .w_full()
        .rounded(colors.radius)
        .border_1()
        .border_color(colors.border)
        .bg(colors.background)
        .overflow_hidden()
        .child(header)
        .when_some(body, |group, body| group.child(body))
        .into_any_element()
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muses_shell_result_gives_its_output_and_exit_code() {
        let text =
            "{\n  \"command\": \"echo hi\",\n  \"exit_code\": 0,\n  \"output\": \"hi\\n\"\n}";
        assert_eq!(
            muse_command_result(text),
            Some(("hi\n".to_owned(), Some(0)))
        );
        assert_eq!(muse_command_result("hi"), None);
        assert_eq!(muse_command_result("{\"other\": 1}"), None);
    }

    #[test]
    fn output_is_cut_after_its_first_lines() {
        let text: String = (1..=30).map(|i| format!("{i}\n")).collect();
        let (shown, more) = first_lines(&text, 16);
        assert_eq!(shown.lines().count(), 16);
        assert_eq!(more, 14);
        assert_eq!(first_lines("a\nb", 16), ("a\nb", 0));
        // A few lines more aren't worth a button.
        let text: String = (1..=19).map(|i| format!("{i}\n")).collect();
        assert_eq!(first_lines(&text, 16), (text.as_str(), 0));
    }
}
