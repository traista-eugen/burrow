//! What Muse tells the client: notifications, plus the requests it sends for approvals and
//! user input, as one typed stream.

use std::pin::Pin;
use std::task::{Context, Poll};

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::methods::names;
use crate::types::*;

/// One thing Muse told the client, in the order it arrived.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// `session/started`.
    SessionStarted(SessionStartedParams),
    /// `session/statusChanged`.
    SessionStatusChanged(SessionStatusChangedParams),
    /// `session/closed`. On shutdown each loaded session closes with `hostShutdown`.
    SessionClosed(SessionClosedParams),
    /// `session/modelChanged`.
    ModelChanged(SessionModelChangedParams),
    /// `turn/started`.
    TurnStarted(TurnStartedParams),
    /// `turn/foregroundCompleted`: the model is done, reminder agents still run.
    TurnForegroundCompleted(TurnForegroundCompletedParams),
    /// `turn/completed`: the turn's one terminal event.
    TurnCompleted(TurnCompletedParams),
    /// `item/started`.
    ItemStarted(ItemParams),
    /// `item/updated`.
    ItemUpdated(ItemParams),
    /// `item/completed`.
    ItemCompleted(ItemParams),
    /// `item/delta`, with the kind of the item it appends to when the client saw it open.
    ItemDelta {
        /// The delta.
        delta: ItemDeltaParams,
        /// The open item's kind: `agentMessage` for reply text, `reasoning` for thoughts,
        /// `toolCall` for output. `None` if the item's start wasn't seen.
        kind: Option<ItemKind>,
    },
    /// An approval requirement waits for a decision: send one with
    /// [`crate::MspClient::decide_approval`]. Sent once per requirement, whether Muse's
    /// `approval/request` or its `approval/requested` arrives first; the receipt is already
    /// sent. A multi-stage shell command gets one event per stage.
    ApprovalRequested(ApprovalRequestParams),
    /// `approval/updated`: a stage was satisfied, a reviewer started or finished, or a host
    /// restart refreshed the choices (`restartRecovered`). Its `availableChoices` and
    /// `currentRequirementId` replace the ones from the request.
    ApprovalUpdated(ApprovalUpdatedParams),
    /// `approval/resolved`: the final decision.
    ApprovalResolved(ApprovalResolvedParams),
    /// A user-input prompt waits for an answer, once per prompt. The receipt is already sent.
    UserInputRequested(UserInputRequestParams),
    /// `userInput/settled`.
    UserInputSettled(UserInputSettledParams),
    /// `session/tokenUsage`: one model call's tokens.
    TokenUsage(SessionTokenUsageParams),
    /// `session/contextUsage`.
    ContextUsage(SessionContextUsageParams),
    /// `usage/changed`: the subscription usage changed.
    UsageChanged(SubscriptionUsage),
    /// `view/gap`: Muse dropped view events.
    ViewGap(ViewGapParams),
    /// A notification this crate doesn't model, kept raw.
    Other {
        /// The method.
        method: String,
        /// The params.
        params: Option<Value>,
    },
    /// A notification this crate models, but whose params didn't decode. A bug here or an
    /// incompatible Muse; kept raw so nothing is silently lost.
    Malformed {
        /// The method.
        method: String,
        /// The params.
        params: Option<Value>,
        /// Why decoding failed.
        error: String,
    },
}

impl Event {
    /// The session the event belongs to, when it names one.
    pub fn session_id(&self) -> Option<&str> {
        Some(match self {
            Self::SessionStarted(e) => &e.session.session_id,
            Self::SessionStatusChanged(e) => &e.session_id,
            Self::SessionClosed(e) => &e.session_id,
            Self::ModelChanged(e) => &e.session_id,
            Self::TurnStarted(e) => &e.session_id,
            Self::TurnForegroundCompleted(e) => &e.session_id,
            Self::TurnCompleted(e) => &e.session_id,
            Self::ItemStarted(e) | Self::ItemUpdated(e) | Self::ItemCompleted(e) => &e.session_id,
            Self::ItemDelta { delta, .. } => &delta.session_id,
            Self::ApprovalRequested(e) => &e.session_id,
            Self::ApprovalUpdated(e) => &e.session_id,
            Self::ApprovalResolved(e) => &e.session_id,
            Self::UserInputRequested(e) => &e.session_id,
            Self::UserInputSettled(e) => &e.session_id,
            Self::TokenUsage(e) => &e.session_id,
            Self::ContextUsage(e) => &e.session_id,
            Self::ViewGap(e) => &e.session_id,
            Self::UsageChanged(_) | Self::Other { .. } | Self::Malformed { .. } => return None,
        })
    }
}

/// Decodes a notification, except the ones the client handles itself (approval and user-input
/// requests, and deltas, which need the client's record of open items).
pub(crate) fn decode(method: String, params: Option<Value>) -> Event {
    fn typed<T: DeserializeOwned>(
        method: String,
        params: Option<Value>,
        wrap: fn(T) -> Event,
    ) -> Event {
        match serde_json::from_value::<T>(params.clone().unwrap_or(Value::Null)) {
            Ok(value) => wrap(value),
            Err(error) => Event::Malformed {
                method,
                params,
                error: error.to_string(),
            },
        }
    }

    match method.as_str() {
        names::SESSION_STARTED => typed(method, params, Event::SessionStarted),
        names::SESSION_STATUS_CHANGED => typed(method, params, Event::SessionStatusChanged),
        names::SESSION_CLOSED => typed(method, params, Event::SessionClosed),
        names::SESSION_MODEL_CHANGED => typed(method, params, Event::ModelChanged),
        names::TURN_STARTED => typed(method, params, Event::TurnStarted),
        names::TURN_FOREGROUND_COMPLETED => typed(method, params, Event::TurnForegroundCompleted),
        names::TURN_COMPLETED => typed(method, params, Event::TurnCompleted),
        names::ITEM_STARTED => typed(method, params, Event::ItemStarted),
        names::ITEM_UPDATED => typed(method, params, Event::ItemUpdated),
        names::ITEM_COMPLETED => typed(method, params, Event::ItemCompleted),
        names::APPROVAL_UPDATED => typed(method, params, Event::ApprovalUpdated),
        names::APPROVAL_RESOLVED => typed(method, params, Event::ApprovalResolved),
        names::USER_INPUT_SETTLED => typed(method, params, Event::UserInputSettled),
        names::SESSION_TOKEN_USAGE => typed(method, params, Event::TokenUsage),
        names::SESSION_CONTEXT_USAGE => typed(method, params, Event::ContextUsage),
        names::USAGE_CHANGED => typed(method, params, Event::UsageChanged),
        names::VIEW_GAP => typed(method, params, Event::ViewGap),
        _ => Event::Other { method, params },
    }
}

/// The client's events, in arrival order. Ends when the connection closes.
///
/// The queue is unbounded, so read it promptly. Dropping it stops delivery; Muse's approval and
/// user-input requests are then answered with an error, so they stay pending for another client.
#[derive(Debug)]
pub struct Events {
    pub(crate) receiver: mpsc::UnboundedReceiver<Event>,
}

impl Events {
    /// The next event; `None` once the connection is closed and every event was read.
    pub async fn recv(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }

    /// The next event if one is queued, without waiting.
    pub fn try_recv(&mut self) -> Option<Event> {
        self.receiver.try_recv().ok()
    }
}

impl futures_core::Stream for Events {
    type Item = Event;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        self.receiver.poll_recv(cx)
    }
}
