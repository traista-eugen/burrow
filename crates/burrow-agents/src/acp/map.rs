//! Converting `burrow-acp`'s types to Burrow's session payloads. Every function is total: what
//! Burrow can't show is dropped or described in text, never an error.

use burrow_acp as acp;
use burrow_protocol::{
    AgentMode, ContentBlock, ModelInfo, PermissionOption, PermissionOptionKind, Plan, PlanEntry,
    PlanEntryPriority, PlanEntryStatus, ProviderUsage, Timestamp, ToolCall, ToolCallContent,
    ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolKind, TurnEndReason, UsageSource,
};

/// A tool call's kind. ACP's `switch_mode` becomes `switchMode`, like every ACP value.
pub fn tool_kind(kind: acp::ToolKind) -> ToolKind {
    match kind {
        acp::ToolKind::Read => ToolKind::Read,
        acp::ToolKind::Edit => ToolKind::Edit,
        acp::ToolKind::Delete => ToolKind::Delete,
        acp::ToolKind::Move => ToolKind::Move,
        acp::ToolKind::Search => ToolKind::Search,
        acp::ToolKind::Execute => ToolKind::Execute,
        acp::ToolKind::Think => ToolKind::Think,
        acp::ToolKind::Fetch => ToolKind::Fetch,
        acp::ToolKind::SwitchMode => ToolKind::SwitchMode,
        acp::ToolKind::Other => ToolKind::Misc,
    }
}

/// A tool call's status; `in_progress` becomes `inProgress`.
pub fn tool_status(status: acp::ToolCallStatus) -> ToolCallStatus {
    match status {
        acp::ToolCallStatus::Pending => ToolCallStatus::Pending,
        acp::ToolCallStatus::InProgress => ToolCallStatus::InProgress,
        acp::ToolCallStatus::Completed => ToolCallStatus::Completed,
        acp::ToolCallStatus::Failed => ToolCallStatus::Failed,
    }
}

/// Content the agent sent, as a Burrow content block. Images have no artifact to point at yet
/// (the artifact store comes later), so they are described in text. Audio and unknown kinds
/// are dropped.
pub fn content_block(content: acp::Content) -> Option<ContentBlock> {
    match content {
        acp::Content::Text(text) => Some(ContentBlock::text(text)),
        acp::Content::Image { mime_type, uri, .. } => Some(ContentBlock::text(match uri {
            Some(uri) => format!("[image: {mime_type}, {uri}]"),
            None => format!("[image: {mime_type}]"),
        })),
        acp::Content::ResourceLink { uri, .. } | acp::Content::Resource { uri, .. } => {
            Some(ContentBlock::Resource { uri })
        }
        acp::Content::Other(_) => None,
    }
}

/// Tool call output: text, a diff or a terminal. Unknown kinds are dropped.
pub fn tool_content(content: acp::ToolCallContent) -> Option<ToolCallContent> {
    match content {
        acp::ToolCallContent::Content(content) => {
            content_block(content).map(|content| ToolCallContent::Content { content })
        }
        acp::ToolCallContent::Diff {
            path,
            old_text,
            new_text,
        } => Some(ToolCallContent::Diff {
            path: path.display().to_string(),
            old_text,
            new_text,
        }),
        acp::ToolCallContent::Terminal { terminal_id } => {
            Some(ToolCallContent::Terminal { terminal_id })
        }
        acp::ToolCallContent::Other(_) => None,
    }
}

fn tool_contents(content: Vec<acp::ToolCallContent>) -> Vec<ToolCallContent> {
    content.into_iter().filter_map(tool_content).collect()
}

fn location(location: acp::Location) -> ToolCallLocation {
    ToolCallLocation {
        path: location.path.display().to_string(),
        line: location.line,
    }
}

/// A new tool call. Its `turnId` is left for the session manager.
pub fn tool_call(call: acp::ToolCall) -> ToolCall {
    ToolCall {
        turn_id: None,
        tool_call_id: call.id,
        title: call.title,
        kind: tool_kind(call.kind),
        status: tool_status(call.status),
        locations: call.locations.into_iter().map(location).collect(),
        content: tool_contents(call.content),
        raw_input: call.raw_input,
    }
}

/// A change to a tool call. Raw output has no place in Burrow's events and is dropped.
pub fn tool_call_update(update: acp::ToolCallUpdate) -> ToolCallUpdate {
    ToolCallUpdate {
        tool_call_id: update.id,
        status: update.status.map(tool_status),
        title: update.title,
        kind: update.kind.map(tool_kind),
        locations: update
            .locations
            .map(|locations| locations.into_iter().map(location).collect()),
        content: update.content.map(tool_contents),
    }
}

/// A tool call made from the update a permission request carries, for agents that ask about a
/// tool call they haven't announced. Missing fields get neutral defaults.
pub fn tool_call_from_update(update: acp::ToolCallUpdate, title: String) -> ToolCall {
    ToolCall {
        turn_id: None,
        tool_call_id: update.id,
        title,
        kind: update.kind.map_or(ToolKind::Misc, tool_kind),
        status: update.status.map_or(ToolCallStatus::Pending, tool_status),
        locations: update
            .locations
            .unwrap_or_default()
            .into_iter()
            .map(location)
            .collect(),
        content: update.content.map(tool_contents).unwrap_or_default(),
        raw_input: update.raw_input,
    }
}

/// The agent's plan. ACP's snake_case values become camelCase; unknown ones pass through.
pub fn plan(entries: Vec<acp::PlanEntry>) -> Plan {
    Plan {
        entries: entries
            .into_iter()
            .map(|entry| PlanEntry {
                content: entry.content,
                status: match entry.status.as_str() {
                    "in_progress" => PlanEntryStatus::InProgress,
                    other => PlanEntryStatus::from(other),
                },
                priority: PlanEntryPriority::from(entry.priority.as_str()),
            })
            .collect(),
    }
}

/// One answer to a permission request.
pub fn permission_option(option: acp::PermissionOption) -> PermissionOption {
    PermissionOption {
        option_id: option.id,
        name: option.name,
        kind: match option.kind {
            acp::PermissionKind::AllowOnce => PermissionOptionKind::AllowOnce,
            acp::PermissionKind::AllowAlways => PermissionOptionKind::AllowAlways,
            acp::PermissionKind::RejectOnce => PermissionOptionKind::RejectOnce,
            acp::PermissionKind::RejectAlways => PermissionOptionKind::RejectAlways,
            acp::PermissionKind::Other(kind) => PermissionOptionKind::Other(kind),
        },
    }
}

/// How a turn ended: `end_turn` is `completed`, `cancelled` is `cancelled`, a usage limit is
/// `limit`, and everything else is `error`, with the reason in the message.
pub fn turn_end(end: acp::TurnEnd) -> (TurnEndReason, Option<String>) {
    match end {
        acp::TurnEnd::Stopped { reason, .. } => match reason {
            acp::StopReason::EndTurn => (TurnEndReason::Completed, None),
            acp::StopReason::Cancelled => (TurnEndReason::Cancelled, None),
            acp::StopReason::MaxTokens => (
                TurnEndReason::Error,
                Some("the agent stopped: it reached its token limit (max_tokens)".into()),
            ),
            acp::StopReason::MaxTurnRequests => (
                TurnEndReason::Error,
                Some(
                    "the agent stopped: it made too many model requests in one turn \
                     (max_turn_requests)"
                        .into(),
                ),
            ),
            acp::StopReason::Refusal => (
                TurnEndReason::Error,
                Some("the model refused to continue (refusal)".into()),
            ),
            acp::StopReason::Other(reason) if reason.is_empty() => (
                TurnEndReason::Error,
                Some("the agent ended the turn without a stop reason".into()),
            ),
            acp::StopReason::Other(reason) => (
                TurnEndReason::Error,
                Some(format!("the agent stopped: {reason}")),
            ),
        },
        acp::TurnEnd::Limit(info) => (TurnEndReason::Limit, Some(info.message)),
        acp::TurnEnd::Error { message, .. } => (TurnEndReason::Error, Some(message)),
    }
}

/// A `jiff` instant as a protocol timestamp; `None` if it is out of range.
pub fn timestamp(instant: jiff::Timestamp) -> Option<Timestamp> {
    Timestamp::from_jiff(instant).ok()
}

/// A mode the agent offers.
pub fn agent_mode(choice: acp::Choice) -> AgentMode {
    AgentMode {
        id: choice.id,
        name: (!choice.name.is_empty()).then_some(choice.name),
        description: choice.description,
    }
}

/// A model the agent offers.
pub fn model_info(choice: acp::Choice) -> ModelInfo {
    ModelInfo {
        id: choice.id,
        name: (!choice.name.is_empty()).then_some(choice.name),
    }
}

/// A rate-limit status Claude Code reported on the side, as an allowance for `usage/get`.
pub fn provider_usage(provider: &str, status: acp::RateLimitStatus) -> ProviderUsage {
    let window = status.window.as_deref().map(|window| match window {
        "five_hour" => "5-hour",
        "seven_day" => "weekly",
        "seven_day_opus" => "weekly Opus",
        "seven_day_sonnet" => "weekly Sonnet",
        other => other,
    });
    let message = match (window, status.status.as_str()) {
        (Some(window), "rejected") => format!("{window} limit reached"),
        (Some(window), "allowed_warning") => format!("{window} limit nearly reached"),
        (Some(window), _) => format!("{window} limit"),
        (None, status) => format!("rate limit: {status}"),
    };
    ProviderUsage {
        provider: provider.to_owned(),
        agent: Some(provider.into()),
        used: status.utilization.map(|used| used * 100.0),
        limit: status.utilization.map(|_| 100.0),
        unit: status.utilization.map(|_| "percent".to_owned()),
        limited: status.status == "rejected",
        resets_at: status.resets_at.and_then(timestamp),
        message: Some(message),
        source: UsageSource::Reported,
        updated_at: Timestamp::now(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn tool_calls_keep_their_content_in_camel_case() {
        let call = tool_call(acp::ToolCall {
            id: "t1".into(),
            title: "Edit README.md".into(),
            kind: acp::ToolKind::SwitchMode,
            status: acp::ToolCallStatus::InProgress,
            content: vec![
                acp::ToolCallContent::Content(acp::Content::Text("done".into())),
                acp::ToolCallContent::Diff {
                    path: PathBuf::from(r"C:\repo\README.md"),
                    old_text: None,
                    new_text: "hi".into(),
                },
                acp::ToolCallContent::Other(serde_json::json!({"type": "hologram"})),
            ],
            locations: vec![acp::Location {
                path: PathBuf::from(r"C:\repo\README.md"),
                line: Some(3),
            }],
            raw_input: Some(serde_json::json!({"path": "README.md"})),
            raw_output: None,
        });
        let json = serde_json::to_value(&call).unwrap();
        assert_eq!(json["kind"], "switchMode");
        assert_eq!(json["status"], "inProgress");
        assert_eq!(call.content.len(), 2, "the unknown content is dropped");
        assert_eq!(json["content"][1]["type"], "diff");
        assert_eq!(json["locations"][0]["line"], 3);
        assert_eq!(json["rawInput"]["path"], "README.md");
    }

    #[test]
    fn plans_and_options_become_camel_case() {
        let plan = plan(vec![acp::PlanEntry {
            content: "Read".into(),
            priority: "high".into(),
            status: "in_progress".into(),
        }]);
        assert_eq!(plan.entries[0].status, PlanEntryStatus::InProgress);
        assert_eq!(plan.entries[0].priority, PlanEntryPriority::High);
        let option = permission_option(acp::PermissionOption {
            id: "allow".into(),
            name: "Allow once".into(),
            kind: acp::PermissionKind::AllowOnce,
        });
        assert_eq!(option.kind, PermissionOptionKind::AllowOnce);
    }

    #[test]
    fn stop_reasons_map_to_turn_end_reasons() {
        let stopped = |reason| acp::TurnEnd::Stopped {
            reason,
            usage: None,
        };
        assert_eq!(
            turn_end(stopped(acp::StopReason::EndTurn)),
            (TurnEndReason::Completed, None)
        );
        assert_eq!(
            turn_end(stopped(acp::StopReason::Cancelled)),
            (TurnEndReason::Cancelled, None)
        );
        let (reason, message) = turn_end(stopped(acp::StopReason::MaxTokens));
        assert_eq!(reason, TurnEndReason::Error);
        assert!(message.unwrap().contains("max_tokens"));
        let (reason, message) = turn_end(acp::TurnEnd::Error {
            code: Some(-32603),
            message: "boom".into(),
        });
        assert_eq!(
            (reason, message.as_deref()),
            (TurnEndReason::Error, Some("boom"))
        );
    }

    #[test]
    fn claude_rate_limit_status_becomes_provider_usage() {
        let usage = provider_usage(
            "claude",
            acp::RateLimitStatus {
                status: "rejected".into(),
                resets_at: Some(jiff::Timestamp::from_second(1_800_000_000).unwrap()),
                window: Some("five_hour".into()),
                utilization: Some(1.0),
            },
        );
        assert!(usage.limited);
        assert_eq!(usage.used, Some(100.0));
        assert_eq!(usage.unit.as_deref(), Some("percent"));
        assert_eq!(usage.message.as_deref(), Some("5-hour limit reached"));
        assert_eq!(usage.resets_at.unwrap().as_unix_millis(), 1_800_000_000_000);
    }
}
