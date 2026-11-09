//! The app's link to `burrowd`: the connection, calls, and streams that survive reconnects.
//!
//! - [`Backend`] is a GPUI global. It owns the tokio runtime `burrow-client` does its I/O on
//!   (GPUI doesn't run tokio, D-016) and publishes the current [`Client`], if connected.
//! - [`run_connection`] keeps a connection up: it connects, starts `burrowd` when nothing serves
//!   the pipe ([`daemon`], D-029), backs off between attempts, and reports a
//!   [`ConnectionStatus`].
//! - [`follow`] keeps an entity subscribed to one stream across reconnects, resuming after the
//!   last seq the entity has.
//! - [`Backend::call`] makes one call on the current connection.
//!
//! Everything the client returns is executor-agnostic, so it is awaited on GPUI's executor.

pub mod daemon;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use burrow_client::{CallError, Client, ConnectError, ConnectOptions};
use burrow_protocol::{ClientInfo, ClientKind, Method, StreamEvent, StreamName};
use futures::FutureExt as _;
use gpui::{App, AppContext as _, AsyncApp, Context, Global, Task, WeakEntity};
use tokio::sync::watch;
use tracing::{info, warn};

/// How long to wait for `burrowd` after starting it before starting it again.
const DAEMON_START_GRACE: Duration = Duration::from_secs(10);
/// How often to retry while a freshly started `burrowd` comes up.
const DAEMON_POLL: Duration = Duration::from_millis(50);
/// The most events handed to an entity in one update.
const MAX_BATCH: usize = 512;

/// Where to connect, and whether the app may start `burrowd` there.
#[derive(Clone, Debug)]
pub struct BackendConfig {
    /// The pipe, or `None` for the current user's default pipe.
    pub pipe: Option<String>,
    /// How to start `burrowd` when nothing serves the pipe; `None` never starts it.
    pub daemon: Option<DaemonLaunch>,
}

/// How to start `burrowd`.
#[derive(Clone, Debug, PartialEq)]
pub struct DaemonLaunch {
    /// `burrowd.exe`.
    pub program: PathBuf,
    /// Its arguments, `--pipe` included when the pipe isn't the default.
    pub args: Vec<String>,
}

/// The connection's state, for the status line.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionStatus {
    /// Trying to connect; `attempt` counts failures since the last success.
    Connecting { attempt: u32 },
    /// `burrowd` wasn't running; the app started it and waits for it.
    StartingDaemon,
    /// Connected.
    Connected {
        /// `burrowd`'s version.
        server_version: String,
    },
    /// Nothing serves the pipe, and the app won't start `burrowd` there (see [`BackendConfig`]),
    /// or starting it failed. Retrying.
    NotRunning { detail: String },
    /// Connecting failed for another reason, for example a protocol version mismatch. Retrying.
    Failed { detail: String },
}

impl ConnectionStatus {
    /// Whether calls can be made.
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

/// The app's handle to `burrowd`. Cheap to clone.
#[derive(Clone)]
pub struct Backend {
    inner: Arc<Inner>,
}

struct Inner {
    config: BackendConfig,
    runtime: tokio::runtime::Runtime,
    client: watch::Sender<Option<Client>>,
}

impl Global for Backend {}

impl Backend {
    /// Starts the I/O runtime. Nothing connects until [`run_connection`] runs.
    pub fn new(config: BackendConfig) -> std::io::Result<Self> {
        let runtime = burrow_client::io_runtime()?;
        let (client, _) = watch::channel(None);
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                runtime,
                client,
            }),
        })
    }

    /// The global backend. Panics if [`Backend::new`] wasn't installed with `cx.set_global`.
    pub fn global(cx: &App) -> &Backend {
        cx.global::<Backend>()
    }

    /// Where it connects.
    pub fn config(&self) -> &BackendConfig {
        &self.inner.config
    }

    /// The pipe's full path, for display.
    pub fn pipe_path(&self) -> String {
        match &self.inner.config.pipe {
            Some(name) => burrow_transport::pipe_path(name),
            None => burrow_transport::default_pipe_name()
                .unwrap_or_else(|_| r"\\.\pipe\burrow-<user>".to_owned()),
        }
    }

    /// The current connection, if there is one and it is open.
    pub fn client(&self) -> Option<Client> {
        self.inner
            .client
            .borrow()
            .clone()
            .filter(|client| !client.is_closed())
    }

    /// Waits until connected, then returns the connection.
    pub async fn connected(&self) -> Client {
        let mut receiver = self.inner.client.subscribe();
        loop {
            if let Some(client) = receiver
                .borrow_and_update()
                .clone()
                .filter(|client| !client.is_closed())
            {
                return client;
            }
            if receiver.changed().await.is_err() {
                // The backend is gone: the app is quitting.
                std::future::pending::<()>().await;
            }
        }
    }

    /// Calls method `M` on the current connection. Fails with [`CallError::Disconnected`] at once
    /// when there is none.
    pub fn call<M>(&self, params: M::Params, cx: &App) -> Task<Result<M::Result, CallError>>
    where
        M: Method,
        M::Params: Send + Sync + 'static,
        M::Result: Send + 'static,
    {
        let Some(client) = self.client() else {
            return Task::ready(Err(CallError::Disconnected));
        };
        cx.background_spawn(async move { client.call::<M>(&params).await })
    }

    fn connect_options(&self) -> ConnectOptions {
        let info = ClientInfo {
            name: "burrow-desktop".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            kind: ClientKind::Desktop,
        };
        let mut options = ConnectOptions::new(info).runtime(self.inner.runtime.handle().clone());
        if let Some(pipe) = &self.inner.config.pipe {
            options = options.pipe(pipe.clone());
        }
        options
    }

    fn publish(&self, client: Option<Client>) {
        self.inner.client.send_replace(client);
    }
}

/// How long to wait before attempt number `attempt` (counted from 1) after a failure.
pub fn backoff(attempt: u32) -> Duration {
    let millis = 250u64.saturating_mul(1 << attempt.saturating_sub(1).min(5));
    Duration::from_millis(millis.min(5_000))
}

/// Keeps the connection up for as long as `entity` lives, reporting each change of status to
/// `on_status`. Call it once, from the entity that shows the status.
pub async fn run_connection<T: 'static>(
    backend: Backend,
    entity: WeakEntity<T>,
    mut on_status: impl FnMut(&mut T, ConnectionStatus, Option<&Client>, &mut Context<T>),
    cx: &mut AsyncApp,
) {
    let mut attempt = 0u32;
    let mut daemon_started: Option<Instant> = None;
    let mut report = |status: ConnectionStatus, client: Option<&Client>, cx: &mut AsyncApp| {
        entity
            .update(cx, |this, cx| on_status(this, status, client, cx))
            .is_ok()
    };
    loop {
        let waiting_for_daemon = daemon_started.is_some_and(|at| at.elapsed() < DAEMON_START_GRACE);
        let status = if waiting_for_daemon {
            ConnectionStatus::StartingDaemon
        } else {
            ConnectionStatus::Connecting { attempt }
        };
        if attempt == 0 || !waiting_for_daemon {
            // Don't flicker the status line for every poll while burrowd starts.
            if !report(status, None, cx) {
                return;
            }
        }
        let delay = match Client::connect(backend.connect_options()).await {
            Ok(client) => {
                info!(pipe = client.pipe(), server = %client.server().server.version, "connected");
                attempt = 0;
                daemon_started = None;
                backend.publish(Some(client.clone()));
                let status = ConnectionStatus::Connected {
                    server_version: client.server().server.version.clone(),
                };
                if !report(status, Some(&client), cx) {
                    return;
                }
                client.closed().await;
                backend.publish(None);
                warn!("disconnected from burrowd");
                if !report(ConnectionStatus::Connecting { attempt: 0 }, None, cx) {
                    return;
                }
                // Reconnect at once: a slow client that was cut off (D-015) resumes right away,
                // and a burrowd that restarted is usually back within a moment.
                Duration::from_millis(50)
            }
            Err(ConnectError::NotRunning { pipe }) => {
                attempt += 1;
                if waiting_for_daemon {
                    DAEMON_POLL
                } else if let Some(launch) = &backend.config().daemon {
                    match daemon::spawn(launch) {
                        Ok(pid) => {
                            info!(pid, program = %launch.program.display(), "started burrowd");
                            daemon_started = Some(Instant::now());
                            if !report(ConnectionStatus::StartingDaemon, None, cx) {
                                return;
                            }
                            DAEMON_POLL
                        }
                        Err(error) => {
                            warn!(%error, "couldn't start burrowd");
                            let detail =
                                format!("couldn't start {}: {error}", launch.program.display());
                            if !report(ConnectionStatus::NotRunning { detail }, None, cx) {
                                return;
                            }
                            backoff(attempt)
                        }
                    }
                } else {
                    let detail = format!("burrowd isn't running on {pipe}");
                    if !report(ConnectionStatus::NotRunning { detail }, None, cx) {
                        return;
                    }
                    backoff(attempt)
                }
            }
            Err(error @ (ConnectError::Rejected(_) | ConnectError::NotYours { .. })) => {
                attempt += 1;
                warn!(%error, "burrowd refused the connection");
                let detail = error.to_string();
                if !report(ConnectionStatus::Failed { detail }, None, cx) {
                    return;
                }
                Duration::from_secs(10)
            }
            Err(error) => {
                attempt += 1;
                warn!(%error, "couldn't connect to burrowd");
                if waiting_for_daemon {
                    DAEMON_POLL
                } else {
                    let detail = error.to_string();
                    if !report(ConnectionStatus::Failed { detail }, None, cx) {
                        return;
                    }
                    backoff(attempt)
                }
            }
        };
        cx.background_executor().timer(delay).await;
    }
}

/// What [`follow`] hands the entity.
#[derive(Debug)]
pub enum FollowEvent {
    /// A (re)subscription started. `reset` means the history after the requested seq isn't
    /// available: reload, then follow live events after `head`.
    Subscribed { head: u64, reset: bool },
    /// Events, in seq order, batched when several are waiting.
    Events(Vec<StreamEvent>),
}

/// Keeps `entity` subscribed to `stream` while it lives, across reconnects.
///
/// Before each subscription, `position` says where to start: `Some(seq)` replays everything
/// after `seq` (`Some(0)` is the whole history), `None` means live events only. `handle`
/// receives the subscription's start and its events.
pub async fn follow<T: 'static>(
    backend: Backend,
    entity: WeakEntity<T>,
    stream: StreamName,
    position: impl Fn(&T) -> Option<u64>,
    mut handle: impl FnMut(&mut T, FollowEvent, &mut Context<T>),
    cx: &mut AsyncApp,
) {
    loop {
        let client = backend.connected().await;
        let Ok(after) = entity.read_with(cx, |this, _| position(this)) else {
            return;
        };
        let mut subscription = match client.subscribe(stream.clone(), after).await {
            Ok(subscription) => subscription,
            Err(CallError::Disconnected) => continue,
            Err(error) => {
                warn!(%stream, %error, "couldn't subscribe");
                cx.background_executor().timer(Duration::from_secs(1)).await;
                continue;
            }
        };
        let started = FollowEvent::Subscribed {
            head: subscription.head(),
            reset: subscription.reset(),
        };
        if entity
            .update(cx, |this, cx| handle(this, started, cx))
            .is_err()
        {
            return;
        }
        while let Some(first) = subscription.next().await {
            let mut batch = vec![first];
            while batch.len() < MAX_BATCH {
                match subscription.next().now_or_never() {
                    Some(Some(event)) => batch.push(event),
                    _ => break,
                }
            }
            if entity
                .update(cx, |this, cx| handle(this, FollowEvent::Events(batch), cx))
                .is_err()
            {
                return;
            }
        }
        if !client.is_closed() {
            // The subscription ended on an open connection (an event this version can't decode):
            // don't spin.
            warn!(%stream, "subscription ended; subscribing again");
            cx.background_executor().timer(Duration::from_secs(1)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_five_seconds() {
        let delays: Vec<u64> = (1..=8).map(|n| backoff(n).as_millis() as u64).collect();
        assert_eq!(delays, [250, 500, 1000, 2000, 4000, 5000, 5000, 5000]);
        assert_eq!(backoff(0).as_millis(), 250);
    }
}
