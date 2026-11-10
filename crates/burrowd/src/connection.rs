//! One client connection: the handshake, request dispatch and event subscriptions.
//!
//! Each connection runs as a few tasks:
//!
//! - **The reader** (this module's main loop) reads one line at a time. It handles the handshake
//!   and `events/subscribe` and `events/unsubscribe` itself, before reading the next line, since
//!   they change the connection. Every other request runs on a task of its own (D-019), so a
//!   slow call doesn't hold up the connection, and answers can come in any order. At most
//!   [`MAX_IN_FLIGHT`] run at once; the reader waits for one to finish before taking more.
//! - **`$/cancel`** sets the cancel flag of a running request; [`crate::methods`] decides what
//!   giving up means for each method.
//! - **The writer** drains a bounded queue of outgoing lines into the pipe, several at a time.
//! - **One forwarder per subscription** replays history from the store in pages, then forwards
//!   live events, dropping any seq it has already sent.
//! - **Terminals.** `terminal/attach`, `terminal/detach`, `terminal/input` and `terminal/resize`
//!   are handled by the reader in order too: keystrokes must reach the program in the order they
//!   were typed. Each attachment has a task that sends its `terminal/update`s
//!   ([`crate::terminals::client`]), one at a time, each once the previous one is written
//!   (D-039). The connection keeps the size each attachment last asked for, for the "latest
//!   wins" resize rule (D-040).
//!
//! A client that stops reading fills the outgoing queue, then its subscriptions' live buffers.
//! When a live buffer overflows, the connection is closed; the client reconnects and resumes from
//! the last seq it processed (D-015).

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use burrow_protocol::methods::{
    EventsSubscribe, EventsUnsubscribe, Initialize, TerminalAttach, TerminalDetach, TerminalInput,
    TerminalResize, TerminalScrollback,
};
use burrow_protocol::notifications::Cancel as CancelNotification;
use burrow_protocol::{
    Empty, ErrorKind, ErrorObject, InitializeParams, InitializeResult, Message, Method,
    Notification, NotificationMethod, PROTOCOL_VERSION, Request, RequestId, Response,
    ServerCapabilities, ServerInfo, StreamName, SubscribeParams, SubscribeResult, TerminalIdParams,
    TerminalInputParams, TerminalResizeParams, TerminalSize,
};
use burrow_transport::{LineReader, push_line};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, error, info, info_span, warn};

use crate::agents::Agents;
use crate::cancel::Cancel;
use crate::hub::{Hub, LiveEvent};
use crate::projects::Projects;
use crate::sessions::Sessions;
use crate::terminals::Terminals;
use crate::terminals::client::{PageRequest, Updates};
use crate::{ids, methods, wire};

/// Lines waiting to be written to one client.
const OUTGOING_QUEUE: usize = 256;
/// Requests one connection may have running at once (D-019).
pub const MAX_IN_FLIGHT: usize = 64;
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
    /// Projects.
    pub projects: Arc<Projects>,
    /// Agents, detection and usage.
    pub agents: Arc<Agents>,
    /// Sessions and the inbox.
    pub sessions: Arc<Sessions>,
    /// Terminals.
    pub terminals: Arc<Terminals>,
}

/// A line for the writer, and whom to tell once it is in the pipe.
#[derive(Debug)]
pub struct Outgoing {
    line: Arc<str>,
    written: Option<oneshot::Sender<()>>,
}

impl Outgoing {
    /// A line whose sender wants to know when it has been written, so that it can send its next
    /// one only then (terminal updates, D-039).
    pub fn tracked(line: Arc<str>, written: oneshot::Sender<()>) -> Self {
        Self {
            line,
            written: Some(written),
        }
    }
}

impl From<Arc<str>> for Outgoing {
    fn from(line: Arc<str>) -> Self {
        Self {
            line,
            written: None,
        }
    }
}

impl From<String> for Outgoing {
    fn from(line: String) -> Self {
        Self::from(Arc::<str>::from(line))
    }
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
        attachments: HashMap::new(),
        in_flight: Arc::default(),
        permits: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
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

/// Writes queued lines to the client, batching whatever is already waiting, and tells the
/// senders that asked once their lines are written.
async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut queue: mpsc::Receiver<Outgoing>,
) -> io::Result<()> {
    let mut buffer = Vec::with_capacity(WRITE_BATCH_BYTES);
    let mut written = Vec::new();
    while let Some(first) = queue.recv().await {
        buffer.clear();
        let mut next = Some(first);
        while let Some(outgoing) = next.take() {
            push_line(&mut buffer, &outgoing.line);
            written.extend(outgoing.written);
            if buffer.len() < WRITE_BATCH_BYTES {
                next = queue.try_recv().ok();
            }
        }
        writer.write_all(&buffer).await?;
        writer.flush().await?;
        for done in written.drain(..) {
            let _ = done.send(());
        }
    }
    Ok(())
}

/// The connection's writer has stopped, so nothing more can be sent.
#[derive(Debug)]
struct Gone;

struct Connection {
    id: String,
    context: Arc<Context>,
    out: mpsc::Sender<Outgoing>,
    close: Arc<Notify>,
    initialized: bool,
    subscriptions: HashMap<StreamName, Subscription>,
    /// Attached terminals, by id.
    attachments: HashMap<String, Attachment>,
    /// Requests running on their own tasks, by id, with the flag `$/cancel` sets.
    in_flight: Arc<Mutex<HashMap<RequestId, Cancel>>>,
    permits: Arc<Semaphore>,
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
                self.handle_notification(&notification);
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
            TerminalAttach::NAME => match request.params_as::<TerminalAttach>() {
                Ok(params) => self.attach(id, params).await,
                Err(error) => self.reply::<TerminalAttach>(id, Err(error)).await,
            },
            TerminalDetach::NAME => {
                let result = match request.params_as::<TerminalDetach>() {
                    Ok(params) => {
                        self.detach(&params.terminal_id).await;
                        Ok(Empty {})
                    }
                    Err(error) => Err(error),
                };
                self.reply::<TerminalDetach>(id, result).await
            }
            TerminalInput::NAME => {
                let result = request
                    .params_as::<TerminalInput>()
                    .and_then(|params| self.terminal_input(params));
                self.reply::<TerminalInput>(id, result).await
            }
            TerminalResize::NAME => {
                let result = request
                    .params_as::<TerminalResize>()
                    .and_then(|params| self.terminal_resize(params));
                self.reply::<TerminalResize>(id, result).await
            }
            // An attached client's pages come from its update task, in order with its updates;
            // anyone else's are relative to the screen now.
            TerminalScrollback::NAME => match request.params_as::<TerminalScrollback>() {
                Ok(params) => {
                    let attachment = self.attachments.get(&params.terminal_id);
                    match attachment.map(|attachment| attachment.pages.send((id, params))) {
                        Some(Ok(())) => Ok(()),
                        Some(Err(_)) | None => self.spawn_request(request).await,
                    }
                }
                Err(error) => self.reply::<TerminalScrollback>(id, Err(error)).await,
            },
            _ => self.spawn_request(request).await,
        }
    }

    /// Runs a request on its own task (D-019). Its answer is queued when it finishes.
    async fn spawn_request(&mut self, request: Request) -> Result<(), Gone> {
        let id = request.id.clone();
        // Waits while MAX_IN_FLIGHT requests run: the client is sending faster than they finish.
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("the semaphore is never closed");
        let cancel = Cancel::new();
        let duplicate = {
            let mut in_flight = lock(&self.in_flight);
            let duplicate = in_flight.contains_key(&id);
            if !duplicate {
                in_flight.insert(id.clone(), cancel.clone());
            }
            duplicate
        };
        if duplicate {
            let error = ErrorObject::invalid_request(format!(
                "request {id} is still running; ids must be unique among pending requests"
            ));
            return self.send(Response::error(Some(id), error).to_line()).await;
        }
        let context = self.context.clone();
        let out = self.out.clone();
        let in_flight = self.in_flight.clone();
        tokio::spawn(
            async move {
                let response = methods::handle(&context, request, cancel).await;
                lock(&in_flight).remove(&id);
                // The connection may be gone; the work is done either way.
                let _ = out.send(response.to_line().into()).await;
                drop(permit);
            }
            .in_current_span(),
        );
        Ok(())
    }

    fn handle_notification(&self, notification: &Notification) {
        if notification.method != CancelNotification::NAME {
            debug!(method = %notification.method, "ignoring an unknown notification");
            return;
        }
        match notification.params_as::<CancelNotification>() {
            Ok(params) => match lock(&self.in_flight).get(&params.id) {
                Some(cancel) => {
                    debug!(id = %params.id, "cancelling a request");
                    cancel.cancel();
                }
                // Finished already, or never sent: nothing to do.
                None => debug!(id = %params.id, "$/cancel for a request that isn't running"),
            },
            Err(error) => debug!(%error, "ignoring a malformed $/cancel"),
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

    /// `terminal/attach`: answers with the whole screen, then starts the updates, so the answer
    /// always comes before the first `terminal/update`.
    async fn attach(&mut self, id: RequestId, params: TerminalIdParams) -> Result<(), Gone> {
        if self.attachments.contains_key(&params.terminal_id) {
            let error = ErrorObject::new(
                ErrorKind::InvalidState,
                format!("already attached to {}; detach first", params.terminal_id),
            );
            return self.reply::<TerminalAttach>(id, Err(error)).await;
        }
        let terminal = match self.context.terminals.get(&params.terminal_id) {
            Ok(terminal) => terminal,
            Err(error) => return self.reply::<TerminalAttach>(id, Err(error)).await,
        };
        let (snapshot, frame, changed) = terminal.attach();
        let closed = snapshot.exit.is_some();
        self.reply::<TerminalAttach>(id, Ok(snapshot)).await?;
        let (pages, requests) = mpsc::unbounded_channel();
        // A closed terminal changes no more: nothing to send.
        let task = (!closed).then(|| {
            let updates = Updates {
                terminal,
                sent: frame,
                changed,
                requests,
                out: self.out.clone(),
            };
            tokio::spawn(updates.run().in_current_span())
        });
        debug!(terminal = params.terminal_id, "attached");
        self.attachments.insert(
            params.terminal_id,
            Attachment {
                task,
                pages,
                wanted: None,
            },
        );
        Ok(())
    }

    /// `terminal/detach`: stops the updates before answering, so none follows the answer.
    /// Detaching a terminal that isn't attached does nothing.
    async fn detach(&mut self, terminal_id: &str) {
        if let Some(mut attachment) = self.attachments.remove(terminal_id) {
            if let Some(task) = attachment.task.take() {
                task.abort();
                let _ = task.await;
            }
            debug!(terminal = terminal_id, "detached");
        }
    }

    /// `terminal/input`. Under "latest wins" (D-040), a client that types gets the size it last
    /// asked for.
    fn terminal_input(&mut self, params: TerminalInputParams) -> Result<Empty, ErrorObject> {
        let terminal = self.context.terminals.get(&params.terminal_id)?;
        if let Some(wanted) = self
            .attachments
            .get(&params.terminal_id)
            .and_then(|attachment| attachment.wanted)
            && terminal.exit().is_none()
        {
            terminal.resize(wanted)?;
        }
        terminal.input(params.data.as_bytes())?;
        Ok(Empty {})
    }

    /// `terminal/resize`: sets the size, and remembers it as what this client wants.
    fn terminal_resize(&mut self, params: TerminalResizeParams) -> Result<Empty, ErrorObject> {
        let terminal = self.context.terminals.get(&params.terminal_id)?;
        let size = TerminalSize {
            cols: params.cols,
            rows: params.rows,
        };
        terminal.resize(size)?;
        if let Some(attachment) = self.attachments.get_mut(&params.terminal_id) {
            attachment.wanted = Some(size);
        }
        Ok(Empty {})
    }
}

/// One attached terminal. Dropping it stops its updates.
struct Attachment {
    /// Sends the updates; `None` for a terminal that was closed when attached.
    task: Option<JoinHandle<()>>,
    /// The client's `terminal/scrollback` requests, for the task to answer.
    pages: mpsc::UnboundedSender<PageRequest>,
    /// The size this client last asked for with `terminal/resize` (D-040).
    wanted: Option<TerminalSize>,
}

impl Drop for Attachment {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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
    out: mpsc::Sender<Outgoing>,
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
        self.out
            .send(line.into())
            .await
            .map_err(|_| Stop::Disconnected)?;
        self.delivered = seq;
        Ok(())
    }
}
