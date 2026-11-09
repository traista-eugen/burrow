//! Detecting usage limits.
//!
//! ACP has no limit error of its own, and each agent reports a limit its own
//! way. Checked against the adapters' sources on 2026-11-09:
//!
//! - **Claude Code** (`claude-agent-acp` 0.101.0) streams the notice as an
//!   agent message, then fails `session/prompt` with `-32603`, message
//!   `"Internal error: You've hit your 5-hour limit · resets 3pm
//!   (Europe/Bucharest)"` and `data: {"errorKind": "rate_limit"}`. The same
//!   `errorKind` also covers short-lived 429s ("Server is temporarily
//!   limiting requests (not your usage limit)"), which are not usage limits.
//!   The reset is local time, with an IANA zone in recent builds.
//! - **Codex** (`codex-acp` 2.6.0) streams the notice, then fails
//!   `session/prompt` with `-32603`, `data: {"message": "You’ve hit your usage
//!   limit. … try again at 3:05 PM.", "codexErrorInfo":
//!   "usageLimitExceeded"}`. The reset is the agent host's local time, with no
//!   zone. Note the curly apostrophe. The turn sometimes never ends after the
//!   notice (codex-acp#747); the client guards against that.
//! - **Cursor** (`agent acp` 2026.11.05) only streams `"\n\nUpgrade your plan
//!   to continue"` and ends the turn normally. There is no reset time. The
//!   same text appears when the chosen model isn't on the user's plan.
//!
//! The structured markers (`errorKind`, `codexErrorInfo`) are preferred; the
//! text decides between a usage limit and a short-lived rate limit, and
//! carries the reset time. A reply counts as a limit notice only when it is
//! short and starts with one of the known phrases.

use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan, Zoned};
use serde_json::Value;

/// A usage limit the agent hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitInfo {
    /// The agent's message, for the user.
    pub message: String,
    /// When the limit resets, if the agent said.
    pub resets_at: Option<Timestamp>,
    /// The JSON-RPC error code, if the limit came as an error.
    pub code: Option<i32>,
    /// Where the limit was found.
    pub source: LimitSource,
}

/// Where a [`LimitInfo`] was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitSource {
    /// A JSON-RPC error answering `session/prompt`.
    Error,
    /// The text of the agent's reply: Cursor, or an agent message streamed
    /// before the turn ended.
    Reply,
}

impl std::fmt::Display for LimitInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "usage limit reached: {}", self.message)?;
        if let Some(resets_at) = self.resets_at {
            write!(f, " (resets at {resets_at})")?;
        }
        Ok(())
    }
}

/// A rate-limit status the agent reported outside an error. Claude Code
/// attaches one to some `usage_update`s (`_meta["_claude/rateLimit"]`); the
/// adapter drops it at the start of a turn, so it is a hint, not a guarantee.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitStatus {
    /// `allowed`, `allowed_warning` or `rejected`.
    pub status: String,
    /// When the window resets.
    pub resets_at: Option<Timestamp>,
    /// Which window: `five_hour`, `seven_day`, `seven_day_opus`, …
    pub window: Option<String>,
    /// How much of the window is used, from 0 to 1.
    pub utilization: Option<f64>,
}

/// Checks an error the agent answered a request with.
pub fn detect_in_error(code: i32, message: &str, data: Option<&Value>) -> Option<LimitInfo> {
    detect_in_error_at(code, message, data, &Zoned::now())
}

/// [`detect_in_error`], with "now" (and the local time zone) given.
pub fn detect_in_error_at(
    code: i32,
    message: &str,
    data: Option<&Value>,
    now: &Zoned,
) -> Option<LimitInfo> {
    let text = message
        .strip_prefix("Internal error: ")
        .or_else(|| message.strip_prefix("Internal error"))
        .unwrap_or(message)
        .trim();
    let data_message = data
        .and_then(|data| data.get("message"))
        .and_then(Value::as_str)
        .map(str::trim);
    let marker = |key: &str| data.and_then(|data| data.get(key)).and_then(Value::as_str);
    let limit = |message: &str| {
        Some(LimitInfo {
            message: message.to_owned(),
            resets_at: reset_from_text(message, now),
            code: Some(code),
            source: LimitSource::Error,
        })
    };

    // Codex: a structured marker, the text in data.message.
    if marker("codexErrorInfo") == Some("usageLimitExceeded") {
        return limit(data_message.filter(|m| !m.is_empty()).unwrap_or(text));
    }
    // Claude Code: a structured marker, but it also covers short-lived 429s.
    match marker("errorKind") {
        Some("rate_limit") => return is_notice(text).then(|| limit(text)).flatten(),
        Some("billing_error" | "account_on_hold") => return limit(text),
        _ => {}
    }
    // No marker (an older adapter, or another agent): the text alone.
    [Some(text), data_message]
        .into_iter()
        .flatten()
        .find(|text| is_notice(text))
        .and_then(limit)
}

/// Checks one agent message (the text of consecutive message chunks) for a
/// limit notice.
pub fn detect_in_reply(message: &str) -> Option<LimitInfo> {
    detect_in_reply_at(message, &Zoned::now())
}

/// [`detect_in_reply`], with "now" (and the local time zone) given.
pub fn detect_in_reply_at(message: &str, now: &Zoned) -> Option<LimitInfo> {
    let message = message.trim();
    is_notice(message).then(|| LimitInfo {
        message: message.to_owned(),
        resets_at: reset_from_text(message, now),
        code: None,
        source: LimitSource::Reply,
    })
}

/// Reads Claude Code's `_meta["_claude/rateLimit"]` from a raw `usage_update`.
pub fn rate_limit_status(update: &Value) -> Option<RateLimitStatus> {
    let info = update.get("_meta")?.get("_claude/rateLimit")?;
    Some(RateLimitStatus {
        status: info.get("status")?.as_str()?.to_owned(),
        resets_at: info
            .get("resetsAt")
            .and_then(Value::as_i64)
            .and_then(|seconds| Timestamp::from_second(seconds).ok()),
        window: info
            .get("rateLimitType")
            .and_then(Value::as_str)
            .map(str::to_owned),
        utilization: info.get("utilization").and_then(Value::as_f64),
    })
}

/// The longest text that can be a limit notice. Notices are one or two
/// sentences; a long reply that happens to start the same way is prose.
const MAX_NOTICE_LEN: usize = 600;

/// Phrases that start a limit notice, with an apostrophe-normalised text.
/// Claude Code's list is its SDK's `USAGE_LIMIT_ERROR_PREFIXES`; "You've hit
/// your" and "You've reached your" also need "limit" (or "budget") in the
/// first clause, since they're ordinary English.
const NOTICE_PREFIXES: &[&str] = &[
    // Claude Code
    "You're out of usage credits",
    "Your org is out of usage",
    "Your seat type doesn't include",
    "Your usage allocation has been disabled",
    "Your group's usage limit is set to $0",
    "Fable 5 requires usage credits",
    "You're out of extra usage",
    "Claude AI usage limit reached",
    // Codex
    "Quota exceeded. Check your plan and billing details",
    "Your workspace is out of credits",
    "You hit your spend cap",
    "To use Codex with your ChatGPT plan, upgrade to Plus",
];

/// What Cursor sends, and nothing else.
const CURSOR_NOTICES: &[&str] = &[
    "Upgrade your plan to continue",
    "Add a payment method to continue",
];

/// Whether `text` is a limit notice.
fn is_notice(text: &str) -> bool {
    let text = normalise(text.trim());
    if text.is_empty() || text.len() > MAX_NOTICE_LEN {
        return false;
    }
    if CURSOR_NOTICES.contains(&text.as_str()) {
        return true;
    }
    if NOTICE_PREFIXES
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return true;
    }
    ["You've hit your ", "You've reached your "]
        .iter()
        .any(|prefix| {
            text.strip_prefix(prefix).is_some_and(|rest| {
                let clause = rest.split([' ', '.', '\n']).take(6).collect::<Vec<_>>();
                clause
                    .iter()
                    .any(|word| word.starts_with("limit") || word.starts_with("budget"))
            })
        })
}

/// Straight apostrophes and no `\r`, so one set of phrases matches both
/// Claude Code (`'`) and Codex (`’`).
fn normalise(text: &str) -> String {
    text.replace('\u{2019}', "'").replace('\r', "")
}

/// The reset time in a limit notice, in any of the agents' formats.
fn reset_from_text(text: &str, now: &Zoned) -> Option<Timestamp> {
    let text = normalise(text);
    legacy_epoch(&text)
        .or_else(|| claude_reset(&text, now))
        .or_else(|| codex_reset(&text, now))
}

/// The 2025 format: `Claude AI usage limit reached|1751911200`.
fn legacy_epoch(text: &str) -> Option<Timestamp> {
    let (_, tail) = text.trim().rsplit_once('|')?;
    Timestamp::from_second(tail.trim().parse().ok()?).ok()
}

/// Claude Code: `… · resets 3pm (Europe/Bucharest)`, `resets 2:50pm`,
/// `resets Sep 14 at 8pm (America/Chicago)`, `resets Jul 31, 2am (UTC)`.
/// Without a date, the next time it's that time; without a zone, local time.
fn claude_reset(text: &str, now: &Zoned) -> Option<Timestamp> {
    let (_, rest) = text.split_once("resets ")?;
    // The clause ends at the next " · " separator.
    let clause = rest.split(" \u{b7} ").next()?.trim().trim_end_matches('.');
    let (body, zone) = match (clause.find(" ("), clause.find(')')) {
        (Some(open), Some(close)) if close > open => {
            (&clause[..open], Some(&clause[open + 2..close]))
        }
        _ => (clause, None),
    };
    let tz = match zone {
        Some(name) => TimeZone::get(name).ok()?,
        None => now.time_zone().clone(),
    };
    let body = body.trim();
    let (date_part, time_part) = match body.rsplit_once(' ') {
        Some((date, time)) => (
            date.trim().trim_end_matches(',').trim_end_matches(" at"),
            time,
        ),
        None => ("", body),
    };
    let time = clock_time(time_part, None)?;
    let now = now.with_time_zone(tz.clone());
    if date_part.trim().is_empty() {
        let today = now.date().to_datetime(time).to_zoned(tz).ok()?;
        let next = if today.timestamp() <= now.timestamp() {
            today.checked_add(1.day()).ok()?
        } else {
            today
        };
        return Some(next.timestamp());
    }
    let (month, day, year) = month_day_year(date_part)?;
    let at = |year: i16| {
        Date::new(year, month, day)
            .ok()?
            .to_datetime(time)
            .to_zoned(tz.clone())
            .ok()
    };
    match year {
        Some(year) => Some(at(year)?.timestamp()),
        None => {
            let this_year = at(now.year())?;
            // "Jul 31" seen in November means next July.
            // (A timestamp only takes hours: days depend on the time zone.)
            if this_year.timestamp() < now.timestamp().checked_sub(24.hours()).ok()? {
                Some(at(now.year() + 1)?.timestamp())
            } else {
                Some(this_year.timestamp())
            }
        }
    }
}

/// Codex: `try again at 3:05 PM.` (today) or `try again at Nov 12th, 2026
/// 3:05 PM.`, in the agent host's local time.
fn codex_reset(text: &str, now: &Zoned) -> Option<Timestamp> {
    let lower = text.to_ascii_lowercase();
    let start = lower.find("try again at ")? + "try again at ".len();
    let rest = text[start..].trim().trim_end_matches('.');
    let words: Vec<&str> = rest.split_whitespace().collect();
    let am_pm = words.iter().position(|word| {
        let word = word.trim_end_matches('.');
        word.eq_ignore_ascii_case("am") || word.eq_ignore_ascii_case("pm")
    })?;
    if am_pm == 0 {
        return None;
    }
    let time = clock_time(words[am_pm - 1], Some(words[am_pm].trim_end_matches('.')))?;
    let tz = now.time_zone().clone();
    let date = if am_pm >= 2 {
        let (month, day, year) = month_day_year(&words[..am_pm - 1].join(" "))?;
        Date::new(year.unwrap_or(now.year()), month, day).ok()?
    } else {
        now.date()
    };
    Some(date.to_datetime(time).to_zoned(tz).ok()?.timestamp())
}

/// `3pm`, `2:50pm`, or `3:05` with `am_pm` given separately.
fn clock_time(text: &str, am_pm: Option<&str>) -> Option<Time> {
    let text = text.trim().to_ascii_lowercase();
    let (digits, suffix) = match am_pm {
        Some(suffix) => (text.as_str(), suffix.to_ascii_lowercase()),
        None => {
            let split = text.len().checked_sub(2)?;
            (&text[..split], text[split..].to_owned())
        }
    };
    let pm = match suffix.as_str() {
        "am" => false,
        "pm" => true,
        _ => return None,
    };
    let (hour, minute) = match digits.split_once(':') {
        Some((hour, minute)) => (hour.parse::<i8>().ok()?, minute.parse::<i8>().ok()?),
        None => (digits.parse::<i8>().ok()?, 0),
    };
    if !(1..=12).contains(&hour) {
        return None;
    }
    let hour = match (hour, pm) {
        (12, false) => 0,
        (12, true) => 12,
        (hour, false) => hour,
        (hour, true) => hour + 12,
    };
    Time::new(hour, minute, 0, 0).ok()
}

/// `Sep 14`, `Jul 31`, `Sep 15, 2027`, `Nov 12th, 2026`.
fn month_day_year(text: &str) -> Option<(i8, i8, Option<i16>)> {
    let mut words = text.split([' ', ',']).filter(|word| !word.is_empty());
    let month = words.next()?.get(..3)?.to_ascii_lowercase();
    let month = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|name| *name == month)?;
    let day = words
        .next()?
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .parse()
        .ok()?;
    let year = match words.next() {
        Some(year) => Some(year.parse().ok()?),
        None => None,
    };
    Some((i8::try_from(month + 1).ok()?, day, year))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 2026-11-09 12:00 in Bucharest (10:00 UTC).
    fn now() -> Zoned {
        "2026-11-09T12:00:00+02:00[Europe/Bucharest]"
            .parse()
            .unwrap()
    }

    fn at(text: &str) -> Timestamp {
        text.parse::<Zoned>().unwrap().timestamp()
    }

    fn claude_error(text: &str) -> Option<LimitInfo> {
        detect_in_error_at(
            -32603,
            &format!("Internal error: {text}"),
            Some(&json!({"errorKind": "rate_limit"})),
            &now(),
        )
    }

    #[test]
    fn claude_usage_limits_with_their_reset_times() {
        let cases = [
            (
                "You've hit your 5-hour limit · resets 3pm (Europe/Bucharest)",
                Some("2026-11-09T15:00:00+02:00[Europe/Bucharest]"),
            ),
            (
                "You've hit your limit · resets 8pm",
                Some("2026-11-09T20:00:00+02:00[Europe/Bucharest]"),
            ),
            (
                "You've hit your session limit · resets 11:20pm (Europe/Budapest)",
                Some("2026-11-09T23:20:00+01:00[Europe/Budapest]"),
            ),
            // 11am has passed in Bucharest: tomorrow.
            (
                "You've hit your limit · resets 11am (Europe/Bucharest)",
                Some("2026-11-10T11:00:00+02:00[Europe/Bucharest]"),
            ),
            (
                "You've hit your weekly limit · resets Sep 14 at 8pm (America/Chicago)",
                Some("2027-09-14T20:00:00-05:00[America/Chicago]"),
            ),
            (
                "You've hit your weekly limit · resets Nov 12, 2am (UTC)",
                Some("2026-11-12T02:00:00+00:00[UTC]"),
            ),
            (
                "You've hit your weekly limit · resets Jul 31, 2am (UTC)",
                Some("2027-07-31T02:00:00+00:00[UTC]"),
            ),
            (
                "You've hit your session limit · resets 12:30pm",
                Some("2026-11-09T12:30:00+02:00[Europe/Bucharest]"),
            ),
            (
                "You've hit your limit · resets 5pm · run /login to switch to an API usage-billed account.",
                Some("2026-11-09T17:00:00+02:00[Europe/Bucharest]"),
            ),
            (
                "You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit",
                None,
            ),
            ("You're out of usage credits", None),
            ("You've reached your account usage limit", None),
        ];
        for (text, reset) in cases {
            let info = claude_error(text).unwrap_or_else(|| panic!("not a limit: {text}"));
            assert_eq!(info.message, text);
            assert_eq!(info.code, Some(-32603));
            assert_eq!(info.source, LimitSource::Error);
            assert_eq!(info.resets_at, reset.map(at), "reset of {text}");
        }
    }

    #[test]
    fn claude_short_lived_rate_limits_are_not_usage_limits() {
        assert_eq!(
            claude_error(
                "API Error: Server is temporarily limiting requests (not your usage limit) · Rate limited"
            ),
            None
        );
        assert_eq!(
            claude_error("API Error: Request rejected (429) · slow down"),
            None
        );
    }

    #[test]
    fn claude_billing_errors_are_limits() {
        let info = detect_in_error_at(
            -32603,
            "Internal error: Your credit balance is too low",
            Some(&json!({"errorKind": "billing_error"})),
            &now(),
        )
        .unwrap();
        assert_eq!(info.message, "Your credit balance is too low");
    }

    #[test]
    fn the_legacy_claude_format_has_an_epoch() {
        let info = detect_in_reply_at("Claude AI usage limit reached|1751911200", &now()).unwrap();
        assert_eq!(
            info.resets_at,
            Some(Timestamp::from_second(1_751_911_200).unwrap())
        );
    }

    fn codex_error(text: &str) -> Option<LimitInfo> {
        detect_in_error_at(
            -32603,
            "Internal error",
            Some(&json!({"message": text, "codexErrorInfo": "usageLimitExceeded"})),
            &now(),
        )
    }

    #[test]
    fn codex_usage_limits_with_their_reset_times() {
        let cases = [
            (
                "You\u{2019}ve hit your usage limit. Try again at 3:05 PM.",
                Some("2026-11-09T15:05:00+02:00[Europe/Bucharest]"),
            ),
            (
                "You\u{2019}ve hit your usage limit. Visit https://chatgpt.com/settings/usage to purchase more credits or try again at Nov 12th, 2026 3:05 PM.",
                Some("2026-11-12T15:05:00+02:00[Europe/Bucharest]"),
            ),
            (
                "You\u{2019}ve hit your usage limit. Upgrade to Plus to continue using Codex (https://chatgpt.com/explore/plus), or try again later.",
                None,
            ),
            (
                "You\u{2019}ve hit your usage limit for codex_other. Switch to another model now, or try again at 12:01 AM.",
                Some("2026-11-09T00:01:00+02:00[Europe/Bucharest]"),
            ),
            ("Quota exceeded. Check your plan and billing details.", None),
        ];
        for (text, reset) in cases {
            let info = codex_error(text).unwrap_or_else(|| panic!("not a limit: {text}"));
            assert_eq!(info.message, text);
            assert_eq!(info.resets_at, reset.map(at), "reset of {text}");
        }
    }

    #[test]
    fn codex_retrying_rate_limits_are_not_usage_limits() {
        let error = detect_in_error_at(
            -32603,
            "Internal error",
            Some(
                &json!({"message": "rate limit exceeded: slow down", "codexErrorInfo": "rateLimitExceeded"}),
            ),
            &now(),
        );
        assert_eq!(error, None);
    }

    #[test]
    fn cursor_notices_are_exact_replies() {
        let info = detect_in_reply_at("\n\nUpgrade your plan to continue", &now()).unwrap();
        assert_eq!(info.message, "Upgrade your plan to continue");
        assert_eq!(info.resets_at, None);
        assert_eq!(info.source, LimitSource::Reply);
        assert!(detect_in_reply_at("\n\nAdd a payment method to continue", &now()).is_some());
        assert!(
            detect_in_reply_at(
                "Upgrade your plan to continue using the API, then retry.",
                &now()
            )
            .is_none()
        );
    }

    #[test]
    fn notices_in_replies_are_found_without_an_error() {
        let info = detect_in_reply_at(
            "You\u{2019}ve hit your usage limit. Try again at 3:05 PM.\n\n",
            &now(),
        )
        .unwrap();
        assert_eq!(
            info.resets_at,
            Some(at("2026-11-09T15:05:00+02:00[Europe/Bucharest]"))
        );
    }

    #[test]
    fn ordinary_replies_and_errors_are_not_limits() {
        for reply in [
            "OK",
            "You've hit your target: all tests pass.",
            "You've reached your destination folder.",
            "Here's how the rate limit works in this code.",
        ] {
            assert_eq!(detect_in_reply_at(reply, &now()), None, "{reply}");
        }
        let long = format!(
            "You've hit your limit · resets 3pm. {}",
            "x".repeat(MAX_NOTICE_LEN)
        );
        assert_eq!(detect_in_reply_at(&long, &now()), None);
        assert_eq!(
            detect_in_error_at(-32603, "Internal error", None, &now()),
            None
        );
        assert_eq!(
            detect_in_error_at(
                -32603,
                "Internal error: boom",
                Some(&json!({"errorKind": "server_error"})),
                &now()
            ),
            None
        );
    }

    #[test]
    fn a_notice_without_a_marker_is_still_a_limit() {
        let info = detect_in_error_at(
            -32603,
            "Internal error: You've hit your weekly limit · resets Nov 12, 2am (UTC)",
            None,
            &now(),
        )
        .unwrap();
        assert_eq!(info.resets_at, Some(at("2026-11-12T02:00:00+00:00[UTC]")));
    }

    #[test]
    fn claude_rate_limit_status_is_read_from_usage_meta() {
        let status = rate_limit_status(&json!({
            "sessionUpdate": "usage_update", "size": 200000, "used": 2,
            "_meta": {"_claude/rateLimit": {
                "resetsAt": 1700000000, "status": "allowed_warning",
                "utilization": 0.9, "rateLimitType": "five_hour"
            }}
        }))
        .unwrap();
        assert_eq!(status.status, "allowed_warning");
        assert_eq!(
            status.resets_at,
            Some(Timestamp::from_second(1_700_000_000).unwrap())
        );
        assert_eq!(status.window.as_deref(), Some("five_hour"));
        assert_eq!(status.utilization, Some(0.9));
        assert!(rate_limit_status(&json!({"sessionUpdate": "usage_update"})).is_none());
    }
}
