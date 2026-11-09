//! Diffs in tool call cards, as unified diffs: removed lines, then added ones, coloured, with
//! line numbers where they are known (D-034).
//!
//! Agents send diffs in two shapes, and both become [`DiffLines`]:
//! - **Old and new text** (ACP's `diff` content, and Muse's edits made into one, D-026):
//!   [`from_texts`] diffs them line by line. The texts are often only the replaced fragment, so
//!   line numbers are shown only when they can be right: for a new file, or when the tool call's
//!   location gives the fragment's first line.
//! - **A unified diff in text**, as Muse prints after an edit: [`parse_unified`] reads its hunks,
//!   whose headers give the line numbers.

use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, ElementId, FontWeight, IntoElement, ParentElement as _, SharedString,
    Styled as _, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Sizable as _, h_flex, v_flex};
use similar::{ChangeTag, TextDiff};

use super::{Colors, Transcript};

/// Lines shown before "Show all" in a collapsed diff.
pub const COLLAPSED_LINES: usize = 24;
/// The longest a line diff may take before it settles for a coarser answer.
const DIFF_TIMEOUT: Duration = Duration::from_millis(50);

/// What a line of a diff is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// Unchanged, shown for context.
    Context,
    /// Added.
    Added,
    /// Removed.
    Removed,
    /// A hunk header: the lines between hunks are left out.
    Hunk,
}

/// One line of a diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// What it is.
    pub kind: LineKind,
    /// Its number in the old text, when known.
    pub old: Option<u32>,
    /// Its number in the new text, when known.
    pub new: Option<u32>,
    /// The line, without its line break; a hunk header's section text.
    pub text: String,
}

/// One file's diff, ready to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffLines {
    /// The file, when the diff names it.
    pub path: Option<String>,
    /// The lines, in order.
    pub lines: Vec<DiffLine>,
    /// How many lines were added.
    pub added: usize,
    /// How many lines were removed.
    pub removed: usize,
    /// Whether the line numbers are real, so worth showing.
    pub numbered: bool,
}

impl DiffLines {
    fn push(&mut self, line: DiffLine) {
        match line.kind {
            LineKind::Added => self.added += 1,
            LineKind::Removed => self.removed += 1,
            LineKind::Context | LineKind::Hunk => {}
        }
        self.lines.push(line);
    }
}

/// The diff from `old` to `new`, with three lines of context; a new file when `old` is absent.
/// `first_line` is the 1-based line `old` starts at, when the agent said.
pub fn from_texts(
    path: Option<&str>,
    old: Option<&str>,
    new: &str,
    first_line: Option<u32>,
) -> DiffLines {
    let mut diff = DiffLines {
        path: path.map(str::to_owned),
        ..Default::default()
    };
    let Some(old) = old else {
        // A new file: every line is added, numbered from 1.
        diff.numbered = true;
        for (ix, line) in new.lines().enumerate() {
            diff.push(DiffLine {
                kind: LineKind::Added,
                old: None,
                new: Some(ix as u32 + 1),
                text: line.to_owned(),
            });
        }
        return diff;
    };
    diff.numbered = first_line.is_some();
    let offset = first_line.unwrap_or(1).saturating_sub(1);
    let text_diff = TextDiff::configure()
        .timeout(DIFF_TIMEOUT)
        .diff_lines(old, new);
    let groups = text_diff.grouped_ops(3);
    let several = groups.len() > 1;
    for group in groups {
        if several && let Some(first) = group.first() {
            let (old_start, new_start) = (first.old_range().start, first.new_range().start);
            diff.push(DiffLine {
                kind: LineKind::Hunk,
                old: Some(old_start as u32 + 1 + offset),
                new: Some(new_start as u32 + 1 + offset),
                text: String::new(),
            });
        }
        for op in group {
            for change in text_diff.iter_changes(&op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => LineKind::Context,
                    ChangeTag::Delete => LineKind::Removed,
                    ChangeTag::Insert => LineKind::Added,
                };
                let number = |ix: Option<usize>| ix.map(|ix| ix as u32 + 1 + offset);
                diff.push(DiffLine {
                    kind,
                    old: number(change.old_index()),
                    new: number(change.new_index()),
                    text: change.value().trim_end_matches(['\n', '\r']).to_owned(),
                });
            }
        }
    }
    diff
}

/// The files of a unified diff in `text`, or `None` when it has no hunk. Text before the first
/// file or hunk, such as Muse's "Edited …" line or `diff --git` headers, is skipped.
pub fn parse_unified(text: &str) -> Option<Vec<DiffLines>> {
    let mut files: Vec<DiffLines> = Vec::new();
    let mut old_path: Option<String> = None;
    // The next numbers in the current hunk, while in one.
    let mut hunk: Option<(u32, u32)> = None;
    let mut found_hunk = false;
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix("--- ")
            && hunk.is_none_or(|_| line.starts_with("--- a/") || rest.starts_with('/'))
        {
            old_path = Some(strip_side(rest, "a/"));
            hunk = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ")
            && (hunk.is_none() || old_path.is_some())
        {
            let new_path = strip_side(rest, "b/");
            let path = if new_path == "/dev/null" {
                old_path.take()
            } else {
                old_path = None;
                Some(new_path)
            };
            files.push(DiffLines {
                path,
                numbered: true,
                ..Default::default()
            });
            hunk = None;
            continue;
        }
        if let Some((old, new, section)) = hunk_header(line) {
            found_hunk = true;
            if files.is_empty() {
                files.push(DiffLines {
                    numbered: true,
                    ..Default::default()
                });
            }
            let file = files.last_mut().expect("pushed above");
            file.push(DiffLine {
                kind: LineKind::Hunk,
                old: Some(old),
                new: Some(new),
                text: section.to_owned(),
            });
            hunk = Some((old, new));
            continue;
        }
        let Some((old, new)) = hunk.as_mut() else {
            continue;
        };
        let file = files.last_mut().expect("a hunk belongs to a file");
        let (kind, rest) = match line.chars().next() {
            Some('+') => (LineKind::Added, &line[1..]),
            Some('-') => (LineKind::Removed, &line[1..]),
            Some(' ') => (LineKind::Context, &line[1..]),
            // "\ No newline at end of file".
            Some('\\') => continue,
            // An empty line inside a hunk is an empty context line, as some tools print it.
            None => (LineKind::Context, ""),
            // Anything else ends the hunk: the diff is over, or text follows it.
            Some(_) => {
                hunk = None;
                continue;
            }
        };
        let (old_no, new_no) = match kind {
            LineKind::Added => (None, Some(*new)),
            LineKind::Removed => (Some(*old), None),
            _ => (Some(*old), Some(*new)),
        };
        if old_no.is_some() {
            *old += 1;
        }
        if new_no.is_some() {
            *new += 1;
        }
        file.push(DiffLine {
            kind,
            old: old_no,
            new: new_no,
            text: rest.to_owned(),
        });
    }
    found_hunk.then(|| {
        files.retain(|file| !file.lines.is_empty());
        files
    })
}

/// A diff header's path, without its `a/` or `b/` and any trailing timestamp.
fn strip_side(path: &str, side: &str) -> String {
    let path = path.split('\t').next().unwrap_or(path).trim();
    path.strip_prefix(side).unwrap_or(path).to_owned()
}

/// `@@ -12,3 +12,4 @@ fn main()`: the first old and new line, and the section text.
fn hunk_header(line: &str) -> Option<(u32, u32, &str)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let start = |range: &str| range.split(',').next()?.parse::<u32>().ok();
    Some((start(old)?, start(new)?, section.trim()))
}

/// Whether `text` holds a unified diff worth drawing as one.
pub fn looks_unified(text: &str) -> bool {
    text.contains("\n@@ -") || text.starts_with("@@ -")
}

/// Renders `diff`: a header with the path and counts, then its lines, the first
/// [`COLLAPSED_LINES`] unless `expanded`. `toggle` is the key [`Transcript::toggle`] flips.
pub fn render(
    diff: &DiffLines,
    display_path: Option<String>,
    expanded: bool,
    toggle: super::Toggle,
    colors: &Colors,
    cx: &mut Context<Transcript>,
) -> AnyElement {
    let shown = if expanded || diff.lines.len() <= COLLAPSED_LINES + super::CUT_SLACK {
        diff.lines.len()
    } else {
        COLLAPSED_LINES
    };
    let hidden = diff.lines.len() - shown;
    let gutter = |number: Option<u32>| {
        div()
            .w(px(34.))
            .flex_shrink_0()
            .pr_1()
            .text_right()
            .text_color(colors.muted_foreground.opacity(0.6))
            .when_some(number, |d, n| d.child(n.to_string()))
    };
    let lines = diff.lines[..shown].iter().map(|line| {
        let (marker, bg, fg) = match line.kind {
            LineKind::Added => ("+", colors.success.opacity(0.13), colors.foreground),
            LineKind::Removed => ("-", colors.danger.opacity(0.13), colors.foreground),
            LineKind::Context => (
                " ",
                gpui::transparent_black(),
                colors.foreground.opacity(0.8),
            ),
            LineKind::Hunk => ("", colors.info.opacity(0.08), colors.muted_foreground),
        };
        if line.kind == LineKind::Hunk {
            // A fragment's numbers aren't the file's: say only that lines were left out.
            let text = match (line.old, line.new) {
                (Some(old), Some(new)) if diff.numbered => {
                    format!("@@ -{old} +{new} @@ {}", line.text)
                }
                _ => "⋯".to_owned(),
            };
            return div()
                .w_full()
                .px_2()
                .bg(bg)
                .text_color(fg)
                .child(text)
                .into_any_element();
        }
        h_flex()
            .w_full()
            .items_start()
            .bg(bg)
            .when(diff.numbered, |row| {
                row.child(gutter(line.old)).child(gutter(line.new))
            })
            .child(
                div()
                    .w(px(14.))
                    .flex_shrink_0()
                    .text_center()
                    .text_color(match line.kind {
                        LineKind::Added => colors.success,
                        LineKind::Removed => colors.danger,
                        _ => colors.muted_foreground,
                    })
                    .child(marker),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pr_2()
                    .text_color(fg)
                    .child(SharedString::from(if line.text.is_empty() {
                        " ".to_owned()
                    } else {
                        line.text.replace('\t', "    ")
                    })),
            )
            .into_any_element()
    });
    let toggle_id = ElementId::Name(format!("diff-toggle-{}", toggle.id()).into());
    v_flex()
        .w_full()
        .rounded(px(6.))
        .border_1()
        .border_color(colors.border)
        .overflow_hidden()
        .bg(colors.code_background)
        .child(
            h_flex()
                .px_2()
                .py_1()
                .gap_2()
                .border_b_1()
                .border_color(colors.border)
                .text_xs()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(colors.muted_foreground)
                        .child(display_path.unwrap_or_else(|| "Changes".to_owned())),
                )
                .when(diff.added > 0, |row| {
                    row.child(
                        div()
                            .text_color(colors.success)
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("+{}", diff.added)),
                    )
                })
                .when(diff.removed > 0, |row| {
                    row.child(
                        div()
                            .text_color(colors.danger)
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("−{}", diff.removed)),
                    )
                }),
        )
        .child(
            v_flex()
                .w_full()
                .py_1()
                .font_family(colors.mono_font.clone())
                .text_xs()
                .line_height(px(18.))
                .children(lines),
        )
        .when(
            hidden > 0 || (expanded && diff.lines.len() > COLLAPSED_LINES + super::CUT_SLACK),
            |card| {
                let label = if expanded {
                    "Show less".to_owned()
                } else {
                    super::show_more_label(hidden)
                };
                card.child(
                    h_flex().px_1().pb_1().child(
                        Button::new(toggle_id)
                            .ghost()
                            .xsmall()
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle(toggle.clone(), expanded, cx)
                            })),
                    ),
                )
            },
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(diff: &DiffLines) -> String {
        diff.lines
            .iter()
            .map(|line| match line.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Removed => '-',
                LineKind::Hunk => '@',
            })
            .collect()
    }

    #[test]
    fn old_and_new_text_become_a_unified_diff() {
        let old = "fn main() {\n    println!(\"hi\");\n}\n";
        let new = "fn main() {\n    println!(\"hello\");\n    println!(\"bye\");\n}\n";
        let diff = from_texts(Some("src/main.rs"), Some(old), new, None);
        assert_eq!(kinds(&diff), " -++ ");
        assert_eq!((diff.added, diff.removed), (2, 1));
        // A fragment's numbers aren't the file's, so they aren't shown.
        assert!(!diff.numbered);
        assert_eq!(diff.lines[1].text, "    println!(\"hi\");");

        // With the fragment's first line from the tool call's location, they are.
        let diff = from_texts(None, Some(old), new, Some(10));
        assert!(diff.numbered);
        assert_eq!((diff.lines[0].old, diff.lines[0].new), (Some(10), Some(10)));
        assert_eq!((diff.lines[3].old, diff.lines[3].new), (None, Some(12)));
    }

    #[test]
    fn far_apart_changes_get_hunk_headers() {
        let old: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        let new = old
            .replace("line 3\n", "line three\n")
            .replace("line 30\n", "");
        let diff = from_texts(None, Some(&old), &new, None);
        let hunks: Vec<_> = diff
            .lines
            .iter()
            .filter(|line| line.kind == LineKind::Hunk)
            .map(|line| (line.old, line.new))
            .collect();
        assert_eq!(hunks, [(Some(1), Some(1)), (Some(27), Some(27))]);
        assert_eq!((diff.added, diff.removed), (1, 2));
    }

    #[test]
    fn a_new_file_is_numbered_from_one() {
        let diff = from_texts(Some("notes.txt"), None, "a\nb\n", None);
        assert_eq!(kinds(&diff), "++");
        assert!(diff.numbered);
        assert_eq!(diff.lines[1].new, Some(2));
    }

    #[test]
    fn unified_diffs_in_text_are_parsed_with_their_line_numbers() {
        let text = "Edited D:\\repo\\src\\lib.rs\n\
                    --- a/src/lib.rs\n\
                    +++ b/src/lib.rs\n\
                    @@ -10,3 +10,4 @@ fn helper()\n \
                    keep\n\
                    -old\n\
                    +new\n\
                    +more\n \
                    tail\n\
                    \\ No newline at end of file\n\
                    @@ -40 +41 @@\n\
                    -x\n\
                    +y\n\
                    Applied 1 edit.\n";
        assert!(looks_unified(text));
        let files = parse_unified(text).unwrap();
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.path.as_deref(), Some("src/lib.rs"));
        assert_eq!(kinds(file), "@ -++ @-+");
        assert_eq!(file.lines[0].text, "fn helper()");
        // Context lines carry both numbers, changes one side's.
        assert_eq!((file.lines[1].old, file.lines[1].new), (Some(10), Some(10)));
        assert_eq!((file.lines[2].old, file.lines[2].new), (Some(11), None));
        assert_eq!((file.lines[4].old, file.lines[4].new), (None, Some(12)));
        assert_eq!((file.lines[5].old, file.lines[5].new), (Some(12), Some(13)));
        assert_eq!((file.lines[7].old, file.lines[8].new), (Some(40), Some(41)));
        assert_eq!((file.added, file.removed), (3, 2));
    }

    #[test]
    fn several_files_and_new_files() {
        let text = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n\
                    --- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n";
        let files = parse_unified(text).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.as_deref()).collect();
        assert_eq!(paths, [Some("new.txt"), Some("old.txt")]);
        assert_eq!(files[0].lines[1].new, Some(1));
        assert_eq!(files[1].removed, 1);
    }

    #[test]
    fn text_without_a_hunk_is_not_a_diff() {
        assert_eq!(parse_unified("wrote 3 bytes to hello.txt"), None);
        assert_eq!(parse_unified("--- a/x\n+++ b/x\n"), None);
        assert!(!looks_unified("a - b\n+ c"));
    }
}
