//! The other rows: plans, permission requests, turn ends, limits and notices.

use burrow_protocol::{
    PermissionOptionKind, PlanEntry, PlanEntryPriority, PlanEntryStatus, Timestamp, TurnEndReason,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, ElementId, Entity, FontWeight, IntoElement, ParentElement as _, SharedString,
    Styled as _, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, IconName, Sizable as _, h_flex, v_flex};

use super::Colors;
use crate::state::AppState;
use crate::state::transcript::{Expiry, PermissionItem, PermissionOutcome, Tone, TurnEnd};

/// The agent's plan, as a checklist.
pub fn plan(entries: &[PlanEntry], colors: &Colors) -> AnyElement {
    let done = entries
        .iter()
        .filter(|entry| entry.status == PlanEntryStatus::Completed)
        .count();
    v_flex()
        .w_full()
        .gap_1()
        .px_4()
        .py_3()
        .rounded(colors.radius)
        .border_1()
        .border_color(colors.border)
        .child(
            h_flex()
                .gap_2()
                .pb_1()
                .items_center()
                .text_sm()
                .child(
                    Icon::new(IconName::Check)
                        .small()
                        .text_color(colors.muted_foreground),
                )
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Plan"))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .child(format!("{done} of {} done", entries.len())),
                ),
        )
        .children(entries.iter().map(|entry| {
            let mark = match entry.status {
                PlanEntryStatus::Completed => Icon::new(IconName::CircleCheck)
                    .xsmall()
                    .text_color(colors.success)
                    .into_any_element(),
                PlanEntryStatus::InProgress => super::ring(colors.warning, true).into_any_element(),
                _ => super::ring(colors.muted_foreground, false).into_any_element(),
            };
            let completed = entry.status == PlanEntryStatus::Completed;
            let current = entry.status == PlanEntryStatus::InProgress;
            h_flex()
                .gap_2()
                .items_start()
                .text_sm()
                .child(div().pt(px(3.)).child(mark))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(completed, |text| {
                            text.line_through().text_color(colors.muted_foreground)
                        })
                        .when(current, |text| text.font_weight(FontWeight::MEDIUM))
                        .child(super::code_spans(&entry.content, colors)),
                )
                .when(
                    entry.priority == PlanEntryPriority::High && !completed,
                    |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(colors.warning.opacity(0.8))
                                .child("high"),
                        )
                    },
                )
        }))
        .into_any_element()
}

/// What an expired request's outcome line says.
fn expiry_text(expiry: &Expiry) -> String {
    match expiry {
        Expiry::Pending | Expiry::NoTurn => "Expired without an answer".to_owned(),
        Expiry::Withdrawn => "Withdrawn: the agent stopped waiting for an answer".to_owned(),
        Expiry::TurnEnded {
            message: Some(message),
            ..
        } => format!("Expired: {message}"),
        Expiry::TurnEnded {
            reason: TurnEndReason::Cancelled,
            ..
        } => "Expired: the turn was cancelled".to_owned(),
        Expiry::TurnEnded { .. } => "Expired: the turn ended".to_owned(),
    }
}

/// A permission request: its buttons while open, then how it ended.
/// `title` is the request's, with paths shortened.
pub fn permission(
    request: &PermissionItem,
    title: String,
    state: &Entity<AppState>,
    colors: &Colors,
) -> AnyElement {
    let open = request.outcome == PermissionOutcome::Open;
    let outcome = match &request.outcome {
        PermissionOutcome::Open => None,
        PermissionOutcome::Chosen { by, .. } => {
            let name = request.chosen_name().unwrap_or_default();
            let by = match by.as_deref() {
                None | Some("local") => String::new(),
                Some(device) => format!(" from {device}"),
            };
            Some((format!("Answered: {name}{by}"), colors.muted_foreground))
        }
        PermissionOutcome::Expired(expiry) => {
            Some((expiry_text(expiry), colors.muted_foreground.opacity(0.85)))
        }
    };
    let buttons = open.then(|| {
        h_flex()
            .flex_wrap()
            .gap_2()
            .children(request.options.iter().map(|option| {
                let state = state.clone();
                let item_id = request.inbox_item_id.clone();
                let action = option.option_id.clone();
                let button = Button::new(ElementId::Name(
                    format!("perm-{}-{}", request.inbox_item_id, option.option_id).into(),
                ))
                .small()
                .label(option.name.clone());
                let button = match option.kind {
                    PermissionOptionKind::AllowOnce => button.primary(),
                    PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways => {
                        button.danger()
                    }
                    _ => button.outline(),
                };
                button.on_click(move |_, window, cx| {
                    crate::views::inbox::respond(
                        &state,
                        item_id.clone(),
                        action.clone(),
                        window,
                        cx,
                    )
                })
            }))
    });
    let accent = if open {
        colors.primary
    } else {
        colors.muted_foreground
    };
    v_flex()
        .w_full()
        .gap_2()
        .px_4()
        .py_3()
        .rounded(colors.radius)
        .border_1()
        .border_color(if open { colors.primary } else { colors.border })
        .when(open, |card| card.bg(colors.primary.opacity(0.08)))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(accent),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::MEDIUM)
                        .child(super::code_spans(&title, colors)),
                )
                .when(open, |row| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(colors.primary)
                            .child("Needs your answer"),
                    )
                }),
        )
        .when_some(buttons, |card, buttons| card.child(buttons))
        .when_some(outcome, |card, (text, color)| {
            card.child(div().text_xs().text_color(color).child(text))
        })
        .into_any_element()
}

/// 1234 → "1.2k", 53985 → "54k".
pub fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..10_000 => format!("{:.1}k", count as f64 / 1_000.0),
        10_000..1_000_000 => format!("{}k", (count as f64 / 1_000.0).round()),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

/// 800 → "0.8s", 65000 → "1m 5s".
pub fn duration(ms: i64) -> String {
    let ms = ms.max(0);
    match ms {
        0..10_000 => format!("{:.1}s", ms as f64 / 1000.0),
        10_000..60_000 => format!("{}s", ms / 1000),
        60_000..3_600_000 => format!("{}m {}s", ms / 60_000, ms % 60_000 / 1000),
        _ => format!("{}h {}m", ms / 3_600_000, ms % 3_600_000 / 60_000),
    }
}

/// The figures of a turn: how long it took, and what it used.
fn turn_figures(end: &TurnEnd) -> Vec<String> {
    let mut figures = Vec::new();
    if let Some(ms) = end.duration_ms {
        figures.push(duration(ms));
    }
    if let Some(usage) = &end.usage {
        figures.push(format!(
            "{} in · {} out",
            tokens(usage.input_tokens),
            tokens(usage.output_tokens)
        ));
        if let Some(cost) = usage.cost_usd {
            figures.push(if cost < 0.01 {
                "<$0.01".to_owned()
            } else {
                format!("${cost:.2}")
            });
        }
    }
    figures
}

/// The line that ends a turn, with its reason and figures. An error is spelled out in a box.
pub fn turn_end(end: &TurnEnd, colors: &Colors) -> AnyElement {
    let label = match &end.reason {
        TurnEndReason::Completed => "Done".to_owned(),
        TurnEndReason::Cancelled => "Cancelled".to_owned(),
        TurnEndReason::Limit => "Stopped at a usage limit".to_owned(),
        TurnEndReason::Error => "Failed".to_owned(),
        TurnEndReason::Other(other) => other.clone(),
    };
    let mut parts = vec![label];
    parts.extend(turn_figures(end));
    let error = end.reason == TurnEndReason::Error;
    // A cancelled turn's message says who cancelled it; an error's goes in the box below.
    let detail = end
        .message
        .as_ref()
        .filter(|_| !error && end.reason != TurnEndReason::Limit);
    if let Some(detail) = detail {
        parts.insert(1, detail.clone());
    }
    let color = if error {
        colors.danger
    } else {
        colors.muted_foreground.opacity(0.75)
    };
    let line = h_flex()
        .w_full()
        .gap_3()
        .items_center()
        .text_xs()
        .text_color(color)
        .child(div().flex_1().h(px(1.)).bg(colors.border))
        .child(div().flex_shrink_0().child(parts.join(" · ")))
        .child(div().flex_1().h(px(1.)).bg(colors.border));
    v_flex()
        .w_full()
        .gap_2()
        .child(line)
        .when_some(end.message.as_ref().filter(|_| error), |column, message| {
            column.child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .px_3()
                    .py_2()
                    .rounded(colors.radius)
                    .border_1()
                    .border_color(colors.danger.opacity(0.5))
                    .bg(colors.danger.opacity(0.08))
                    .text_sm()
                    .child(
                        div().pt(px(2.)).child(
                            Icon::new(IconName::CircleAlert)
                                .small()
                                .text_color(colors.danger),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(SharedString::from(message.clone())),
                    ),
            )
        })
        .into_any_element()
}

/// When a limit resets, in local time: "Resets at 15:00, in 42 min".
pub fn reset_text(resets_at: Option<Timestamp>, now: Timestamp) -> String {
    let Some(at) = resets_at else {
        return "The provider didn't say when it resets.".to_owned();
    };
    let zone = jiff::tz::TimeZone::system();
    let local = at.as_jiff().to_zoned(zone.clone());
    let today = now.as_jiff().to_zoned(zone).date();
    let clock = if local.date() == today {
        local.strftime("%H:%M").to_string()
    } else {
        local.strftime("%a %-d %b, %H:%M").to_string()
    };
    let left_ms = at.as_unix_millis() - now.as_unix_millis();
    if left_ms <= 0 {
        return format!("It reset at {clock}.");
    }
    let minutes = (left_ms + 59_999) / 60_000;
    let left = match minutes {
        0..60 => format!("{minutes} min"),
        _ if minutes % 60 == 0 => format!("{} h", minutes / 60),
        _ => format!("{} h {} min", minutes / 60, minutes % 60),
    };
    format!("Resets at {clock}, in {left}.")
}

/// A provider limit, as a banner.
pub fn limit(
    provider: &str,
    message: &str,
    resets_at: Option<Timestamp>,
    colors: &Colors,
) -> AnyElement {
    h_flex()
        .w_full()
        .gap_3()
        .items_start()
        .px_4()
        .py_3()
        .rounded(colors.radius)
        .border_1()
        .border_color(colors.warning.opacity(0.6))
        .bg(colors.warning.opacity(0.08))
        .child(
            div().pt(px(2.)).child(
                Icon::new(IconName::BatteryWarning)
                    .small()
                    .text_color(colors.warning),
            ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Usage limit reached"),
                        )
                        .child(
                            div()
                                .px_1p5()
                                .rounded(px(4.))
                                .bg(colors.muted)
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .child(SharedString::from(provider.to_owned())),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .child(SharedString::from(message.to_owned())),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(colors.warning)
                        .child(reset_text(resets_at, Timestamp::now())),
                ),
        )
        .into_any_element()
}

/// A quiet one-liner, or a red one for an error.
pub fn notice(text: &str, tone: Tone, colors: &Colors) -> AnyElement {
    let (icon, color) = match tone {
        Tone::Quiet if text.starts_with("Model") => (IconName::Cpu, colors.muted_foreground),
        Tone::Quiet if text.starts_with("Mode") => (IconName::Settings2, colors.muted_foreground),
        Tone::Quiet => (IconName::Square, colors.muted_foreground),
        Tone::Error => (IconName::CircleAlert, colors.danger),
    };
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(color)
        .child(Icon::new(icon).xsmall())
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

/// An event this version can't show.
pub fn unsupported(type_name: &str, colors: &Colors) -> AnyElement {
    div()
        .w_full()
        .text_center()
        .text_xs()
        .italic()
        .text_color(colors.muted_foreground.opacity(0.6))
        .child(format!("An event this version can't show: {type_name}"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figures_are_short() {
        assert_eq!(tokens(2), "2");
        assert_eq!(tokens(4177), "4.2k");
        assert_eq!(tokens(53_985), "54k");
        assert_eq!(tokens(2_500_000), "2.5M");
        assert_eq!(duration(800), "0.8s");
        assert_eq!(duration(12_400), "12s");
        assert_eq!(duration(65_000), "1m 5s");
        assert_eq!(duration(3_900_000), "1h 5m");
    }

    #[test]
    fn reset_times_say_how_long_is_left() {
        let now = Timestamp::from_unix_millis(1_800_000_000_000).unwrap();
        let later =
            |minutes: i64| Timestamp::from_unix_millis(1_800_000_000_000 + minutes * 60_000).ok();
        assert!(reset_text(later(42), now).ends_with(", in 42 min."));
        assert!(reset_text(later(120), now).ends_with(", in 2 h."));
        assert!(reset_text(later(150), now).ends_with(", in 2 h 30 min."));
        assert!(reset_text(later(-5), now).starts_with("It reset at "));
        assert_eq!(
            reset_text(None, now),
            "The provider didn't say when it resets."
        );
    }
}
