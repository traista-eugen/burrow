//! A client for `burrowd`: connect to its named pipe, shake hands, make typed calls, receive
//! notifications and follow event streams. Used by `burrowctl` and, from M3, the desktop app.
//!
//! ```no_run
//! use burrow_client::{Client, ConnectOptions};
//! use burrow_protocol::{ClientInfo, ClientKind, StreamName, methods::Ping, Empty};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let info = ClientInfo { name: "example".into(), version: "0.1.0".into(), kind: ClientKind::Cli };
//! let client = Client::connect(ConnectOptions::new(info)).await?;
//! client.call::<Ping>(&Empty {}).await?;
//!
//! let mut index = client.subscribe(StreamName::index(), Some(0)).await?;
//! while let Some(event) = index.next().await {
//!     println!("{} #{}: {}", event.stream, event.seq, event.event.type_name());
//! }
//! // The connection closed. After reconnecting, carry on where this left off:
//! let resume = index.resume_params();
//! # let _ = resume;
//! # Ok(())
//! # }
//! ```
//!
//! # Runtimes (D-016)
//!
//! The pipe I/O runs on tokio, as two tasks per connection (a reader and a writer). Everything
//! the caller touches is runtime-agnostic: calls await `tokio::sync` channels, which work on any
//! executor, and [`Subscription`] and [`Notifications`] are plain [`Stream`]s.
//!
//! - Code already on tokio (`burrowctl`, tests) just calls [`Client::connect`]; the I/O tasks run
//!   on the current runtime.
//! - Code on another executor, such as the GPUI app, starts a small runtime with
//!   [`io_runtime`], keeps it alive, and passes its handle with [`ConnectOptions::runtime`]. The
//!   futures it gets back can be awaited on GPUI's own executor.
//!
//! # Delivery
//!
//! `burrowd` sends each subscription's events in order, at most once, with no gaps. A
//! [`Subscription`] remembers the last seq it yielded; after a reconnect,
//! [`Subscription::resume_params`] subscribes again from exactly there.
//!
//! Answers, events and notifications each have their own channel, so a caller can see a call's
//! answer before or after a notification `burrowd` sent ahead of it. Where that order matters,
//! as for terminals, [`Client::send_in_order`] delivers the answer through [`Notifications`]
//! instead, in the order it arrived.
//!
//! [`Stream`]: futures_core::Stream

mod connection;
mod error;
mod subscription;

use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use burrow_protocol::methods::{EventsSubscribe, Initialize};
use burrow_protocol::notifications::Cancel;
use burrow_protocol::{
    CancelParams, ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, Method,
    NotificationMethod, PROTOCOL_VERSION, RequestId, Response, StreamName, SubscribeParams,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::connection::Connection;
pub use crate::error::{CallError, ConnectError};
pub use crate::subscription::{Incoming, Notifications, Subscription};

/// How long [`Client::connect`] waits while every pipe instance is busy, by default.
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// How to connect: where, as whom, and on which runtime.
#[derive(Clone, Debug)]
pub struct ConnectOptions {
    pipe: Option<String>,
    client: ClientInfo,
    capabilities: ClientCapabilities,
    runtime: Option<Handle>,
    busy_timeout: Duration,
}

impl ConnectOptions {
    /// Connect to the current user's `burrowd` (`\\.\pipe\burrow-<user SID>`) as `client`.
    pub fn new(client: ClientInfo) -> Self {
        Self {
            pipe: None,
            client,
            capabilities: ClientCapabilities::default(),
            runtime: None,
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        }
    }

    /// Connect to another pipe: a bare name such as `burrow-dev`, or a full path such as
    /// `\\.\pipe\burrow-dev`.
    pub fn pipe(mut self, name: impl Into<String>) -> Self {
        self.pipe = Some(name.into());
        self
    }

    /// The capabilities to announce in `initialize`. None by default.
    pub fn capabilities(mut self, capabilities: ClientCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Run the connection's I/O on this tokio runtime instead of the current one. Needed when
    /// connecting from outside tokio, for example from GPUI's executor; see [`io_runtime`].
    pub fn runtime(mut self, handle: Handle) -> Self {
        self.runtime = Some(handle);
        self
    }

    /// How long to keep retrying while every pipe instance is busy.
    pub fn busy_timeout(mut self, timeout: Duration) -> Self {
        self.busy_timeout = timeout;
        self
    }
}

/// A connection to `burrowd`. Cheap to clone; every clone shares the connection, which closes
/// when the last clone and the last [`Subscription`] are dropped, or on [`Client::close`].
#[derive(Clone, Debug)]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    connection: Connection,
    server: InitializeResult,
    pipe: String,
}

impl Client {
    /// Connects to `burrowd` and performs the `initialize` handshake.
    ///
    /// Fails with [`ConnectError::NotRunning`] when nothing serves the pipe, so a caller can start
    /// `burrowd` and try again, and with [`ConnectError::Rejected`] when `burrowd` refuses the
    /// handshake, for example with `versionMismatch`.
    pub async fn connect(options: ConnectOptions) -> Result<Self, ConnectError> {
        let runtime = match &options.runtime {
            Some(handle) => handle.clone(),
            None => Handle::try_current().map_err(|_| ConnectError::NoRuntime)?,
        };
        let pipe = match &options.pipe {
            Some(name) => burrow_transport::pipe_path(name),
            None => burrow_transport::default_pipe_name()?,
        };
        let stream = runtime
            .spawn(connection::open_pipe(pipe.clone(), options.busy_timeout))
            .await
            .map_err(|error| ConnectError::Io(io::Error::other(error)))??;
        Self::handshake(stream, runtime, pipe, options).await
    }

    /// Runs the handshake over an already open byte stream. `label` names it in errors.
    async fn handshake<S>(
        stream: S,
        runtime: Handle,
        label: String,
        options: ConnectOptions,
    ) -> Result<Self, ConnectError>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let connection = Connection::start(stream, &runtime);
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION,
            client: options.client,
            capabilities: options.capabilities,
        };
        let server = match connection.call::<Initialize>(&params, None).await {
            Ok(server) => server,
            Err(CallError::Rpc(error)) => return Err(ConnectError::Rejected(error)),
            Err(CallError::Disconnected) => return Err(ConnectError::Disconnected),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                connection,
                server,
                pipe: label,
            }),
        })
    }

    /// What `burrowd` said about itself in the handshake.
    pub fn server(&self) -> &InitializeResult {
        &self.inner.server
    }

    /// This connection's id (`con_…`).
    pub fn connection_id(&self) -> &str {
        &self.inner.server.connection_id
    }

    /// The full path of the pipe this client is connected to.
    pub fn pipe(&self) -> &str {
        &self.inner.pipe
    }

    /// Calls method `M` and waits for its result.
    ///
    /// For `events/subscribe`, use [`Client::subscribe`] instead: a subscription made with `call`
    /// has nowhere to deliver its events, so they are dropped.
    pub async fn call<M: Method>(&self, params: &M::Params) -> Result<M::Result, CallError> {
        self.inner.connection.call::<M>(params, None).await
    }

    /// Starts a call to `M` without waiting for it, so it can be cancelled: see [`PendingCall`].
    pub fn start_call<M: Method>(&self, params: &M::Params) -> Result<PendingCall<M>, CallError> {
        let (id, answer) = self.inner.connection.send_request::<M>(params, None)?;
        Ok(PendingCall {
            client: self.clone(),
            id,
            answer,
            method: PhantomData,
        })
    }

    /// Sends a request to `M` whose answer comes through this connection's [`Notifications`], as
    /// [`Incoming::Answer`] with the returned id, in the order `burrowd` sent it among the
    /// notifications. Returns at once; the request goes out after any sent before it.
    ///
    /// For answers that must be applied in order with notifications: a `terminal/attach`
    /// snapshot before the first `terminal/update` that follows it, or a `terminal/scrollback`
    /// page after exactly the updates `burrowd` sent before it. A plain [`Client::call`] answers
    /// on its own channel, so its result and the notifications around it can be seen in either
    /// order.
    ///
    /// Someone must be reading [`Client::notifications`]; the answer is dropped otherwise, and
    /// when the connection closes first.
    pub fn send_in_order<M: Method>(&self, params: &M::Params) -> Result<RequestId, CallError> {
        self.inner.connection.send_in_order::<M>(params)
    }

    /// Sends notification `N`. It gets no response.
    pub fn notify<N: NotificationMethod>(&self, params: &N::Params) -> Result<(), CallError> {
        self.inner.connection.notify::<N>(params)
    }

    /// Subscribes to `stream`: events after seq `after`, then live events. `None` means live
    /// events only; `Some(0)` means the whole history.
    pub async fn subscribe(
        &self,
        stream: impl Into<StreamName>,
        after: Option<u64>,
    ) -> Result<Subscription, CallError> {
        self.subscribe_with(SubscribeParams {
            stream: stream.into(),
            after,
        })
        .await
    }

    /// Subscribes with the given params, for example [`Subscription::resume_params`] of a
    /// subscription from an earlier connection.
    ///
    /// A connection has at most one subscription per stream; subscribing again while one is
    /// active fails with `invalidState`.
    pub async fn subscribe_with(&self, params: SubscribeParams) -> Result<Subscription, CallError> {
        let (route, events) = self.inner.connection.new_route();
        let route_id = route.id;
        let result = self
            .inner
            .connection
            .call::<EventsSubscribe>(&params, Some((params.stream.clone(), route)))
            .await?;
        Ok(Subscription::new(
            self.clone(),
            route_id,
            &params,
            &result,
            events,
        ))
    }

    /// Incoming notifications other than `events/event`, which go to subscriptions, and the
    /// answers of requests sent with [`Client::send_in_order`].
    ///
    /// There is one consumer at a time: calling this again ends the previous stream.
    /// Notifications that arrive while nobody listens are dropped.
    pub fn notifications(&self) -> Notifications {
        Notifications::new(self.inner.connection.notifications())
    }

    /// Whether the connection has closed.
    pub fn is_closed(&self) -> bool {
        self.inner.connection.is_closed()
    }

    /// Resolves once the connection has closed, for example because `burrowd` exited.
    pub async fn closed(&self) {
        self.inner.connection.closed().await;
    }

    /// Closes the connection now. Pending calls fail with [`CallError::Disconnected`] and every
    /// subscription ends.
    pub fn close(&self) {
        self.inner.connection.close();
    }

    fn connection(&self) -> &Connection {
        &self.inner.connection
    }
}

/// A call in flight, from [`Client::start_call`]: await [`PendingCall::result`] for its answer,
/// or ask `burrowd` to give up with [`PendingCall::cancel`].
pub struct PendingCall<M: Method> {
    client: Client,
    id: RequestId,
    answer: oneshot::Receiver<Response>,
    method: PhantomData<fn() -> M>,
}

impl<M: Method> fmt::Debug for PendingCall<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingCall")
            .field("method", &M::NAME)
            .field("id", &self.id)
            .finish()
    }
}

impl<M: Method> PendingCall<M> {
    /// The request's id.
    pub fn id(&self) -> &RequestId {
        &self.id
    }

    /// Sends `$/cancel` for this call. `burrowd` answers it with the error `cancelled` if it
    /// gives up, or with the normal result if the work finishes anyway; await
    /// [`PendingCall::result`] either way.
    pub fn cancel(&self) -> Result<(), CallError> {
        self.client.notify::<Cancel>(&CancelParams {
            id: self.id.clone(),
        })
    }

    /// Waits for the answer.
    pub async fn result(self) -> Result<M::Result, CallError> {
        Connection::finish::<M>(self.answer).await
    }
}

/// Starts a small tokio runtime for client I/O: one worker thread, I/O and timers enabled.
///
/// For callers that don't run tokio themselves, such as the GPUI app. Keep the runtime alive for
/// as long as any connection made on it, and pass its handle with [`ConnectOptions::runtime`].
pub fn io_runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("burrow-client-io")
        .enable_io()
        .enable_time()
        .build()
}

#[cfg(test)]
mod tests;
