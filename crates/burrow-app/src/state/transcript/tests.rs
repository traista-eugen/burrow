use burrow_protocol::{
    AgentMessageChunk, AgentModeChanged, AgentThoughtChunk, ContentBlock, LimitReached,
    ModelChanged, PermissionOptionKind, PermissionRequested, PermissionResolved, Plan,
    PlanEntryPriority, PlanEntryStatus, StateChanged, StreamName, ToolCall, ToolCallUpdate,
    TurnEnded, Usage, UserMessage,
};

use super::*;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_unix_millis(1_800_000_000_000 + ms).unwrap()
}

/// Events are a second apart, so a turn's duration is easy to read.
fn ev(seq: u64, event: impl Into<Event>) -> StreamEvent {
    StreamEvent {
        stream: StreamName::session("ses_1"),
        seq,
        at: at(seq as i64 * 1000),
        event: event.into(),
    }
}

fn prompt(turn: &str, text: &str) -> UserMessage {
    UserMessage {
        turn_id: turn.into(),
        content: vec![ContentBlock::text(text)],
    }
}

fn chunk(turn: &str, text: &str) -> AgentMessageChunk {
    AgentMessageChunk {
        turn_id: turn.into(),
        text: text.into(),
    }
}

fn thought(turn: &str, text: &str) -> AgentThoughtChunk {
    AgentThoughtChunk {
        turn_id: turn.into(),
        text: text.into(),
    }
}

fn tool(id: &str, kind: ToolKind, title: &str) -> ToolCall {
    ToolCall {
        turn_id: Some("t1".into()),
        tool_call_id: id.into(),
        title: title.into(),
        kind,
        status: ToolCallStatus::Pending,
        locations: Vec::new(),
        content: Vec::new(),
        raw_input: None,
    }
}

fn status(id: &str, status: ToolCallStatus) -> ToolCallUpdate {
    ToolCallUpdate {
        tool_call_id: id.into(),
        status: Some(status),
        title: None,
        kind: None,
        locations: None,
        content: None,
    }
}

fn ended(turn: &str, reason: TurnEndReason, message: Option<&str>) -> TurnEnded {
    TurnEnded {
        turn_id: turn.into(),
        reason,
        message: message.map(str::to_owned),
    }
}

fn state(state: SessionState, reason: Option<&str>) -> StateChanged {
    StateChanged {
        state,
        reason: reason.map(str::to_owned),
    }
}

fn request(id: &str) -> PermissionRequested {
    PermissionRequested {
        inbox_item_id: id.into(),
        turn_id: Some("t1".into()),
        tool_call_id: "c1".into(),
        title: "Run cleanup.ps1".into(),
        options: vec![PermissionOption {
            option_id: "allow".into(),
            name: "Allow once".into(),
            kind: PermissionOptionKind::AllowOnce,
        }],
    }
}

fn expired(id: &str) -> PermissionResolved {
    PermissionResolved {
        inbox_item_id: id.into(),
        tool_call_id: "c1".into(),
        status: InboxItemStatus::Expired,
        option_id: None,
        by: None,
    }
}

fn plan(statuses: &[PlanEntryStatus]) -> Plan {
    Plan {
        entries: statuses
            .iter()
            .enumerate()
            .map(|(i, status)| PlanEntry {
                content: format!("Step {i}"),
                status: status.clone(),
                priority: PlanEntryPriority::Medium,
            })
            .collect(),
    }
}

fn build(events: Vec<StreamEvent>) -> (TranscriptModel, Vec<Change>) {
    let mut model = TranscriptModel::new();
    let mut changes = Vec::new();
    model.apply_all(&events, &mut changes);
    (model, changes)
}

fn outcome(model: &TranscriptModel, row: usize) -> &PermissionOutcome {
    match &model.items()[row] {
        Item::Permission(item) => &item.outcome,
        other => panic!("row {row} is {other:?}"),
    }
}

#[test]
fn chunks_of_one_turn_join_and_anything_between_splits_them() {
    let (model, changes) = build(vec![
        ev(1, prompt("t1", "hello")),
        ev(2, thought("t1", "Let me ")),
        ev(3, thought("t1", "think.")),
        ev(4, chunk("t1", "Echo: ")),
        ev(5, chunk("t1", "hello")),
        ev(6, ended("t1", TurnEndReason::Completed, None)),
        ev(7, prompt("t2", "again")),
        ev(8, chunk("t2", "Second")),
    ]);
    let rows: Vec<_> = model.items().iter().map(std::mem::discriminant).collect();
    assert_eq!(rows.len(), 6, "{:?}", model.items());
    assert_eq!(
        model.items()[1],
        Item::Thought {
            turn_id: "t1".into(),
            text: "Let me think.".into()
        }
    );
    assert_eq!(
        model.items()[2],
        Item::Agent {
            turn_id: "t1".into(),
            text: "Echo: hello".into()
        }
    );
    assert!(
        matches!(&model.items()[5], Item::Agent { turn_id, text } if turn_id == "t2" && text == "Second")
    );
    // Growing a row reports an append, not a new row.
    assert_eq!(changes[2], Change::Appended(1));
    assert_eq!(changes[4], Change::Appended(2));
}

#[test]
fn tool_call_updates_change_their_row_in_place() {
    let diff = ToolCallContent::Diff {
        path: "D:\\repo\\notes.txt".into(),
        old_text: Some("a\n".into()),
        new_text: "a\nb\n".into(),
    };
    let (model, changes) = build(vec![
        ev(1, prompt("t1", "edit")),
        ev(2, chunk("t1", "Let me look.")),
        ev(3, tool("c1", ToolKind::Execute, "Terminal")),
        ev(
            4,
            ToolCallUpdate {
                tool_call_id: "c1".into(),
                status: Some(ToolCallStatus::InProgress),
                title: Some("echo hi".into()),
                kind: Some(ToolKind::Edit),
                locations: Some(vec![ToolCallLocation {
                    path: "D:\\repo\\notes.txt".into(),
                    line: Some(3),
                }]),
                content: Some(vec![diff.clone()]),
            },
        ),
        ev(5, status("c1", ToolCallStatus::Completed)),
        ev(6, chunk("t1", "Done.")),
    ]);
    assert_eq!(model.len(), 4);
    let Item::Tool(call) = &model.items()[2] else {
        panic!("{:?}", model.items()[2]);
    };
    assert_eq!(call.title, "echo hi");
    assert_eq!(call.kind, ToolKind::Edit);
    // Absent fields leave the old values; present ones replace them.
    assert_eq!(call.status, ToolCallStatus::Completed);
    assert_eq!(call.content, vec![diff]);
    assert_eq!(call.locations[0].line, Some(3));
    assert_eq!(call.revision, 2);
    assert_eq!(&changes[3..5], [Change::Updated(2), Change::Updated(2)]);
    // The text after the call is a new message.
    assert!(matches!(&model.items()[3], Item::Agent { text, .. } if text == "Done."));
}

#[test]
fn an_announced_again_tool_call_replaces_the_old_announcement() {
    let mut again = tool("c1", ToolKind::Read, "Read src/main.rs");
    again.status = ToolCallStatus::Completed;
    let (model, changes) = build(vec![
        ev(1, tool("c1", ToolKind::Read, "Read")),
        ev(2, again),
    ]);
    assert_eq!(model.len(), 1);
    assert!(matches!(
        &model.items()[0],
        Item::Tool(call) if call.title == "Read src/main.rs" && call.status == ToolCallStatus::Completed
    ));
    assert_eq!(changes, [Change::Added(0), Change::Updated(0)]);
}

#[test]
fn runs_of_reads_and_searches_collapse_into_one_group() {
    let (model, changes) = build(vec![
        ev(1, prompt("t1", "look around")),
        ev(2, tool("r1", ToolKind::Read, "Read a.rs")),
        ev(3, tool("s1", ToolKind::Search, "grep foo")),
        ev(4, tool("r2", ToolKind::Read, "Read b.rs")),
        ev(5, status("s1", ToolCallStatus::Completed)),
        ev(6, tool("x1", ToolKind::Execute, "cargo test")),
        ev(7, tool("r3", ToolKind::Read, "Read c.rs")),
        ev(8, chunk("t1", "Found it.")),
        ev(9, tool("r4", ToolKind::Read, "Read d.rs")),
        ev(10, tool("r5", ToolKind::Read, "Read e.rs")),
    ]);
    let summary: Vec<String> = model
        .items()
        .iter()
        .map(|item| match item {
            Item::Tool(call) => format!("tool {}", call.tool_call_id),
            Item::ToolGroup(group) => format!(
                "group {}",
                group
                    .iter()
                    .map(|call| call.tool_call_id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Item::User { .. } => "user".into(),
            Item::Agent { .. } => "agent".into(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        summary,
        [
            "user",
            "group r1,s1,r2",
            "tool x1",
            "tool r3",
            "agent",
            "group r4,r5"
        ]
    );
    // The second read turns the lone call's row into the group; the third joins it.
    assert_eq!(
        &changes[1..5],
        [
            Change::Added(1),
            Change::Updated(1),
            Change::Updated(1),
            Change::Updated(1)
        ]
    );
    // An update reaches a call inside a group.
    let Item::ToolGroup(group) = &model.items()[1] else {
        unreachable!()
    };
    assert_eq!(group[1].status, ToolCallStatus::Completed);
    assert_eq!(group[0].status, ToolCallStatus::Pending);
}

#[test]
fn a_turns_plan_updates_in_place_and_the_next_turn_starts_another() {
    use PlanEntryStatus::{Completed, InProgress, Pending};
    let (model, changes) = build(vec![
        ev(1, prompt("t1", "do it")),
        ev(2, plan(&[InProgress, Pending, Pending])),
        ev(3, tool("x1", ToolKind::Execute, "cargo build")),
        ev(4, plan(&[Completed, InProgress, Pending])),
        ev(5, chunk("t1", "Halfway.")),
        ev(6, plan(&[Completed, Completed, Completed])),
        ev(7, ended("t1", TurnEndReason::Completed, None)),
        ev(8, prompt("t2", "more")),
        ev(9, plan(&[Pending])),
    ]);
    let plans: Vec<(usize, Vec<PlanEntryStatus>)> = model
        .items()
        .iter()
        .enumerate()
        .filter_map(|(row, item)| match item {
            Item::Plan { entries } => Some((
                row,
                entries.iter().map(|entry| entry.status.clone()).collect(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        plans,
        [
            (1, vec![Completed, Completed, Completed]),
            (6, vec![Pending])
        ]
    );
    assert_eq!(
        changes
            .iter()
            .filter(|change| **change == Change::Updated(1))
            .count(),
        2
    );
}

#[test]
fn an_answered_permission_request_shows_the_choice() {
    let (model, _) = build(vec![
        ev(1, prompt("t1", "go")),
        ev(2, request("inb_1")),
        ev(
            3,
            PermissionResolved {
                inbox_item_id: "inb_1".into(),
                tool_call_id: "c1".into(),
                status: InboxItemStatus::Resolved,
                option_id: Some("allow".into()),
                by: Some("local".into()),
            },
        ),
    ]);
    let Item::Permission(item) = &model.items()[1] else {
        unreachable!()
    };
    assert_eq!(item.chosen_name(), Some("Allow once"));
}

#[test]
fn a_request_the_agent_withdrew_says_so_once_the_turn_carries_on() {
    let (model, changes) = build(vec![
        ev(1, prompt("t1", "/withdraw")),
        ev(2, request("inb_1")),
        ev(3, expired("inb_1")),
        ev(4, state(SessionState::Working, None)),
    ]);
    assert_eq!(
        *outcome(&model, 1),
        PermissionOutcome::Expired(Expiry::Pending)
    );
    let mut model = model;
    let mut changes = changes;
    // A tool call's own update isn't new work: a cancel produces those too.
    model.apply(&ev(5, status("c1", ToolCallStatus::Failed)), &mut changes);
    assert_eq!(
        *outcome(&model, 1),
        PermissionOutcome::Expired(Expiry::Pending)
    );
    model.apply(&ev(6, chunk("t1", "Nobody answered.")), &mut changes);
    assert_eq!(
        *outcome(&model, 1),
        PermissionOutcome::Expired(Expiry::Withdrawn)
    );
    assert!(changes.contains(&Change::Updated(1)));
    // The turn's end doesn't change it again.
    model.apply(
        &ev(7, ended("t1", TurnEndReason::Cancelled, None)),
        &mut changes,
    );
    assert_eq!(
        *outcome(&model, 1),
        PermissionOutcome::Expired(Expiry::Withdrawn)
    );
}

#[test]
fn a_request_expired_by_a_cancel_stop_or_restart_says_why() {
    // Cancelled: the request expires at once, then the turn ends `cancelled`.
    let (model, _) = build(vec![
        ev(1, prompt("t1", "go")),
        ev(2, request("inb_1")),
        ev(3, expired("inb_1")),
        ev(4, state(SessionState::Working, None)),
        ev(5, status("c1", ToolCallStatus::Failed)),
        ev(6, ended("t1", TurnEndReason::Cancelled, None)),
    ]);
    assert_eq!(
        *outcome(&model, 1),
        PermissionOutcome::Expired(Expiry::TurnEnded {
            reason: TurnEndReason::Cancelled,
            message: None
        })
    );

    // Stopped: the same commit expires it and ends the turn, with a message.
    let (model, _) = build(vec![
        ev(1, prompt("t1", "go")),
        ev(2, request("inb_1")),
        ev(3, expired("inb_1")),
        ev(
            4,
            ended(
                "t1",
                TurnEndReason::Cancelled,
                Some("the session was stopped"),
            ),
        ),
        ev(5, state(SessionState::Stopped, Some("stopped by request"))),
    ]);
    assert_eq!(
        *outcome(&model, 1),
        PermissionOutcome::Expired(Expiry::TurnEnded {
            reason: TurnEndReason::Cancelled,
            message: Some("the session was stopped".into())
        })
    );

    // The agent finished the turn without the answer: it stopped waiting.
    let (model, _) = build(vec![
        ev(1, prompt("t1", "go")),
        ev(2, request("inb_1")),
        ev(3, expired("inb_1")),
        ev(4, ended("t1", TurnEndReason::Completed, None)),
    ]);
    assert_eq!(
        *outcome(&model, 1),
        PermissionOutcome::Expired(Expiry::Withdrawn)
    );

    // A restart ends the turn first, then expires what was open.
    let (model, _) = build(vec![
        ev(1, prompt("t1", "go")),
        ev(2, request("inb_1")),
        ev(
            3,
            ended("t1", TurnEndReason::Error, Some("burrowd stopped")),
        ),
        ev(4, expired("inb_1")),
    ]);
    assert_eq!(
        *outcome(&model, 1),
        PermissionOutcome::Expired(Expiry::NoTurn)
    );
}

#[test]
fn turns_end_with_their_duration_and_summed_usage() {
    let usage = |turn: Option<&str>, input, output, cost| Usage {
        turn_id: turn.map(str::to_owned),
        input_tokens: input,
        output_tokens: output,
        cost_usd: cost,
    };
    let (model, changes) = build(vec![
        ev(1, prompt("t1", "hi")),
        ev(2, chunk("t1", "OK")),
        ev(3, usage(Some("t1"), 100, 10, Some(0.25))),
        ev(4, usage(None, 50, 5, None)),
        ev(5, ended("t1", TurnEndReason::Completed, None)),
        // Late usage for an ended turn lands on its row.
        ev(6, usage(Some("t1"), 1, 1, Some(0.5))),
        // Usage for a turn this transcript never saw is dropped.
        ev(7, usage(Some("t0"), 9, 9, None)),
    ]);
    let Item::TurnEnded(end) = &model.items()[2] else {
        panic!("{:?}", model.items());
    };
    assert_eq!(end.duration_ms, Some(4000));
    assert_eq!(
        end.usage,
        Some(TurnUsage {
            input_tokens: 151,
            output_tokens: 16,
            cost_usd: Some(0.75)
        })
    );
    assert_eq!(changes.last(), Some(&Change::Updated(2)));
    assert!(!model.in_turn());
}

#[test]
fn limits_notices_and_unknown_events_become_rows() {
    let unknown: Event = serde_json::from_str(r#"{"type":"fromTheFuture"}"#).unwrap();
    let (model, _) = build(vec![
        ev(1, prompt("t1", "/limit")),
        ev(
            2,
            LimitReached {
                provider: "claude".into(),
                resets_at: Some(at(3_600_000)),
                message: "You've hit your limit".into(),
            },
        ),
        ev(
            3,
            ended("t1", TurnEndReason::Limit, Some("usage limit reached")),
        ),
        ev(4, state(SessionState::Limited, None)),
        ev(
            5,
            ModelChanged {
                model: "fake-alt".into(),
            },
        ),
        ev(
            6,
            AgentModeChanged {
                agent_mode: "plan".into(),
            },
        ),
        ev(7, state(SessionState::Failed, Some("exit code 3"))),
        ev(8, unknown),
    ]);
    assert!(matches!(
        &model.items()[1],
        Item::Limit { provider, resets_at: Some(_), .. } if provider == "claude"
    ));
    assert_eq!(
        model.items()[3..],
        [
            Item::Notice {
                text: "Model: fake-alt".into(),
                tone: Tone::Quiet
            },
            Item::Notice {
                text: "Mode: plan".into(),
                tone: Tone::Quiet
            },
            Item::Notice {
                text: "Session failed: exit code 3".into(),
                tone: Tone::Error
            },
            Item::Unsupported {
                type_name: "fromTheFuture".into()
            },
        ]
    );
}

/// The model's share of the per-event cost: `cargo test -p burrow-app --release -- --ignored
/// replay_cost --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn replay_cost() {
    let mut events = Vec::new();
    let mut seq = 0;
    let mut next = |event: Event| {
        seq += 1;
        ev(seq, event)
    };
    for turn in 0..250 {
        let turn_id = format!("t{turn}");
        events.push(next(prompt(&turn_id, "Look into this").into()));
        for i in 0..3 {
            events.push(next(thought(&turn_id, "Thinking about it. ").into()));
            events.push(next(
                tool(&format!("r{turn}-{i}"), ToolKind::Read, "Read src/lib.rs").into(),
            ));
            events.push(next(
                status(&format!("r{turn}-{i}"), ToolCallStatus::Completed).into(),
            ));
        }
        for _ in 0..6 {
            events.push(next(
                chunk(
                    &turn_id,
                    "Some **markdown** text, about a hundred milliseconds of it. ",
                )
                .into(),
            ));
        }
        events.push(next(ended(&turn_id, TurnEndReason::Completed, None).into()));
    }
    let started = std::time::Instant::now();
    let rounds = 20;
    let mut rows = 0;
    for _ in 0..rounds {
        let (model, _) = build(events.clone());
        rows = model.len();
    }
    let per_event = started.elapsed() / (rounds * events.len() as u32);
    println!(
        "{} events -> {rows} rows; {per_event:?} per event (cloning the input included)",
        events.len()
    );
}
