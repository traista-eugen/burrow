//! One client connection: the handshake, request dispatch and event subscriptions.
//!
//! Each connection runs as a few tasks:
//!
//! - **The reader** (this module's main loop) reads one line at a time and handles it to the end
//!   before reading the next, so requests are answered in the order they arrive.
//! - **The writer** drains a bounded queue of outgoing lines into the pipe, several at a time.
//! - **One forwarder per subscription** replays history from the store in pages, then forwards
//!   live events, dropping any seq it has already sent.
//!
//! A client that stops reading fills the outgoing queue, then its subscriptions' live buffers.
//! When a live buffer overflows, the connection is closed; the client reconnects and resumes from
//! the last seq it processed (D-015).

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::Arc;

use burrow_protocol::methods::{DebugAppend, EventsSubscribe, EventsUnsubscribe, Initialize, Ping};
use burrow_protocol::notifications::Cancel;
use burrow_protocol::{
    DebugAppendParams, DebugAppendResult, Empty, ErrorKind, ErrorObject, InitializeParams,
    InitializeResult, Message, Method, Notification, NotificationMethod, PROTOCOL_VERSION, Request,
    RequestId, Response, ServerCapabilities, ServerInfo, StreamName, SubscribeParams,
    SubscribeResult,
};
use burrow_transport::{LineReader, push_line};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, error, info, info_span, warn};

use crate::hub::{Hub, LiveEvent};
use crate::{ids, wire};

/// Lines waiting to be written to one client.
const OUTGOING_QUEUE: usize = 256;
/// Live events one subscription may hold while the client catches up. When it overflows, the
/// connection is closed (D-015).
const LIVE_BUFFER: usize = 1024;
/// Events read from the store per replay page.
const REPLAY_PAGE: usize = 256;
/// The most bytes the writer gathers into one write.
const WRITE_BATCH_BYTES: usize = 64 * 1024;

/// What every connection shares.
#[derive(Debug)]
pub struct Context {
    /// The store thread.
    pub hub: Hub,
    /// `--dev`: development-only methods are enabled.
    pub dev: bool,
}

/// Serves one client until it disconnects or has to be disconnected.
pub async fn serve<S>(stream: S, context: Arc<Context>)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let id = ids::new_id("con");
    let span = info_span!("connection", id = %id);
    run(stream, context, id).instrument(span).await;
}

async fn run<S>(stream: S, context: Arc<Context>, id: String)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    info!("client connected");
    let (reader, writer) = tokio::io::split(stream);
    let (out, queue) = mpsc::channel(OUTGOING_QUEUE);
    let mut writer = tokio::spawn(write_loop(writer, queue).in_current_span());
    let close = Arc::new(Notify::new());

    let mut connection = Connection {
        id,
        context,
        out,
        close: close.clone(),
        initialized: false,
        subscriptions: HashMap::new(),
    };
    let reason = tokio::select! {
        reason = connection.read_loop(LineReader::new(reader)) => reason,
        () = close.notified() => Closed::FellBehind,
        result = &mut writer => Closed::Writer(result.ok().and_then(Result::err)),
    };

    // Stops every subscription, then the writer. Dropping both halves closes the pipe.
    drop(connection);
    writer.abort();
    info!(%reason, "connection closed");
}

/// Why a connection ended.
enum Closed {
    /// The client closed its end.
    ByClient,
    /// Reading failed, or the client sent a line that can't be framed.
    ReadFailed(io::Error),
    /// The writer stopped, with the write error if there was one.
    Writer(Option<io::Error>),
    /// A subscription's live buffer overflowed, or its replay failed.
    FellBehind,
}

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ByClient => f.write_str("closed by the client"),
            Self::ReadFailed(error) => write!(f, "read failed: {error}"),
            Self::Writer(Some(error)) => write!(f, "write failed: {error}"),
            Self::Writer(None) => f.write_str("writer stopped"),
            Self::FellBehind => f.write_str("a subscription fell behind or failed"),
        }
    }
}

/// Writes queued lines to the client, batching whatever is already waiting.
async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut queue: mpsc::Receiver<Arc<str>>,
) -> io::Result<()> {
    let mut buffer = Vec::with_capacity(WRITE_BATCH_BYTES);
    while let Some(line) = queue.recv().await {
        buffer.clear();
        push_line(&mut buffer, &line);
        while buffer.len() < WRITE_BATCH_BYTES {
            match queue.try_recv() {
                Ok(line) => push_line(&mut buffer, &line),
                Err(_) => break,
            }
        }
        writer.write_all(&buffer).await?;
        writer.flush().await?;
    }
    Ok(())
}

/// The connection's writer has stopped, so nothing more can be sent.
#[derive(Debug)]
struct Gone;

struct Connection {
    id: String,
    context: Arc<Context>,
    out: mpsc::Sender<Arc<str>>,
    close: Arc<Notify>,
    initialized: bool,
    subscriptions: HashMap<StreamName, Subscription>,
}

impl Connection {
    async fn read_loop<R: AsyncRead + Unpin>(&mut self, mut lines: LineReader<R>) -> Closed {
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if self.handle_line(&line).await.is_err() {
                        return Closed::Writer(None);
                    }
                }
                Ok(None) => return Closed::ByClient,
                Err(error) => return Closed::ReadFailed(error),
            }
        }
    }

    async fn send(&self, line: String) -> Result<(), Gone> {
        self.out.send(line.into()).await.map_err(|_| Gone)
    }

    async fn reply<M: Method>(
        &self,
        id: RequestId,
        result: Result<M::Result, ErrorObject>,
    ) -> Result<(), Gone> {
        let response = match result {
            Ok(result) => Response::ok::<M>(id, &result),
            Err(error) => {
                debug!(method = M::NAME, %error, "request failed");
                Response::error(Some(id), error)
            }
        };
        self.send(response.to_line()).await
    }

    async fn handle_line(&mut self, line: &str) -> Result<(), Gone> {
        match Message::parse(line) {
            Ok(Message::Request(request)) => self.handle_request(request).await,
            Ok(Message::Notification(notification)) => {
                handle_notification(&notification);
                Ok(())
            }
            Ok(Message::Response(response)) => {
                debug!(id = ?response.id, "ignoring a response: burrowd sends no requests yet");
                Ok(())
            }
            Err(error) => {
                debug!(%error, "malformed message");
                self.send(error.to_response().to_line()).await
            }
        }
    }

    async fn handle_request(&mut self, request: Request) -> Result<(), Gone> {
        let method = request.method.clone();
        let id = request.id.clone();

        if method == Initialize::NAME {
            let result = request
                .params_as::<Initialize>()
                .and_then(|params| self.initialize(params));
            return self.reply::<Initialize>(id, result).await;
        }
        if !self.initialized {
            let error = ErrorObject::new(
                ErrorKind::InvalidState,
                "initialize must be the first request on a connection",
            );
            return self.send(Response::error(Some(id), error).to_line()).await;
        }

        match method.as_str() {
            Ping::NAME => {
                let result = request.params_as::<Ping>().map(|_| Empty {});
                self.reply::<Ping>(id, result).await
            }
            EventsSubscribe::NAME => match request.params_as::<EventsSubscribe>() {
                Ok(params) => self.subscribe(id, params).await,
                Err(error) => self.reply::<EventsSubscribe>(id, Err(error)).await,
            },
            EventsUnsubscribe::NAME => {
                let result = match request.params_as::<EventsUnsubscribe>() {
                    Ok(params) => {
                        self.unsubscribe(&params.stream).await;
                        Ok(Empty {})
                    }
                    Err(error) => Err(error),
                };
                self.reply::<EventsUnsubscribe>(id, result).await
            }
            // Development-only: without --dev it doesn't exist (protocol.md).
            DebugAppend::NAME if self.context.dev => {
                let result = match request.params_as::<DebugAppend>() {
                    Ok(params) => self.debug_append(params).await,
                    Err(error) => Err(error),
                };
                self.reply::<DebugAppend>(id, result).await
            }
            _ => {
                let error = ErrorObject::method_not_found(&method);
                self.send(Response::error(Some(id), error).to_line()).await
            }
        }
    }

    fn initialize(&mut self, params: InitializeParams) -> Result<InitializeResult, ErrorObject> {
        if self.initialized {
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                "this connection is already initialized",
            ));
        }
        if params.protocol_version != PROTOCOL_VERSION {
            warn!(
                client = %params.client.name,
                requested = params.protocol_version,
                "protocol version mismatch"
            );
            return Err(ErrorObject::version_mismatch(
                params.protocol_version,
                vec![PROTOCOL_VERSION],
            ));
        }
        self.initialized = true;
        info!(
            client = %params.client.name,
            version = %params.client.version,
            kind = ?params.client.kind,
            "initialized"
        );
        Ok(InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            server: ServerInfo {
                name: "burrowd".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities: ServerCapabilities::default(),
            connection_id: self.id.clone(),
        })
    }

    /// `events/subscribe`: register for live events first, answer, then replay.
    ///
    /// Registering first means no event can fall between the replay and the live events; the
    /// forwarder drops anything it already sent. Answering before the forwarder starts means the
    /// result always arrives before the subscription's first event.
    async fn subscribe(&mut self, id: RequestId, params: SubscribeParams) -> Result<(), Gone> {
        let SubscribeParams { stream, after } = params;
        if stream.as_str().is_empty() {
            let error = ErrorObject::invalid_params("events/subscribe: stream name is empty");
            return self.reply::<EventsSubscribe>(id, Err(error)).await;
        }
        if self.subscriptions.contains_key(&stream) {
            let error = ErrorObject::new(
                ErrorKind::InvalidState,
                format!("already subscribed to {stream}; unsubscribe first"),
            );
            return self.reply::<EventsSubscribe>(id, Err(error)).await;
        }

        let (sender, live) = mpsc::channel(LIVE_BUFFER);
        let registration = match self.context.hub.subscribe(stream.clone(), sender).await {
            Ok(registration) => registration,
            Err(error) => {
                let error = ErrorObject::internal(error.to_string());
                return self.reply::<EventsSubscribe>(id, Err(error)).await;
            }
        };
        let mut subscription = Subscription {
            stream: stream.clone(),
            hub: self.context.hub.clone(),
            registration: registration.id,
            task: None,
        };

        let head = registration.head;
        let (delivered, reset) = match after {
            // Live events only.
            None => (head, false),
            // The client has seen events this stream doesn't have, for example because the
            // database was replaced. It must reload state; live events after head still follow.
            Some(after) if after > head => (head, true),
            Some(after) => (after, false),
        };
        let result = SubscribeResult {
            stream: stream.clone(),
            head,
            reset,
        };
        // On failure `subscription` drops here and unregisters itself.
        self.reply::<EventsSubscribe>(id, Ok(result)).await?;
        debug!(%stream, ?after, head, reset, "subscribed");

        let forwarder = Forwarder {
            stream: stream.clone(),
            hub: self.context.hub.clone(),
            out: self.out.clone(),
            close: self.close.clone(),
            delivered,
            head,
            live,
        };
        subscription.task = Some(tokio::spawn(forwarder.run().in_current_span()));
        self.subscriptions.insert(stream, subscription);
        Ok(())
    }

    /// `events/unsubscribe`. Unsubscribing from a stream that isn't subscribed does nothing.
    async fn unsubscribe(&mut self, stream: &StreamName) {
        if let Some(subscription) = self.subscriptions.remove(stream) {
            subscription.stop().await;
            debug!(%stream, "unsubscribed");
        }
    }

    async fn debug_append(
        &self,
        params: DebugAppendParams,
    ) -> Result<DebugAppendResult, ErrorObject> {
        wire::check_appendable(&params.stream, &params.event)
            .map_err(ErrorObject::invalid_params)?;
        let record = wire::record(&params.event);
        let appended = self
            .context
            .hub
            .append(params.stream, vec![record])
            .await
            .map_err(|error| ErrorObject::internal(error.to_string()))?;
        Ok(DebugAppendResult {
            seq: *appended.seqs.start(),
        })
    }
}

fn handle_notification(notification: &Notification) {
    if notification.method == Cancel::NAME {
        // Every request is answered before the next line is read, so by the time a $/cancel
        // arrives there is nothing left to cancel.
        debug!("ignoring $/cancel: no request runs long enough to cancel yet");
    } else {
        debug!(method = %notification.method, "ignoring an unknown notification");
    }
}

/// An active subscription. Dropping it stops its forwarder and unregisters it from the store
/// thread.
struct Subscription {
    stream: StreamName,
    hub: Hub,
    registration: u64,
    task: Option<JoinHandle<()>>,
}

impl Subscription {
    /// Stops the forwarder and waits until it has, so that nothing more of this subscription is
    /// queued after this returns.
    async fn stop(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
        self.hub.unsubscribe(self.stream.clone(), self.registration);
    }
}

/// Why a forwarder stopped.
enum Stop {
    /// The connection is closing.
    Disconnected,
    /// The live buffer overflowed and the store thread dropped the subscriber.
    FellBehind,
    /// Reading history failed.
    Store(String),
}

/// Sends one subscription's events: history first, then live events, each seq exactly once.
struct Forwarder {
    stream: StreamName,
    hub: Hub,
    out: mpsc::Sender<Arc<str>>,
    close: Arc<Notify>,
    /// The last seq sent, or the position the subscription started from.
    delivered: u64,
    /// The stream's head when the subscription was registered: history ends here.
    head: u64,
    live: mpsc::Receiver<LiveEvent>,
}

impl Forwarder {
    async fn run(mut self) {
        match self.forward().await {
            Stop::Disconnected => {}
            Stop::FellBehind => {
                warn!(
                    stream = %self.stream,
                    delivered = self.delivered,
                    "client fell behind; closing the connection so it can resume"
                );
                self.close.notify_one();
            }
            Stop::Store(message) => {
                error!(stream = %self.stream, %message, "replay failed; closing the connection");
                self.close.notify_one();
            }
        }
    }

    async fn forward(&mut self) -> Stop {
        if let Err(stop) = self.catch_up(self.head).await {
            return stop;
        }
        loop {
            let Some(event) = self.live.recv().await else {
                return Stop::FellBehind;
            };
            if event.seq <= self.delivered {
                // Already sent while replaying.
                continue;
            }
            if event.seq > self.delivered + 1 {
                // Can't happen while the store thread delivers every event after the head we
                // registered at; filled from the store rather than trusted.
                if let Err(stop) = self.catch_up(event.seq - 1).await {
                    return stop;
                }
                if event.seq <= self.delivered {
                    continue;
                }
            }
            if let Err(stop) = self.deliver(event.seq, event.line).await {
                return stop;
            }
        }
    }

    /// Sends stored events until `target` has been sent. Pages may run past `target` when events
    /// were appended meanwhile; those are sent too, and their live copies are skipped later.
    async fn catch_up(&mut self, target: u64) -> Result<(), Stop> {
        while self.delivered < target {
            if self.live.is_closed() {
                // Dropped by the store thread while replaying; live events would be missed.
                return Err(Stop::FellBehind);
            }
            let page = self
                .hub
                .read(self.stream.clone(), self.delivered, REPLAY_PAGE)
                .await
                .map_err(|error| Stop::Store(error.to_string()))?;
            if page.is_empty() {
                return Err(Stop::Store(format!(
                    "events {}..={target} are missing from the store",
                    self.delivered + 1
                )));
            }
            for event in page {
                if event.seq != self.delivered + 1 {
                    return Err(Stop::Store(format!(
                        "expected seq {}, the store returned {}",
                        self.delivered + 1,
                        event.seq
                    )));
                }
                let line = wire::event_line(&self.stream, event.seq, event.at_ms, &event.payload);
                self.deliver(event.seq, line.into()).await?;
            }
        }
        Ok(())
    }

    async fn deliver(&mut self, seq: u64, line: Arc<str>) -> Result<(), Stop> {
        self.out.send(line).await.map_err(|_| Stop::Disconnected)?;
        self.delivered = seq;
        Ok(())
    }
}
