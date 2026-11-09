use std::pin::Pin;
use std::task::{Context, Poll};

use burrow_protocol::methods::EventsUnsubscribe;
use burrow_protocol::{
    Notification, StreamEvent, StreamName, SubscribeParams, SubscribeResult, UnsubscribeParams,
};
use futures_core::Stream;
use tokio::sync::mpsc;

use crate::{CallError, Client};

/// A subscription to one stream: replayed history, then live events, in seq order.
///
/// It yields [`StreamEvent`]s, through [`Subscription::next`] or as a [`Stream`], and ends
/// (`None`) when the connection closes. It remembers the last seq it yielded, so after a
/// reconnect [`Subscription::resume_params`] picks up exactly where it stopped.
///
/// Dropping it unsubscribes, without waiting. A subscription keeps its connection open.
#[derive(Debug)]
pub struct Subscription {
    client: Client,
    stream: StreamName,
    route: u64,
    head: u64,
    reset: bool,
    position: u64,
    events: mpsc::UnboundedReceiver<StreamEvent>,
    active: bool,
}

impl Subscription {
    pub(crate) fn new(
        client: Client,
        route: u64,
        params: &SubscribeParams,
        result: &SubscribeResult,
        events: mpsc::UnboundedReceiver<StreamEvent>,
    ) -> Self {
        // Where the stream's events start for this subscription: right after `after`, or after
        // the head for live-only and reset subscriptions.
        let position = match params.after {
            Some(after) if !result.reset => after,
            _ => result.head,
        };
        Self {
            client,
            stream: params.stream.clone(),
            route,
            head: result.head,
            reset: result.reset,
            position,
            events,
            active: true,
        }
    }

    /// The stream this subscription follows.
    pub fn stream(&self) -> &StreamName {
        &self.stream
    }

    /// The stream's head when the subscription started: the seq of its latest event, or 0.
    pub fn head(&self) -> u64 {
        self.head
    }

    /// Whether `burrowd` answered `reset`: the requested history isn't available (for example,
    /// `after` was beyond the head), so nothing was replayed and the caller should reload state.
    /// Live events after [`head`](Self::head) still follow.
    pub fn reset(&self) -> bool {
        self.reset
    }

    /// The seq of the last event yielded, or, before the first one, the position the
    /// subscription started from.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// The params that subscribe again from [`position`](Self::position), on this or a new
    /// connection, without missing or repeating an event.
    pub fn resume_params(&self) -> SubscribeParams {
        SubscribeParams {
            stream: self.stream.clone(),
            after: Some(self.position),
        }
    }

    /// The next event. `None` once the connection has closed (or `burrowd` sent an event this
    /// client can't decode, which ends the subscription rather than skip a seq).
    pub async fn next(&mut self) -> Option<StreamEvent> {
        std::future::poll_fn(|cx| Pin::new(&mut *self).poll_next(cx)).await
    }

    /// Ends the subscription and waits for `burrowd` to confirm. After this, no more of its
    /// events arrive.
    pub async fn unsubscribe(mut self) -> Result<(), CallError> {
        self.active = false;
        self.client
            .connection()
            .remove_route(&self.stream, self.route);
        self.client
            .call::<EventsUnsubscribe>(&UnsubscribeParams {
                stream: self.stream.clone(),
            })
            .await
            .map(|_| ())
    }
}

impl Stream for Subscription {
    type Item = StreamEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<StreamEvent>> {
        let poll = self.events.poll_recv(cx);
        if let Poll::Ready(Some(event)) = &poll {
            self.position = event.seq;
        }
        poll
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if self.active {
            self.client.connection().detach(&self.stream, self.route);
        }
    }
}

/// Incoming notifications other than `events/event`. See [`Client::notifications`].
#[derive(Debug)]
pub struct Notifications {
    receiver: mpsc::UnboundedReceiver<Notification>,
}

impl Notifications {
    pub(crate) fn new(receiver: mpsc::UnboundedReceiver<Notification>) -> Self {
        Self { receiver }
    }

    /// The next notification. `None` once the connection has closed, or another
    /// [`Client::notifications`] stream replaced this one.
    pub async fn next(&mut self) -> Option<Notification> {
        self.receiver.recv().await
    }
}

impl Stream for Notifications {
    type Item = Notification;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Notification>> {
        self.receiver.poll_recv(cx)
    }
}
