//! Usage: subscription allowance (`usage/read`, `usage/changed`) and token counts.
//!
//! **No allowance.** On an account without a subscription allowance (`muse usage` prints
//! "No subscription allowance (no_allowance)"), Muse 1.4.14 has nothing to report:
//! `usage/read` answers `{}`, with no `usage` member, and no `usage/changed` arrives, even after
//! `meta` turns. Those turns still run and complete normally. So a missing `usage` means
//! "nothing observed", not "no allowance" and not "limited". A real limit stops a turn with a
//! [`crate::TurnError`] whose `quota` is set ([`crate::FailureClass::Limit`]).

use serde::{Deserialize, Serialize};

use super::TokenUsage;

/// The current usage window, typically five hours.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUsageWindow {
    /// When the window resets, in epoch milliseconds.
    pub resets_at_ms: u64,
    /// Percent of the window used. Can exceed 100.
    pub used_percent: u64,
    /// The window's length in minutes.
    pub window_duration_mins: u64,
}

/// The rolling weekly budget.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUsageWeekly {
    /// When the week resets, in epoch milliseconds.
    pub resets_at_ms: u64,
    /// Percent of the week used. Can exceed 100.
    pub used_percent: u64,
}

/// The last subscription usage Muse observed: `usage/read`'s `usage`, and `usage/changed`'s
/// params. A point-in-time reading as of `observedAtMs`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionUsage {
    /// When Muse received it, in epoch milliseconds.
    pub observed_at_ms: u64,
    /// The subscription tier, verbatim.
    pub tier: String,
    /// The weekly budget.
    pub weekly: SubscriptionUsageWeekly,
    /// The current window.
    pub window: SubscriptionUsageWindow,
}

impl SubscriptionUsage {
    /// If the window or the week is used up, when the later of the exhausted ones resets.
    pub fn exhausted_until_ms(&self) -> Option<u64> {
        let window = (self.window.used_percent >= 100).then_some(self.window.resets_at_ms);
        let weekly = (self.weekly.used_percent >= 100).then_some(self.weekly.resets_at_ms);
        window.max(weekly)
    }
}

/// `usage/read` result. `usage` is absent when Muse has observed nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct UsageReadResult {
    /// The last observation, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SubscriptionUsage>,
}

/// A session's running token totals.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CumulativeTokenUsage {
    /// Cache-read tokens, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Cache-write tokens, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// Output tokens.
    pub output_tokens: u64,
    /// Prompt tokens, counted once.
    pub prompt_tokens: u64,
    /// Prompt plus output.
    pub total_tokens: u64,
}

/// `session/tokenUsage`: one model call finished. Raw counters plus session totals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionTokenUsageParams {
    /// The session's totals so far.
    pub cumulative: CumulativeTokenUsage,
    /// How long the call took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// The provider's finish reason, such as `tool_calls`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// The model; `None` for `echo`.
    #[serde(default)]
    pub model_id: Option<String>,
    /// Prompt tokens, counted once.
    pub prompt_tokens: u64,
    /// The session.
    pub session_id: String,
    /// Prompt plus output.
    pub total_tokens: u64,
    /// The turn.
    pub turn_id: String,
    /// The call's raw counters.
    pub usage: TokenUsage,
    /// The view position.
    pub view_cursor: String,
}

open_enum! {
    /// How full the context window is.
    pub enum ContextPressure {
        /// Fine.
        Normal = "normal",
        /// Getting full; compaction is near.
        Warning = "warning",
        /// Full.
        Blocked = "blocked",
    }
}

/// `session/contextUsage`: context occupancy changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionContextUsageParams {
    /// How full.
    pub pressure: ContextPressure,
    /// The session.
    pub session_id: String,
    /// Tokens in the context.
    pub used_tokens: u64,
    /// The view position.
    pub view_cursor: String,
    /// The context window's size, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(window: u64, weekly: u64) -> SubscriptionUsage {
        SubscriptionUsage {
            observed_at_ms: 0,
            tier: "pro".into(),
            weekly: SubscriptionUsageWeekly {
                resets_at_ms: 2_000,
                used_percent: weekly,
            },
            window: SubscriptionUsageWindow {
                resets_at_ms: 1_000,
                used_percent: window,
                window_duration_mins: 300,
            },
        }
    }

    #[test]
    fn exhaustion_waits_for_the_later_reset() {
        assert_eq!(usage(40, 90).exhausted_until_ms(), None);
        assert_eq!(usage(100, 90).exhausted_until_ms(), Some(1_000));
        assert_eq!(usage(120, 100).exhausted_until_ms(), Some(2_000));
        assert_eq!(usage(10, 101).exhausted_until_ms(), Some(2_000));
    }
}
