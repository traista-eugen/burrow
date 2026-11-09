//! The store thread: the one owner of the event store, and the source of live events (D-014).
//!
//! A dedicated `std::thread` owns the [`Store`] and serves [`Hub`] commands in order. Because one
//! thread does every append, it can also tell live subscribers about each event right after it
//! is committed, in seq order, and register a new subscriber at an exact position:
//!
//! - **Subscribing** reads the stream's head and registers the subscriber in one step, with no
//!   append in between. Every event after that head reaches the subscriber live; everything up to
//!   it is history, read with [`Hub::read`].
//! - **Appending** commits, answers the caller, then hands each new event to the stream's
//!   subscribers. The `events/event` line is encoded once and shared.
//! - **Group commit.** Every commit costs an fsync (D-012). When several appends to the same
//!   stream are already queued, the thread commits them as one transaction and splits the seqs.
//!
//! The thread never waits on a subscriber. Each one has a bounded buffer; a subscriber whose
//! buffer is full is dropped, which ends its connection (D-015).

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;

use burrow_protocol::StreamName;
use burrow_store::{Appended, Event as StoredEvent, NewEvent, Store};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, warn};

use crate::wire;

/// The most events one group commit may hold.
const MAX_BATCH_EVENTS: usize = 512;

/// One event delivered live to a subscriber.
#[derive(Clone, Debug)]
pub struct LiveEvent {
    /// The event's seq in its stream.
    pub seq: u64,
    /// The complete `events/event` notification line, shared by every subscriber.
    pub line: Arc<str>,
}

/// A subscriber registered with the store thread.
#[derive(Clone, Copy, Debug)]
pub struct Registration {
    /// Identifies the subscriber for [`Hub::unsubscribe`].
    pub id: u64,
    /// The stream's head when the subscriber was registered. Events after it arrive live.
    pub head: u64,
}

/// Why a store command failed.
#[derive(Clone, Debug, thiserror::Error)]
pub enum HubError {
    /// The store failed, for example because the disk is full.
    #[error("event store: {0}")]
    Store(Arc<burrow_store::Error>),
    /// The store thread has stopped.
    #[error("the event store has stopped")]
    Stopped,
}

enum Command {
    Append {
        stream: StreamName,
        events: Vec<NewEvent>,
        reply: oneshot::Sender<Result<Appended, HubError>>,
    },
    Read {
        stream: StreamName,
        after: u64,
        limit: usize,
        reply: oneshot::Sender<Result<Vec<StoredEvent>, HubError>>,
    },
    Subscribe {
        stream: StreamName,
        sender: mpsc::Sender<LiveEvent>,
        reply: oneshot::Sender<Result<Registration, HubError>>,
    },
    Unsubscribe {
        stream: StreamName,
        id: u64,
    },
}

/// A handle to the store thread. Cheap to clone; the thread stops when every handle is gone.
#[derive(Clone, Debug)]
pub struct Hub {
    commands: mpsc::UnboundedSender<Command>,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Append { stream, events, .. } => {
                write!(f, "Append({stream}, {} events)", events.len())
            }
            Self::Read { stream, after, .. } => write!(f, "Read({stream}, after {after})"),
            Self::Subscribe { stream, .. } => write!(f, "Subscribe({stream})"),
            Self::Unsubscribe { stream, id } => write!(f, "Unsubscribe({stream}, {id})"),
        }
    }
}

impl Hub {
    /// Moves `store` onto a new thread and returns the handle to it.
    pub fn start(store: Store) -> std::io::Result<Self> {
        let (commands, inbox) = mpsc::unbounded_channel();
        thread::Builder::new()
            .name("burrow-store".into())
            .spawn(move || StoreThread::new(store).run(inbox))?;
        Ok(Self { commands })
    }

    /// Appends `events` to `stream` atomically, then delivers them to live subscribers.
    ///
    /// `events` must not be empty, and every event type and the stream name must be non-empty.
    pub async fn append(
        &self,
        stream: StreamName,
        events: Vec<NewEvent>,
    ) -> Result<Appended, HubError> {
        debug_assert!(!events.is_empty(), "an append needs at least one event");
        self.ask(|reply| Command::Append {
            stream,
            events,
            reply,
        })
        .await
    }

    /// Up to `limit` events of `stream` after seq `after`, in order.
    pub async fn read(
        &self,
        stream: StreamName,
        after: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, HubError> {
        self.ask(|reply| Command::Read {
            stream,
            after,
            limit,
            reply,
        })
        .await
    }

    /// Registers a live subscriber. From now on, every event appended to `stream` is offered to
    /// `sender`; if its buffer is full the subscriber is dropped, and `sender`'s receiver sees
    /// the channel close.
    pub async fn subscribe(
        &self,
        stream: StreamName,
        sender: mpsc::Sender<LiveEvent>,
    ) -> Result<Registration, HubError> {
        self.ask(|reply| Command::Subscribe {
            stream,
            sender,
            reply,
        })
        .await
    }

    /// Removes a live subscriber. Doesn't wait; safe to call from `Drop`.
    pub fn unsubscribe(&self, stream: StreamName, id: u64) {
        let _ = self.commands.send(Command::Unsubscribe { stream, id });
    }

    async fn ask<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, HubError>>) -> Command,
    ) -> Result<T, HubError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(command(reply))
            .map_err(|_| HubError::Stopped)?;
        answer.await.unwrap_or(Err(HubError::Stopped))
    }
}

struct Subscriber {
    id: u64,
    sender: mpsc::Sender<LiveEvent>,
}

struct StoreThread {
    store: Store,
    subscribers: HashMap<StreamName, Vec<Subscriber>>,
    next_subscriber: u64,
}

impl StoreThread {
    fn new(store: Store) -> Self {
        Self {
            store,
            subscribers: HashMap::new(),
            next_subscriber: 1,
        }
    }

    fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Command>) {
        // A command taken off the queue while gathering a group commit, to handle next.
        let mut next: Option<Command> = None;
        loop {
            let command = match next.take() {
                Some(command) => command,
                None => match inbox.blocking_recv() {
                    Some(command) => command,
                    None => break,
                },
            };
            match command {
                Command::Append {
                    stream,
                    events,
                    reply,
                } => {
                    next = self.append(&mut inbox, stream, events, reply);
                }
                Command::Read {
                    stream,
                    after,
                    limit,
                    reply,
                } => {
                    let result = self.store.read(stream.as_str(), after, limit);
                    let _ = reply.send(result.map_err(|error| HubError::Store(Arc::new(error))));
                }
                Command::Subscribe {
                    stream,
                    sender,
                    reply,
                } => {
                    let result = self.store.head(stream.as_str()).map(|head| {
                        let id = self.next_subscriber;
                        self.next_subscriber += 1;
                        self.subscribers
                            .entry(stream)
                            .or_default()
                            .push(Subscriber { id, sender });
                        Registration { id, head }
                    });
                    let _ = reply.send(result.map_err(|error| HubError::Store(Arc::new(error))));
                }
                Command::Unsubscribe { stream, id } => {
                    if let Some(list) = self.subscribers.get_mut(&stream) {
                        list.retain(|subscriber| subscriber.id != id);
                        if list.is_empty() {
                            self.subscribers.remove(&stream);
                        }
                    }
                }
            }
        }
        debug!("store thread stopped");
    }

    /// Commits one append, plus any appends to the same stream already waiting behind it.
    /// Returns the first queued command that couldn't join the batch.
    fn append(
        &mut self,
        inbox: &mut mpsc::UnboundedReceiver<Command>,
        stream: StreamName,
        mut events: Vec<NewEvent>,
        reply: oneshot::Sender<Result<Appended, HubError>>,
    ) -> Option<Command> {
        let mut callers = vec![(events.len(), reply)];
        let mut next = None;
        while events.len() < MAX_BATCH_EVENTS {
            match inbox.try_recv() {
                Ok(Command::Append {
                    stream: other,
                    events: more,
                    reply,
                }) if other == stream => {
                    callers.push((more.len(), reply));
                    events.extend(more);
                }
                Ok(command) => {
                    next = Some(command);
                    break;
                }
                Err(_) => break,
            }
        }

        match self.store.append(stream.as_str(), &events) {
            Ok(appended) => {
                if callers.len() > 1 {
                    debug!(%stream, appends = callers.len(), events = events.len(), "group commit");
                }
                let mut first = *appended.seqs.start();
                for (count, reply) in callers {
                    let count = count as u64;
                    let _ = reply.send(Ok(Appended {
                        seqs: first..=first + count - 1,
                        at_ms: appended.at_ms,
                    }));
                    first += count;
                }
                self.fan_out(&stream, &events, &appended);
            }
            Err(error) => {
                error!(%stream, %error, "append failed");
                let error = Arc::new(error);
                for (_, reply) in callers {
                    let _ = reply.send(Err(HubError::Store(error.clone())));
                }
            }
        }
        next
    }

    /// Offers freshly committed events to the stream's live subscribers, in seq order.
    fn fan_out(&mut self, stream: &StreamName, events: &[NewEvent], appended: &Appended) {
        let Some(subscribers) = self.subscribers.get_mut(stream) else {
            return;
        };
        for (seq, event) in appended.seqs.clone().zip(events) {
            let live = LiveEvent {
                seq,
                line: wire::event_line(stream, seq, appended.at_ms, &event.payload).into(),
            };
            subscribers.retain(
                |subscriber| match subscriber.sender.try_send(live.clone()) {
                    Ok(()) => true,
                    Err(TrySendError::Full(_)) => {
                        warn!(%stream, subscriber = subscriber.id, seq, "subscriber fell behind; dropping it");
                        false
                    }
                    Err(TrySendError::Closed(_)) => false,
                },
            );
        }
        if subscribers.is_empty() {
            self.subscribers.remove(stream);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::*;

    fn event(n: u64) -> NewEvent {
        NewEvent::json("test", &json!({ "type": "test", "n": n })).unwrap()
    }

    fn hub() -> Hub {
        Hub::start(Store::open_in_memory().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn appends_get_consecutive_seqs_and_reads_see_them() {
        let hub = hub();
        let stream = StreamName::index();
        let first = hub.append(stream.clone(), vec![event(1)]).await.unwrap();
        assert_eq!(first.seqs, 1..=1);
        let more = hub
            .append(stream.clone(), vec![event(2), event(3)])
            .await
            .unwrap();
        assert_eq!(more.seqs, 2..=3);
        let read = hub.read(stream, 1, 10).await.unwrap();
        assert_eq!(read.iter().map(|e| e.seq).collect::<Vec<_>>(), [2, 3]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_appends_never_share_a_seq() {
        let hub = hub();
        let tasks: Vec<_> = (0..16)
            .map(|task| {
                let hub = hub.clone();
                tokio::spawn(async move {
                    let mut seqs = Vec::new();
                    for i in 0..20 {
                        let appended = hub
                            .append(StreamName::index(), vec![event(task * 100 + i)])
                            .await
                            .unwrap();
                        seqs.extend(appended.seqs);
                    }
                    seqs
                })
            })
            .collect();
        let mut all = HashSet::new();
        for task in tasks {
            for seq in task.await.unwrap() {
                assert!(all.insert(seq), "seq {seq} given twice");
            }
        }
        assert_eq!(all, (1..=320).collect());
    }

    #[tokio::test]
    async fn subscribers_get_events_after_their_head_in_order() {
        let hub = hub();
        let stream = StreamName::session("ses_1");
        hub.append(stream.clone(), vec![event(1)]).await.unwrap();

        let (sender, mut live) = mpsc::channel(16);
        let registration = hub.subscribe(stream.clone(), sender).await.unwrap();
        assert_eq!(registration.head, 1);

        hub.append(stream.clone(), vec![event(2), event(3)])
            .await
            .unwrap();
        hub.append(StreamName::index(), vec![event(99)])
            .await
            .unwrap();
        for seq in [2, 3] {
            let event = live.recv().await.unwrap();
            assert_eq!(event.seq, seq);
            assert!(event.line.contains(r#""stream":"session:ses_1""#));
        }
        assert!(live.try_recv().is_err());

        hub.unsubscribe(stream.clone(), registration.id);
        hub.append(stream, vec![event(4)]).await.unwrap();
        assert!(live.recv().await.is_none(), "unsubscribed");
    }

    #[tokio::test]
    async fn a_full_subscriber_is_dropped_without_blocking_appends() {
        let hub = hub();
        let (sender, mut live) = mpsc::channel(2);
        hub.subscribe(StreamName::index(), sender).await.unwrap();
        for n in 1..=5 {
            hub.append(StreamName::index(), vec![event(n)])
                .await
                .unwrap();
        }
        assert_eq!(live.recv().await.unwrap().seq, 1);
        assert_eq!(live.recv().await.unwrap().seq, 2);
        assert!(
            live.recv().await.is_none(),
            "dropped when its buffer filled"
        );
    }
}
