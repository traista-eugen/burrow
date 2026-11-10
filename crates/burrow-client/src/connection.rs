//! The connection machinery: a reader task, a writer task, and the state they share.
//!
//! - **Calls** put a pending entry in the shared state, then queue their request line for the
//!   writer. The reader matches each response to its entry by id.
//! - **Subscriptions** ride on their `events/subscribe` call: the pending entry carries the
//!   subscription's route, and the reader installs it when the success response arrives, before
//!   it reads the next line. So every event after the response reaches the subscription, and
//!   events of an earlier subscription to the same stream, still in flight, reach nobody.
//! - **Closing.** When the pipe closes, the reader marks the connection closed, fails pending
//!   calls, ends every subscription and stops the writer, so the pipe handle is released at once.
//!   That matters on Windows: a restarted `burrowd` can't create its pipe while an old client
//!   handle is still open.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use burrow_protocol::methods::EventsUnsubscribe;
use burrow_protocol::notifications::EventsEvent;
use burrow_protocol::{
    ErrorObject, Message, Method, Notification, NotificationMethod, Request, RequestId, Response,
    ResponseOutcome, StreamEvent, StreamName, UnsubscribeParams,
};
use burrow_transport::{LineReader, push_line};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use tracing::{debug, warn};

use crate::error::{CallError, ConnectError};
use crate::subscription::Incoming;

/// The most bytes the writer gathers into one write.
const WRITE_BATCH_BYTES: usize = 64 * 1024;

/// Opens the pipe, retrying while every instance is busy, and checks that its server runs as
/// the current user before anything is sent. Must run on a tokio runtime.
#[cfg(windows)]
pub(crate) async fn open_pipe(
    path: String,
    busy_timeout: Duration,
) -> Result<tokio::net::windows::named_pipe::NamedPipeClient, ConnectError> {
    use std::os::windows::io::AsRawHandle;

    use tokio::net::windows::named_pipe::ClientOptions;

    /// All pipe instances are busy.
    const ERROR_PIPE_BUSY: i32 = 231;

    let deadline = Instant::now() + busy_timeout;
    loop {
        match ClientOptions::new().open(&path) {
            // Pipe squatting: another account could have created a pipe with this name first.
            Ok(pipe) => {
                return match burrow_transport::pipe_server_is_current_user(pipe.as_raw_handle()) {
                    Ok(true) => Ok(pipe),
                    Ok(false) => Err(ConnectError::NotYours {
                        pipe: path,
                        reason: "the server runs as another user".into(),
                    }),
                    Err(error) => Err(ConnectError::NotYours {
                        pipe: path,
                        reason: format!("checking the server's user failed: {error}"),
                    }),
                };
            }
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                if Instant::now() >= deadline {
                    return Err(ConnectError::Busy { pipe: path });
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ConnectError::NotRunning { pipe: path });
            }
            Err(error) => return Err(ConnectError::Io(error)),
        }
    }
}

/// Named pipes are Windows-only; elsewhere connecting always fails.
#[cfg(not(windows))]
pub(crate) async fn open_pipe(
    _path: String,
    _busy_timeout: Duration,
) -> Result<tokio::io::DuplexStream, ConnectError> {
    Err(ConnectError::Io(io::Error::new(
        io::ErrorKind::Unsupported,
        "burrow's local transport is Windows-only for now",
    )))
}

/// Where a subscription's events go.
#[derive(Debug)]
pub(crate) struct Route {
    /// Tells this subscription apart from a later one to the same stream.
    pub id: u64,
    events: mpsc::UnboundedSender<StreamEvent>,
}

/// Where a call's answer goes.
#[derive(Debug)]
enum Reply {
    /// To the caller, which waits for it.
    Caller(oneshot::Sender<Response>),
    /// Through the notifications, in order with them ([`crate::Client::send_in_order`]).
    InOrder,
}

#[derive(Debug)]
struct Pending {
    reply: Reply,
    /// Installed when the response is a success: the route of an `events/subscribe` call.
    route: Option<(StreamName, Route)>,
}

#[derive(Debug, Default)]
struct State {
    closed: bool,
    pending: HashMap<RequestId, Pending>,
    routes: HashMap<StreamName, Route>,
    notifications: Option<mpsc::UnboundedSender<Incoming>>,
}

/// What the reader, the writer and every `Client` clone share.
#[derive(Debug)]
struct Shared {
    lines: mpsc::UnboundedSender<String>,
    next_request: AtomicI64,
    next_route: AtomicU64,
    state: Mutex<State>,
    closed: watch::Sender<bool>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn next_id(&self) -> RequestId {
        RequestId::Number(self.next_request.fetch_add(1, Ordering::Relaxed))
    }

    /// Marks the connection closed: pending calls fail, subscriptions end. Idempotent.
    fn close(&self) {
        let mut state = self.state();
        if state.closed {
            return;
        }
        state.closed = true;
        state.pending.clear();
        state.routes.clear();
        state.notifications = None;
        drop(state);
        self.closed.send_replace(true);
    }

    /// Sends `events/unsubscribe` without waiting for the answer.
    fn send_unsubscribe(&self, stream: StreamName) {
        let request =
            Request::typed::<EventsUnsubscribe>(self.next_id(), &UnsubscribeParams { stream });
        let _ = self.lines.send(request.to_line());
    }

    fn dispatch(&self, line: &str) {
        match Message::parse(line) {
            Ok(Message::Response(response)) => self.complete(response),
            Ok(Message::Notification(notification)) if notification.method == EventsEvent::NAME => {
                self.route_event(&notification);
            }
            Ok(Message::Notification(notification)) => self.deliver_notification(notification),
            Ok(Message::Request(request)) => {
                // burrowd sends no requests yet; answer so it isn't left waiting.
                let error = ErrorObject::method_not_found(&request.method);
                let _ = self
                    .lines
                    .send(Response::error(Some(request.id), error).to_line());
            }
            Err(error) => warn!(%error, "burrowd sent a malformed message"),
        }
    }

    fn complete(&self, response: Response) {
        let Some(id) = response.id.clone() else {
            warn!(outcome = ?response.outcome, "burrowd couldn't read a message we sent");
            return;
        };
        let mut state = self.state();
        let Some(pending) = state.pending.remove(&id) else {
            debug!(%id, "a response nobody waits for");
            return;
        };
        if let (Some((stream, route)), ResponseOutcome::Result(_)) =
            (pending.route, &response.outcome)
        {
            state.routes.insert(stream, route);
        }
        match pending.reply {
            Reply::Caller(reply) => {
                drop(state);
                let _ = reply.send(response);
            }
            // Queued under the lock, so it keeps its place among the notifications.
            Reply::InOrder => Self::queue(&mut state, Incoming::Answer(response)),
        }
    }

    fn route_event(&self, notification: &Notification) {
        let event = match notification.params_as::<EventsEvent>() {
            Ok(event) => event,
            Err(error) => {
                // Skipping it would leave a silent gap, so end that stream's subscription.
                let stream = notification
                    .params
                    .as_ref()
                    .and_then(|params| params.get("stream"))
                    .and_then(|stream| stream.as_str())
                    .map(StreamName::new);
                warn!(%error, ?stream, "can't decode an event; ending its subscription");
                if let Some(stream) = stream
                    && self.state().routes.remove(&stream).is_some()
                {
                    self.send_unsubscribe(stream);
                }
                return;
            }
        };
        let mut state = self.state();
        let Some(route) = state.routes.get(&event.stream) else {
            debug!(stream = %event.stream, seq = event.seq, "an event for no subscription");
            return;
        };
        if let Err(mpsc::error::SendError(event)) = route.events.send(event) {
            // The Subscription is gone without unsubscribing, for example because its subscribe
            // call was abandoned before the answer arrived.
            state.routes.remove(&event.stream);
            drop(state);
            self.send_unsubscribe(event.stream);
        }
    }

    fn deliver_notification(&self, notification: Notification) {
        Self::queue(&mut self.state(), Incoming::Notification(notification));
    }

    /// Hands a message to the notifications' consumer, if any.
    fn queue(state: &mut State, message: Incoming) {
        if let Some(sender) = &state.notifications
            && sender.send(message).is_err()
        {
            state.notifications = None;
        }
    }
}

/// One open connection: the shared state plus the two I/O tasks, stopped when this is dropped.
#[derive(Debug)]
pub(crate) struct Connection {
    shared: Arc<Shared>,
    reader: AbortHandle,
    writer: AbortHandle,
}

impl Connection {
    /// Starts the reader and writer tasks for `stream` on `runtime`.
    pub fn start<S>(stream: S, runtime: &Handle) -> Self
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (reader, writer) = tokio::io::split(stream);
        let (lines, queue) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            lines,
            next_request: AtomicI64::new(0),
            next_route: AtomicU64::new(1),
            state: Mutex::new(State::default()),
            closed: watch::Sender::new(false),
        });
        let writer = runtime.spawn(write_loop(writer, queue, shared.clone()));
        let reader = runtime.spawn(read_loop(
            LineReader::new(reader),
            shared.clone(),
            writer.abort_handle(),
        ));
        Self {
            shared,
            reader: reader.abort_handle(),
            writer: writer.abort_handle(),
        }
    }

    /// Calls `M`. `route` is installed if the call succeeds.
    pub async fn call<M: Method>(
        &self,
        params: &M::Params,
        route: Option<(StreamName, Route)>,
    ) -> Result<M::Result, CallError> {
        let (_, answer) = self.send_request::<M>(params, route)?;
        Self::finish::<M>(answer).await
    }

    /// Sends a request and returns its id and the receiver of its response.
    pub fn send_request<M: Method>(
        &self,
        params: &M::Params,
        route: Option<(StreamName, Route)>,
    ) -> Result<(RequestId, oneshot::Receiver<Response>), CallError> {
        let (reply, answer) = oneshot::channel();
        let line = |id| Request::typed::<M>(id, params).to_line();
        let id = self.send(line, Reply::Caller(reply), route)?;
        Ok((id, answer))
    }

    /// Sends a request whose response is delivered through the notifications, in order.
    pub fn send_in_order<M: Method>(&self, params: &M::Params) -> Result<RequestId, CallError> {
        self.send(
            |id| Request::typed::<M>(id, params).to_line(),
            Reply::InOrder,
            None,
        )
    }

    /// Registers where the answer goes, then queues the request's `line` for the writer.
    fn send(
        &self,
        line: impl FnOnce(RequestId) -> String,
        reply: Reply,
        route: Option<(StreamName, Route)>,
    ) -> Result<RequestId, CallError> {
        let id = self.shared.next_id();
        let line = line(id.clone());
        {
            let mut state = self.shared.state();
            if state.closed {
                return Err(CallError::Disconnected);
            }
            state.pending.insert(id.clone(), Pending { reply, route });
        }
        if self.shared.lines.send(line).is_err() {
            self.shared.state().pending.remove(&id);
            return Err(CallError::Disconnected);
        }
        Ok(id)
    }

    /// Waits for a response from [`Connection::send_request`].
    pub async fn finish<M: Method>(
        answer: oneshot::Receiver<Response>,
    ) -> Result<M::Result, CallError> {
        let response = answer.await.map_err(|_| CallError::Disconnected)?;
        response.result_as::<M>().map_err(CallError::Rpc)
    }

    pub fn notify<N: NotificationMethod>(&self, params: &N::Params) -> Result<(), CallError> {
        if self.is_closed() {
            return Err(CallError::Disconnected);
        }
        self.shared
            .lines
            .send(Notification::typed::<N>(params).to_line())
            .map_err(|_| CallError::Disconnected)
    }

    /// A new route and the receiving end of its events.
    pub fn new_route(&self) -> (Route, mpsc::UnboundedReceiver<StreamEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let id = self.shared.next_route.fetch_add(1, Ordering::Relaxed);
        (Route { id, events }, receiver)
    }

    /// Removes route `id` of `stream`, if it is still the active one. Returns whether it was.
    pub fn remove_route(&self, stream: &StreamName, id: u64) -> bool {
        let mut state = self.shared.state();
        if state.routes.get(stream).is_some_and(|route| route.id == id) {
            state.routes.remove(stream);
            true
        } else {
            false
        }
    }

    /// Ends a subscription without waiting: removes its route and tells burrowd.
    pub fn detach(&self, stream: &StreamName, id: u64) {
        if self.remove_route(stream, id) && !self.is_closed() {
            self.shared.send_unsubscribe(stream.clone());
        }
    }

    pub fn notifications(&self) -> mpsc::UnboundedReceiver<Incoming> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut state = self.shared.state();
        if !state.closed {
            state.notifications = Some(sender);
        }
        receiver
    }

    pub fn is_closed(&self) -> bool {
        *self.shared.closed.borrow()
    }

    pub async fn closed(&self) {
        let mut closed = self.shared.closed.subscribe();
        let _ = closed.wait_for(|closed| *closed).await;
    }

    pub fn close(&self) {
        self.shared.close();
        self.reader.abort();
        self.writer.abort();
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

async fn read_loop<R: AsyncRead + Unpin>(
    mut lines: LineReader<R>,
    shared: Arc<Shared>,
    writer: AbortHandle,
) {
    let mut closed = shared.closed.subscribe();
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => shared.dispatch(&line),
                Ok(None) => {
                    debug!("burrowd closed the connection");
                    break;
                }
                Err(error) => {
                    debug!(%error, "reading from burrowd failed");
                    break;
                }
            },
            _ = closed.wait_for(|closed| *closed) => break,
        }
    }
    shared.close();
    // Stop the writer too, so both halves of the pipe are dropped and its handle closes now.
    writer.abort();
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut queue: mpsc::UnboundedReceiver<String>,
    shared: Arc<Shared>,
) {
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
        let written = async {
            writer.write_all(&buffer).await?;
            writer.flush().await
        };
        if let Err(error) = written.await {
            debug!(%error, "writing to burrowd failed");
            shared.close();
            return;
        }
    }
}
