//! The store thread: the one owner of the event store, and the source of live events (D-014).
//!
//! A dedicated `std::thread` owns the [`Store`] and serves [`Hub`] commands in order. Because one
//! thread does every append, it can also tell live subscribers about each event right after it
//! is committed, in seq order, and register a new subscriber at an exact position:
//!
//! - **Subscribing** reads the stream's head and registers the subscriber in one step, with no
//!   append in between. Every event after that head reaches the subscriber live; everything up to
//!   it is history, read with [`Hub::read`].
//! - **Committing** writes events to one or more streams, plus entity rows (D-017), in one
//!   transaction, answers the caller, then hands each new event to its stream's subscribers. The
//!   `events/event` line is encoded once and shared.
//! - **Group commit.** Every commit costs an fsync (D-012). When several commits are already
//!   queued, the thread writes them as one transaction and splits the seqs.
//!
//! The thread never waits on a subscriber. Each one has a bounded buffer; a subscriber whose
//! buffer is full is dropped, which ends its connection (D-015).

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;

use burrow_protocol::{Event, StreamName};
use burrow_store::{Append, Appended, Entity, EntityWrite, Event as StoredEvent, NewEvent, Store};
use serde::Serialize;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, warn};

use crate::wire;

/// The most events one group commit may hold.
const MAX_BATCH_EVENTS: usize = 512;

/// Events for one or more streams, plus entity rows, to write in one transaction (D-017).
///
/// Events for the same stream stay in the order added and get consecutive seqs. Entity rows are
/// the current state of what the events change; write them only together with those events.
#[derive(Debug, Default)]
pub struct Commit {
    appends: Vec<(StreamName, Vec<NewEvent>)>,
    entities: Vec<EntityWrite>,
}

impl Commit {
    /// An empty commit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `event` to `stream`.
    pub fn event(&mut self, stream: &StreamName, event: impl Into<Event>) -> &mut Self {
        self.record(stream, wire::record(&event.into()))
    }

    /// Adds an already encoded record to `stream`.
    pub fn record(&mut self, stream: &StreamName, record: NewEvent) -> &mut Self {
        match self.appends.iter_mut().find(|(name, _)| name == stream) {
            Some((_, events)) => events.push(record),
            None => self.appends.push((stream.clone(), vec![record])),
        }
        self
    }

    /// Writes an entity's current state.
    pub fn put(&mut self, kind: &str, id: &str, body: &impl Serialize) -> &mut Self {
        self.entities.push(
            EntityWrite::put(kind, id, body).expect("protocol types always serialize to JSON"),
        );
        self
    }

    /// Removes an entity's row.
    pub fn delete(&mut self, kind: &str, id: &str) -> &mut Self {
        self.entities.push(EntityWrite::delete(kind, id));
        self
    }

    /// Whether there is nothing to write.
    pub fn is_empty(&self) -> bool {
        self.appends.is_empty() && self.entities.is_empty()
    }

    fn event_count(&self) -> usize {
        self.appends.iter().map(|(_, events)| events.len()).sum()
    }
}

/// What a commit wrote: the seqs each stream's events got, in the order the streams were first
/// added.
#[derive(Clone, Debug)]
pub struct Committed {
    /// One entry per stream.
    pub appended: Vec<(StreamName, Appended)>,
}

#[cfg(test)]
impl Committed {
    /// The seqs `stream`'s events got, if the commit had any.
    pub fn seqs(&self, stream: &StreamName) -> Option<&Appended> {
        self.appended
            .iter()
            .find(|(name, _)| name == stream)
            .map(|(_, appended)| appended)
    }
}

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
    Commit {
        commit: Commit,
        reply: oneshot::Sender<Result<Committed, HubError>>,
    },
    Entities {
        kind: String,
        reply: oneshot::Sender<Result<Vec<Entity>, HubError>>,
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
            Self::Commit { commit, .. } => write!(
                f,
                "Commit({} streams, {} events, {} entities)",
                commit.appends.len(),
                commit.event_count(),
                commit.entities.len()
            ),
            Self::Entities { kind, .. } => write!(f, "Entities({kind})"),
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
        let mut commit = Commit::new();
        for event in events {
            commit.record(&stream, event);
        }
        let committed = self.commit(commit).await?;
        Ok(committed
            .appended
            .into_iter()
            .next()
            .map(|(_, appended)| appended)
            .expect("one stream was appended"))
    }

    /// Writes `commit` in one transaction, then delivers its events to live subscribers.
    ///
    /// The commit must not be empty, and every stream name and event type must be non-empty.
    pub async fn commit(&self, commit: Commit) -> Result<Committed, HubError> {
        debug_assert!(!commit.is_empty(), "a commit needs something to write");
        self.ask(|reply| Command::Commit { commit, reply }).await
    }

    /// Every entity row of `kind`, ordered by id.
    pub async fn entities(&self, kind: &str) -> Result<Vec<Entity>, HubError> {
        let kind = kind.to_owned();
        self.ask(|reply| Command::Entities { kind, reply }).await
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
                Command::Commit { commit, reply } => {
                    next = self.commit(&mut inbox, commit, reply);
                }
                Command::Entities { kind, reply } => {
                    let result = self.store.entities(&kind);
                    let _ = reply.send(result.map_err(|error| HubError::Store(Arc::new(error))));
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

    /// Writes one commit, plus any commits already waiting behind it, in one transaction.
    /// Returns the first queued command that couldn't join the batch.
    fn commit(
        &mut self,
        inbox: &mut mpsc::UnboundedReceiver<Command>,
        commit: Commit,
        reply: oneshot::Sender<Result<Committed, HubError>>,
    ) -> Option<Command> {
        let mut events = commit.event_count();
        let mut batch = vec![(commit, reply)];
        let mut next = None;
        while events < MAX_BATCH_EVENTS {
            match inbox.try_recv() {
                Ok(Command::Commit { commit, reply }) => {
                    events += commit.event_count();
                    batch.push((commit, reply));
                }
                Ok(command) => {
                    next = Some(command);
                    break;
                }
                Err(_) => break,
            }
        }

        let appends: Vec<Append<'_>> = batch
            .iter()
            .flat_map(|(commit, _)| &commit.appends)
            .map(|(stream, events)| Append {
                stream: stream.as_str(),
                events,
            })
            .collect();
        let entities: Vec<EntityWrite> = batch
            .iter()
            .flat_map(|(commit, _)| commit.entities.iter().cloned())
            .collect();

        match self.store.commit(&appends, &entities) {
            Ok(results) => {
                if batch.len() > 1 {
                    debug!(commits = batch.len(), events, "group commit");
                }
                let mut results = results.into_iter();
                for (commit, reply) in batch {
                    let appended: Vec<(StreamName, Appended)> = commit
                        .appends
                        .iter()
                        .map(|(stream, _)| {
                            let appended = results.next().expect("one result per append");
                            (stream.clone(), appended)
                        })
                        .collect();
                    for ((stream, events), (_, appended)) in commit.appends.iter().zip(&appended) {
                        self.fan_out(stream, events, appended);
                    }
                    let _ = reply.send(Ok(Committed { appended }));
                }
            }
            Err(error) => {
                error!(%error, "commit failed");
                let error = Arc::new(error);
                for (_, reply) in batch {
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
    async fn a_commit_writes_several_streams_and_entities_and_fans_out() {
        let hub = hub();
        let session = StreamName::session("ses_1");
        let (sender, mut live) = mpsc::channel(16);
        hub.subscribe(session.clone(), sender).await.unwrap();

        let mut commit = Commit::new();
        commit
            .record(&StreamName::index(), event(1))
            .record(&session, event(2))
            .record(&StreamName::index(), event(3))
            .record(&session, event(4))
            .put("session", "ses_1", &json!({"state": "idle"}));
        let committed = hub.commit(commit).await.unwrap();
        assert_eq!(committed.seqs(&StreamName::index()).unwrap().seqs, 1..=2);
        assert_eq!(committed.seqs(&session).unwrap().seqs, 1..=2);

        assert_eq!(live.recv().await.unwrap().seq, 1);
        assert_eq!(live.recv().await.unwrap().seq, 2);
        let rows = hub.entities("session").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].body.get(), r#"{"state":"idle"}"#);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_commits_to_many_streams_stay_gap_free() {
        let hub = hub();
        let tasks: Vec<_> = (0..8)
            .map(|task| {
                let hub = hub.clone();
                tokio::spawn(async move {
                    for i in 0..20 {
                        let mut commit = Commit::new();
                        commit
                            .record(&StreamName::index(), event(task * 100 + i))
                            .record(&StreamName::session(&format!("ses_{task}")), event(i));
                        hub.commit(commit).await.unwrap();
                    }
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let index = hub.read(StreamName::index(), 0, 1000).await.unwrap();
        assert_eq!(
            index.iter().map(|e| e.seq).collect::<Vec<_>>(),
            (1..=160).collect::<Vec<_>>()
        );
        for task in 0..8 {
            let events = hub
                .read(StreamName::session(&format!("ses_{task}")), 0, 100)
                .await
                .unwrap();
            assert_eq!(events.len(), 20);
        }
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
