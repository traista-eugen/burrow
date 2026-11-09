//! One open session: its `session:<id>` events and its metadata.
//!
//! [`SessionLog`] is the plain model: the events in seq order, plus a [`Session`] kept current
//! from them (state, model, agent mode, usage, limit). [`SessionStore`] is the GPUI entity around
//! it. It loads the session with `session/get`, follows its stream across reconnects, and emits
//! [`SessionStoreEvent`]s so views can update incrementally.
//!
//! Both halves of the session view read it: the header reads [`SessionLog::session`], the
//! transcript reads [`SessionLog::events`] and listens for [`SessionStoreEvent::Appended`].

use std::ops::Range;

use burrow_protocol::methods::SessionGet;
use burrow_protocol::{
    Event, Session, SessionIdParams, SessionState, SessionUsage, StreamEvent, StreamName,
};
use gpui::{Context, EventEmitter, Task};

use crate::backend::{self, Backend, FollowEvent};

/// A session's events and its current metadata. No GPUI in here.
#[derive(Debug, Default)]
pub struct SessionLog {
    session: Option<Session>,
    events: Vec<StreamEvent>,
    seq: u64,
    /// Usage summed from `usage` events, when there are any.
    event_usage: Option<SessionUsage>,
    /// The latest turn that reported usage, and its figures summed.
    turn_usage: Option<(String, SessionUsage)>,
    /// The latest turn started, from its `userMessage`.
    latest_turn: Option<String>,
}

impl SessionLog {
    /// An empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the metadata from `session/get`. Events pushed later update it.
    pub fn set_session(&mut self, session: Session) {
        self.session = Some(session);
        if let Some(usage) = &self.event_usage
            && let Some(session) = &mut self.session
        {
            session.usage = Some(usage.clone());
        }
    }

    /// Appends events, skipping any at or before the last seq (a replay that overlaps), and
    /// updates the metadata. Returns the range of the new events in [`Self::events`].
    pub fn push(&mut self, events: impl IntoIterator<Item = StreamEvent>) -> Range<usize> {
        let start = self.events.len();
        for event in events {
            if event.seq <= self.seq {
                continue;
            }
            self.seq = event.seq;
            self.apply(&event);
            self.events.push(event);
        }
        start..self.events.len()
    }

    /// Forgets the events, after `burrowd` answered `reset`: the history is gone, and only live
    /// events after `head` follow.
    pub fn reset(&mut self, head: u64) {
        self.events.clear();
        self.event_usage = None;
        self.turn_usage = None;
        self.latest_turn = None;
        self.seq = head;
    }

    fn apply(&mut self, event: &StreamEvent) {
        if let Event::UserMessage(message) = &event.event {
            self.latest_turn = Some(message.turn_id.clone());
        }
        if let Event::Usage(usage) = &event.event {
            add_usage(
                self.event_usage.get_or_insert_with(SessionUsage::default),
                usage,
            );
            if let Some(turn_id) = &usage.turn_id {
                match &mut self.turn_usage {
                    Some((id, total)) if id == turn_id => add_usage(total, usage),
                    _ => {
                        let mut total = SessionUsage::default();
                        add_usage(&mut total, usage);
                        self.turn_usage = Some((turn_id.clone(), total));
                    }
                }
            }
        }
        let Some(session) = &mut self.session else {
            return;
        };
        match &event.event {
            Event::UserMessage(message) => session.turn_id = Some(message.turn_id.clone()),
            Event::TurnEnded(_) => session.turn_id = None,
            Event::StateChanged(changed) => {
                session.state = changed.state.clone();
                session.reason = changed.reason.clone();
                if session.state != SessionState::Limited {
                    session.limit = None;
                }
            }
            Event::ModelChanged(changed) => session.model = Some(changed.model.clone()),
            Event::AgentModeChanged(changed) => {
                session.agent_mode = Some(changed.agent_mode.clone())
            }
            Event::LimitReached(limit) => {
                session.limit = Some(burrow_protocol::LimitInfo {
                    provider: limit.provider.clone(),
                    resets_at: limit.resets_at,
                    message: limit.message.clone(),
                })
            }
            Event::Usage(_) => session.usage = self.event_usage.clone(),
            _ => {}
        }
        session.updated_at = event.at;
    }

    /// The session's metadata, once `session/get` answered.
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// The latest turn's usage, summed from its `usage` events, with the turn's id. `None`
    /// until it reports usage, also when an earlier turn did.
    pub fn turn_usage(&self) -> Option<(&str, &SessionUsage)> {
        self.turn_usage
            .as_ref()
            .filter(|(turn_id, _)| {
                self.latest_turn
                    .as_ref()
                    .is_none_or(|latest| latest == turn_id)
            })
            .map(|(turn_id, usage)| (turn_id.as_str(), usage))
    }

    /// The texts of the prompts sent so far, oldest first: the prompt box's history.
    pub fn prompts(&self) -> impl Iterator<Item = String> + '_ {
        self.events.iter().filter_map(|event| match &event.event {
            Event::UserMessage(message) => {
                let text: Vec<&str> = message
                    .content
                    .iter()
                    .filter_map(burrow_protocol::ContentBlock::as_text)
                    .collect();
                (!text.is_empty()).then(|| text.join("\n"))
            }
            _ => None,
        })
    }

    /// The current state, or `starting` before anything is known.
    pub fn state(&self) -> SessionState {
        self.session
            .as_ref()
            .map(|session| session.state.clone())
            .unwrap_or(SessionState::Starting)
    }

    /// Whether a turn is running: `working`, or `waiting` on a human.
    pub fn is_busy(&self) -> bool {
        matches!(self.state(), SessionState::Working | SessionState::Waiting)
    }

    /// Every event so far, in seq order.
    pub fn events(&self) -> &[StreamEvent] {
        &self.events
    }

    /// The seq of the last event, where a resumed subscription carries on.
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

fn add_usage(total: &mut SessionUsage, usage: &burrow_protocol::Usage) {
    total.input_tokens += usage.input_tokens;
    total.output_tokens += usage.output_tokens;
    if let Some(cost) = usage.cost_usd {
        *total.cost_usd.get_or_insert(0.0) += cost;
    }
}

/// Whether the session has loaded.
#[derive(Clone, Debug, PartialEq)]
pub enum LoadStatus {
    /// Waiting for `burrowd`, or for `session/get`.
    Loading,
    /// The metadata is in and events follow.
    Ready,
    /// `session/get` failed, for example `notFound`.
    Failed(String),
}

/// What changed in a [`SessionStore`].
#[derive(Clone, Debug, PartialEq)]
pub enum SessionStoreEvent {
    /// Events were appended: this range of [`SessionLog::events`]. The metadata may have changed
    /// too.
    Appended(Range<usize>),
    /// The events were cleared (`reset`); rebuild from [`SessionLog::events`].
    Reset,
    /// The metadata changed without new events, for example when `session/get` answered.
    Loaded,
}

/// One open session, as a GPUI entity. Created by the session view; dropped with it, which ends
/// the subscription.
pub struct SessionStore {
    id: String,
    log: SessionLog,
    status: LoadStatus,
    /// The stream's head when the subscription started: the history is in once `seq` reaches it.
    head: Option<u64>,
    _follow: Task<()>,
}

impl EventEmitter<SessionStoreEvent> for SessionStore {}

impl SessionStore {
    /// Loads session `id` and follows its stream for as long as the entity lives.
    pub fn new(id: String, cx: &mut Context<Self>) -> Self {
        let follow = cx.spawn({
            let id = id.clone();
            async move |this, cx| {
                // The metadata first, so the events replayed after it bring it up to date.
                let backend = cx.update(|cx| Backend::global(cx).clone());
                loop {
                    let client = backend.connected().await;
                    let params = SessionIdParams {
                        session_id: id.clone(),
                    };
                    match client.call::<SessionGet>(&params).await {
                        Ok(session) => {
                            let updated = this.update(cx, |this, cx| {
                                this.log.set_session(session);
                                this.status = LoadStatus::Ready;
                                cx.emit(SessionStoreEvent::Loaded);
                                cx.notify();
                            });
                            if updated.is_err() {
                                return;
                            }
                            break;
                        }
                        Err(burrow_client::CallError::Disconnected) => continue,
                        Err(error) => {
                            let _ = this.update(cx, |this, cx| {
                                this.status = LoadStatus::Failed(error.to_string());
                                cx.notify();
                            });
                            return;
                        }
                    }
                }
                backend::follow(
                    backend,
                    this,
                    StreamName::session(&id),
                    |this: &SessionStore| Some(this.log.seq()),
                    |this, event, cx| match event {
                        FollowEvent::Subscribed { head, reset } => {
                            this.head = Some(head);
                            if reset {
                                this.log.reset(head);
                                cx.emit(SessionStoreEvent::Reset);
                                cx.notify();
                            }
                        }
                        FollowEvent::Events(events) => {
                            let range = this.log.push(events);
                            if !range.is_empty() {
                                cx.emit(SessionStoreEvent::Appended(range));
                                cx.notify();
                            }
                        }
                    },
                    cx,
                )
                .await;
            }
        });
        Self {
            id,
            log: SessionLog::new(),
            status: LoadStatus::Loading,
            head: None,
            _follow: follow,
        }
    }

    /// Whether the session has loaded and its history has replayed up to where the stream was
    /// when it was opened.
    pub fn is_caught_up(&self) -> bool {
        self.status == LoadStatus::Ready && self.head.is_some_and(|head| self.log.seq() >= head)
    }

    /// The session's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The events and metadata.
    pub fn log(&self) -> &SessionLog {
        &self.log
    }

    /// Whether it has loaded.
    pub fn status(&self) -> &LoadStatus {
        &self.status
    }
}

#[cfg(test)]
mod tests {
    use burrow_protocol::{
        AgentId, AgentMessageChunk, LimitReached, ModelChanged, SessionMode, StateChanged,
        Timestamp, TurnEndReason, TurnEnded, Usage, UserMessage,
    };

    use super::*;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(1_800_000_000_000 + ms).unwrap()
    }

    fn ev(seq: u64, event: impl Into<Event>) -> StreamEvent {
        StreamEvent {
            stream: StreamName::session("ses_1"),
            seq,
            at: at(seq as i64),
            event: event.into(),
        }
    }

    fn session() -> Session {
        Session {
            id: "ses_1".into(),
            project_id: None,
            agent: AgentId::Fake,
            model: Some("fake".into()),
            mode: SessionMode::Structured,
            cwd: "D:\\repo".into(),
            title: None,
            state: SessionState::Idle,
            reason: None,
            turn_id: None,
            models: Vec::new(),
            agent_mode: None,
            agent_modes: Vec::new(),
            usage: None,
            limit: None,
            archived: false,
            created_at: at(0),
            updated_at: at(0),
            last_activity_at: None,
        }
    }

    fn state(state: &str) -> StateChanged {
        StateChanged {
            state: SessionState::from(state),
            reason: None,
        }
    }

    #[test]
    fn events_keep_the_metadata_current() {
        let mut log = SessionLog::new();
        log.set_session(session());
        let range = log.push([
            ev(
                1,
                UserMessage {
                    turn_id: "trn_1".into(),
                    content: vec![burrow_protocol::ContentBlock::text("hi")],
                },
            ),
            ev(2, state("working")),
            ev(
                3,
                AgentMessageChunk {
                    turn_id: "trn_1".into(),
                    text: "Echo: hi".into(),
                },
            ),
        ]);
        assert_eq!(range, 0..3);
        assert!(log.is_busy());
        assert_eq!(log.session().unwrap().turn_id.as_deref(), Some("trn_1"));

        log.push([
            ev(
                4,
                Usage {
                    turn_id: Some("trn_1".into()),
                    input_tokens: 10,
                    output_tokens: 5,
                    cost_usd: Some(0.5),
                },
            ),
            ev(
                5,
                TurnEnded {
                    turn_id: "trn_1".into(),
                    reason: TurnEndReason::Completed,
                    message: None,
                },
            ),
            ev(6, state("idle")),
            ev(
                7,
                ModelChanged {
                    model: "fake-alt".into(),
                },
            ),
            ev(
                8,
                Usage {
                    turn_id: None,
                    input_tokens: 1,
                    output_tokens: 1,
                    cost_usd: None,
                },
            ),
        ]);
        let meta = log.session().unwrap();
        assert_eq!(meta.state, SessionState::Idle);
        assert_eq!(meta.turn_id, None);
        assert_eq!(meta.model.as_deref(), Some("fake-alt"));
        let usage = meta.usage.as_ref().unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (11, 6));
        assert_eq!(usage.cost_usd, Some(0.5));
        assert_eq!(log.seq(), 8);
    }

    #[test]
    fn an_overlapping_replay_is_skipped() {
        let mut log = SessionLog::new();
        log.set_session(session());
        log.push([ev(1, state("working")), ev(2, state("idle"))]);
        // A resumed subscription that starts too early repeats events: they're dropped.
        let range = log.push([ev(2, state("idle")), ev(3, state("working"))]);
        assert_eq!(range, 2..3);
        assert_eq!(log.events().len(), 3);
        assert_eq!(log.state(), SessionState::Working);
    }

    #[test]
    fn limits_and_reset() {
        let mut log = SessionLog::new();
        log.set_session(session());
        log.push([
            ev(
                1,
                LimitReached {
                    provider: "fake".into(),
                    resets_at: Some(at(3_600_000)),
                    message: "limit".into(),
                },
            ),
            ev(2, state("limited")),
        ]);
        assert_eq!(
            log.session().unwrap().limit.as_ref().unwrap().provider,
            "fake"
        );
        log.push([ev(3, state("working"))]);
        assert!(log.session().unwrap().limit.is_none());

        log.reset(10);
        assert!(log.events().is_empty());
        assert_eq!(log.seq(), 10);
        assert_eq!(log.push([ev(9, state("idle"))]), 0..0);
        assert_eq!(log.push([ev(11, state("idle"))]), 0..1);
    }

    #[test]
    fn the_latest_turn_has_its_own_usage_and_prompts_make_the_history() {
        let mut log = SessionLog::new();
        log.set_session(session());
        let usage = |turn: Option<&str>, input, output, cost| Usage {
            turn_id: turn.map(Into::into),
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
        };
        let prompt = |turn: &str, texts: &[&str]| UserMessage {
            turn_id: turn.into(),
            content: texts
                .iter()
                .map(|text| burrow_protocol::ContentBlock::text(*text))
                .collect(),
        };
        log.push([
            ev(1, prompt("trn_1", &["first"])),
            ev(2, usage(Some("trn_1"), 10, 5, Some(0.25))),
            ev(3, usage(Some("trn_1"), 2, 1, None)),
        ]);
        let (turn, figures) = log.turn_usage().unwrap();
        assert_eq!(turn, "trn_1");
        assert_eq!((figures.input_tokens, figures.output_tokens), (12, 6));
        assert_eq!(figures.cost_usd, Some(0.25));

        // A new turn starts its own figures; usage without a turn counts for the session only.
        log.push([
            ev(4, prompt("trn_2", &["second", "line"])),
            ev(5, usage(Some("trn_2"), 1, 1, None)),
            ev(6, usage(None, 100, 100, None)),
        ]);
        let (turn, figures) = log.turn_usage().unwrap();
        assert_eq!(turn, "trn_2");
        assert_eq!((figures.input_tokens, figures.output_tokens), (1, 1));
        assert_eq!(figures.cost_usd, None);
        let total = log.session().unwrap().usage.as_ref().unwrap();
        assert_eq!((total.input_tokens, total.output_tokens), (113, 107));
        assert_eq!(log.prompts().collect::<Vec<_>>(), ["first", "second\nline"]);

        // A newer turn that reports nothing (yet) shows no turn figures, not the last ones.
        log.push([ev(7, prompt("trn_3", &["third"]))]);
        assert!(log.turn_usage().is_none());
    }

    #[test]
    fn events_before_the_metadata_still_count_usage() {
        let mut log = SessionLog::new();
        log.push([ev(
            1,
            Usage {
                turn_id: None,
                input_tokens: 3,
                output_tokens: 4,
                cost_usd: None,
            },
        )]);
        assert_eq!(log.state(), SessionState::Starting);
        log.set_session(session());
        assert_eq!(
            log.session().unwrap().usage.as_ref().unwrap().input_tokens,
            3
        );
    }
}
