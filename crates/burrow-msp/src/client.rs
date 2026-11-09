//! The connection: a reader task, a writer task, and the state they share with the client.
//!
//! - **Calls** register a pending entry, then queue their request line. The reader matches the
//!   response by id. A call's line is queued when its future is first polled, so calls started
//!   together (`tokio::join!`) go out in order, often in one write.
//! - **Events** go to one unbounded queue in arrival order. Responses and events come off the
//!   same reader, so every event Muse sent before a response is queued before that response
//!   resolves its call.
//! - **Muse's requests.** `approval/request` and `userInput/request` are answered at once with
//!   the empty presentation receipt and surfaced as events, once per requirement or prompt.
//!   If nobody reads events any more, they're answered with an error instead, so they stay
//!   pending for another client. Other requests get `methodNotFound`.
//! - **Shutdown** closes Muse's stdin after the queued lines. `muse serve` then unloads its
//!   sessions (`session/closed`, reason `hostShutdown`), closes stdout and exits 0.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, warn};

use crate::error::{MspError, Result};
use crate::event::{self, Event, Events};
use crate::framing::LineReader;
use crate::methods::{self, Method, names};
use crate::rpc::{self, Incoming, RequestId, RpcError};
use crate::types::*;

/// The fingerprint of the schema this crate was written against (`schema/manifest.json`).
pub const SCHEMA_FINGERPRINT: &str =
    "sha256:c94c3d1f2f51e6424074b9ec43eac3eedcf532d3dcd77351ec1383a87d3049c6";

/// The Muse version that schema came from.
pub const SCHEMA_MUSE_VERSION: &str = "1.4.14";

/// The most bytes the writer gathers into one write.
const WRITE_BATCH_BYTES: usize = 64 * 1024;

/// How to open a connection.
#[derive(Clone, Debug)]
pub struct ConnectOptions {
    /// Who the client is. Defaults to `burrow` and this crate's version.
    pub client_info: ClientInfo,
    /// What to ask for. Defaults to nothing: the stable surface, every notification, and
    /// user-input prompts shown.
    pub capabilities: ClientCapabilities,
    /// How long to wait for `initialize`'s answer. Defaults to 30 s; a cold `muse serve` takes
    /// about 0.5 s.
    pub handshake_timeout: Duration,
    /// The runtime the I/O tasks run on. Defaults to the current one.
    pub runtime: Option<Handle>,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            client_info: ClientInfo {
                name: "burrow".into(),
                title: Some("Burrow".into()),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities: ClientCapabilities::default(),
            handshake_timeout: Duration::from_secs(30),
            runtime: None,
        }
    }
}

enum Outgoing {
    Line(String),
    /// Flush, then close the writer: Muse sees its stdin close.
    Close,
}

struct Pending {
    method: String,
    reply: oneshot::Sender<std::result::Result<Value, RpcError>>,
}

#[derive(Default)]
struct State {
    /// The connection is closed, or closing: no new calls.
    closed: bool,
    pending: HashMap<RequestId, Pending>,
    events: Option<mpsc::UnboundedSender<Event>>,
    /// Approval requirements already surfaced: (approvalId, sourceIndex).
    approvals: HashSet<(String, u32)>,
    /// User-input prompts already surfaced.
    user_inputs: HashSet<String>,
    /// Open items: id to (session, kind), so deltas can say what they append to.
    open_items: HashMap<String, (String, ItemKind)>,
}

struct Shared {
    outgoing: mpsc::UnboundedSender<Outgoing>,
    next_id: AtomicI64,
    state: Mutex<State>,
    closed: watch::Sender<bool>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Marks the connection closed: pending calls fail, the event stream ends. Idempotent.
    fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        state.pending.clear();
        state.events = None;
        drop(state);
        let _ = self.outgoing.send(Outgoing::Close);
        self.closed.send_replace(true);
    }

    /// Stops new calls and closes Muse's stdin after the lines already queued.
    fn begin_shutdown(&self) {
        self.state().closed = true;
        let _ = self.outgoing.send(Outgoing::Close);
    }

    async fn call_value(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = RequestId::Number(self.next_id.fetch_add(1, Ordering::Relaxed));
        let (reply, answer) = oneshot::channel();
        {
            let mut state = self.state();
            if state.closed {
                return Err(MspError::Closed);
            }
            state.pending.insert(
                id.clone(),
                Pending {
                    method: method.to_owned(),
                    reply,
                },
            );
        }
        let line = rpc::request_line(&id, method, params.as_ref());
        if self.outgoing.send(Outgoing::Line(line)).is_err() {
            self.state().pending.remove(&id);
            return Err(MspError::Closed);
        }
        match answer.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(MspError::Rpc {
                method: method.to_owned(),
                error: Box::new(error),
            }),
            Err(_) => Err(MspError::Closed),
        }
    }

    async fn call<M: Method>(&self, params: &M::Params) -> Result<M::Result> {
        let params = if M::HAS_PARAMS {
            Some(
                serde_json::to_value(params).map_err(|error| MspError::Encode {
                    method: M::NAME.to_owned(),
                    error,
                })?,
            )
        } else {
            None
        };
        let value = self.call_value(M::NAME, params).await?;
        serde_json::from_value(value).map_err(|error| MspError::Decode {
            method: M::NAME.to_owned(),
            error,
        })
    }

    fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        if self.state().closed {
            return Err(MspError::Closed);
        }
        let line = rpc::notification_line(method, params.as_ref());
        self.outgoing
            .send(Outgoing::Line(line))
            .map_err(|_| MspError::Closed)
    }

    fn respond(&self, id: &RequestId, outcome: std::result::Result<Value, RpcError>) {
        let _ = self
            .outgoing
            .send(Outgoing::Line(rpc::response_line(id, outcome)));
    }

    fn emit(&self, state: &mut State, event: Event) {
        if let Some(events) = &state.events
            && events.send(event).is_err()
        {
            state.events = None;
        }
    }

    fn dispatch(&self, line: &str) {
        match Incoming::parse(line) {
            Ok(Incoming::Response {
                id: Some(id),
                outcome,
            }) => {
                let pending = self.state().pending.remove(&id);
                match pending {
                    Some(pending) => {
                        if let Err(error) = &outcome {
                            debug!(method = pending.method, %error, "muse refused a call");
                        }
                        let _ = pending.reply.send(outcome);
                    }
                    None => debug!(%id, "a response nobody waits for"),
                }
            }
            Ok(Incoming::Response { id: None, outcome }) => {
                warn!(?outcome, "muse couldn't read a message we sent");
            }
            Ok(Incoming::Request { id, method, params }) => self.on_request(id, method, params),
            Ok(Incoming::Notification { method, params }) => self.on_notification(method, params),
            Err(error) => warn!(%error, "muse sent a line that isn't JSON-RPC"),
        }
    }

    fn on_request(&self, id: RequestId, method: String, params: Option<Value>) {
        let mut state = self.state();
        match method.as_str() {
            names::APPROVAL_REQUEST => {
                match decode::<ApprovalRequestParams>(&method, params) {
                    Ok(request) => self.surface_approval(&mut state, request),
                    Err(event) => self.emit(&mut state, *event),
                }
                let presented = state.events.is_some();
                drop(state);
                self.respond_receipt(&id, presented);
            }
            names::USER_INPUT_REQUEST => {
                match decode::<UserInputRequestParams>(&method, params) {
                    Ok(request) => self.surface_user_input(&mut state, request),
                    Err(event) => self.emit(&mut state, *event),
                }
                let presented = state.events.is_some();
                drop(state);
                self.respond_receipt(&id, presented);
            }
            _ => {
                drop(state);
                debug!(method, "muse sent a request this client doesn't handle");
                self.respond(
                    &id,
                    Err(RpcError {
                        code: -32601,
                        message: format!("burrow-msp doesn't handle {method}"),
                        data: Some(json!({ "kind": "methodNotFound" })),
                    }),
                );
            }
        }
    }

    /// Answers a presentation request: `{}` when the event reached a reader, else an error so
    /// Muse keeps the approval or prompt pending for another client.
    fn respond_receipt(&self, id: &RequestId, presented: bool) {
        let outcome = if presented {
            Ok(json!({}))
        } else {
            Err(RpcError {
                code: -32603,
                message: "nobody reads this client's events".into(),
                data: Some(json!({ "kind": "internal" })),
            })
        };
        self.respond(id, outcome);
    }

    fn surface_approval(&self, state: &mut State, request: ApprovalRequestParams) {
        let key = (
            request.approval_id.clone(),
            request.current_requirement_id.source_index,
        );
        if state.approvals.insert(key) {
            self.emit(state, Event::ApprovalRequested(request));
        }
    }

    fn surface_user_input(&self, state: &mut State, request: UserInputRequestParams) {
        if state.user_inputs.insert(request.user_input_id.clone()) {
            self.emit(state, Event::UserInputRequested(request));
        }
    }

    fn on_notification(&self, method: String, params: Option<Value>) {
        let mut state = self.state();
        let event = match method.as_str() {
            names::APPROVAL_REQUESTED => {
                match decode::<ApprovalRequestParams>(&method, params) {
                    Ok(request) => self.surface_approval(&mut state, request),
                    Err(event) => self.emit(&mut state, *event),
                }
                return;
            }
            names::USER_INPUT_REQUESTED => {
                match decode::<UserInputRequestParams>(&method, params) {
                    Ok(request) => self.surface_user_input(&mut state, request),
                    Err(event) => self.emit(&mut state, *event),
                }
                return;
            }
            names::ITEM_DELTA => match decode::<ItemDeltaParams>(&method, params) {
                Ok(delta) => {
                    let kind = state
                        .open_items
                        .get(&delta.item_id)
                        .map(|(_, kind)| kind.clone());
                    Event::ItemDelta { delta, kind }
                }
                Err(event) => *event,
            },
            _ => event::decode(method, params),
        };
        match &event {
            Event::ItemStarted(params) | Event::ItemUpdated(params)
                if !params.item.status.is_terminal() =>
            {
                state.open_items.insert(
                    params.item.item_id.clone(),
                    (params.session_id.clone(), params.item.kind.clone()),
                );
            }
            Event::ItemStarted(params)
            | Event::ItemUpdated(params)
            | Event::ItemCompleted(params) => {
                state.open_items.remove(&params.item.item_id);
            }
            Event::ApprovalResolved(resolved) => {
                state
                    .approvals
                    .retain(|(approval, _)| *approval != resolved.approval_id);
            }
            Event::UserInputSettled(settled) => {
                state.user_inputs.remove(&settled.user_input_id);
            }
            Event::SessionClosed(closed) => {
                state
                    .open_items
                    .retain(|_, (session, _)| *session != closed.session_id);
            }
            _ => {}
        }
        self.emit(&mut state, event);
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    method: &str,
    params: Option<Value>,
) -> std::result::Result<T, Box<Event>> {
    serde_json::from_value(params.clone().unwrap_or(Value::Null)).map_err(|error| {
        Box::new(Event::Malformed {
            method: method.to_owned(),
            params,
            error: error.to_string(),
        })
    })
}

/// Closes Muse's stdin when the last [`MspClient`] clone is dropped.
struct Owner {
    shared: Arc<Shared>,
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.begin_shutdown();
    }
}

/// A connection to `muse serve`, after the handshake. Cheap to clone; every clone is the same
/// connection. Dropping the last clone closes Muse's stdin, like [`MspClient::shutdown`].
#[derive(Clone)]
pub struct MspClient {
    shared: Arc<Shared>,
    server: Arc<InitializeResult>,
    _owner: Arc<Owner>,
}

impl std::fmt::Debug for MspClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MspClient")
            .field("server", &self.server.server_info)
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl MspClient {
    /// Speaks MSP over `reader` (Muse's stdout) and `writer` (its stdin): starts the I/O tasks
    /// and does the handshake (`initialize`, then `initialized`).
    ///
    /// The caller spawns the process; see [`crate::MuseCommand`] for a helper. Needs a tokio
    /// runtime: the current one, or [`ConnectOptions::runtime`].
    pub async fn connect<R, W>(
        reader: R,
        writer: W,
        options: ConnectOptions,
    ) -> Result<(Self, Events)>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let runtime = options.runtime.clone().unwrap_or_else(Handle::current);
        let (outgoing, queue) = mpsc::unbounded_channel();
        let (events, receiver) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            outgoing,
            next_id: AtomicI64::new(1),
            state: Mutex::new(State {
                events: Some(events),
                ..State::default()
            }),
            closed: watch::Sender::new(false),
        });
        runtime.spawn(write_loop(writer, queue, shared.clone()));
        runtime.spawn(read_loop(LineReader::new(reader), shared.clone()));
        let owner = Arc::new(Owner {
            shared: shared.clone(),
        });

        let params = InitializeParams {
            capabilities: Some(options.capabilities.clone()),
            client_info: options.client_info.clone(),
        };
        let server = tokio::time::timeout(
            options.handshake_timeout,
            shared.call::<methods::Initialize>(&params),
        )
        .await
        .map_err(|_| MspError::HandshakeTimeout)??;
        shared.notify(names::INITIALIZED, None)?;
        if server.schema.fingerprint != SCHEMA_FINGERPRINT {
            warn!(
                muse = server.server_info.version,
                fingerprint = server.schema.fingerprint,
                expected = SCHEMA_FINGERPRINT,
                "muse's MSP schema differs from the one burrow-msp was written against ({SCHEMA_MUSE_VERSION})"
            );
        }
        let client = Self {
            shared,
            server: Arc::new(server),
            _owner: owner,
        };
        Ok((client, Events { receiver }))
    }

    /// What Muse said in the handshake: its version, platform, sandbox and session durability.
    pub fn server(&self) -> &InitializeResult {
        &self.server
    }

    /// Whether Muse's schema fingerprint is the one this crate was written against. A mismatch
    /// usually still works; it's logged as a warning at connect.
    pub fn schema_matches(&self) -> bool {
        self.server.schema.fingerprint == SCHEMA_FINGERPRINT
    }

    /// Calls `M`.
    pub async fn call<M: Method>(&self, params: &M::Params) -> Result<M::Result> {
        self.shared.call::<M>(params).await
    }

    /// Calls any method with raw JSON. `params: None` leaves the member out.
    pub async fn call_raw(&self, method: &str, params: Option<Value>) -> Result<Value> {
        self.shared.call_value(method, params).await
    }

    /// Sends any notification with raw JSON.
    pub fn notify_raw(&self, method: &str, params: Option<Value>) -> Result<()> {
        self.shared.notify(method, params)
    }

    /// `session/start`. The connection is subscribed to the new session, so its events follow.
    pub async fn start_session(&self, params: &SessionStartParams) -> Result<SessionStartResult> {
        self.call::<methods::SessionStart>(params).await
    }

    /// `turn/start`.
    pub async fn start_turn(&self, params: &TurnStartParams) -> Result<TurnStartResult> {
        self.call::<methods::TurnStart>(params).await
    }

    /// `turn/start` with one text part.
    pub async fn prompt(&self, session_id: &str, text: &str) -> Result<TurnStartResult> {
        self.start_turn(&TurnStartParams::text(session_id, text))
            .await
    }

    /// `turn/interrupt`: stop the turn now. `turn_id: None` targets the current turn. The turn
    /// is over at its `turn/completed`, normally with terminal `cancelled`.
    pub async fn interrupt_turn(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
    ) -> Result<TurnInterruptResult> {
        let params = TurnInterruptParams::new(session_id, turn_id.map(str::to_owned));
        self.call::<methods::TurnInterrupt>(&params).await
    }

    /// `turn/cancel`: cancel on the normal command lane.
    pub async fn cancel_turn(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
    ) -> Result<TurnCancelResult> {
        let params = TurnCancelParams::new(session_id, turn_id.map(str::to_owned));
        self.call::<methods::TurnCancel>(&params).await
    }

    /// `approval/decide` with `choice`, one of `request.available_choices`.
    pub async fn decide_approval(
        &self,
        request: &ApprovalRequestParams,
        choice: &ApprovalChoice,
    ) -> Result<ApprovalDecideResult> {
        self.call::<methods::ApprovalDecide>(&ApprovalDecideParams::new(request, choice))
            .await
    }

    /// `approval/listPending`.
    pub async fn list_pending(&self, session_id: &str) -> Result<ApprovalListPendingResult> {
        let params = ApprovalListPendingParams {
            session_id: session_id.to_owned(),
        };
        self.call::<methods::ApprovalListPending>(&params).await
    }

    /// `userInput/answer`.
    pub async fn answer_user_input(
        &self,
        params: &UserInputAnswerParams,
    ) -> Result<UserInputAnswerResult> {
        self.call::<methods::UserInputAnswer>(params).await
    }

    /// `userInput/cancel`.
    pub async fn cancel_user_input(
        &self,
        request: &UserInputRequestParams,
    ) -> Result<UserInputCancelResult> {
        self.call::<methods::UserInputCancel>(&UserInputCancelParams::new(request))
            .await
    }

    /// `usage/read`. `None` when Muse has observed no subscription usage, which is also what an
    /// account without an allowance gets.
    pub async fn read_usage(&self) -> Result<Option<SubscriptionUsage>> {
        Ok(self
            .call::<methods::UsageRead>(&methods::NoParams)
            .await?
            .usage)
    }

    /// `model/list`. With `session_id`, that session's model is marked `isActive`.
    pub async fn list_models(&self, session_id: Option<&str>) -> Result<ModelListResult> {
        let params = ModelListParams {
            session_id: session_id.map(str::to_owned),
        };
        self.call::<methods::ModelList>(&params).await
    }

    /// `session/setModel`.
    pub async fn set_model(
        &self,
        session_id: &str,
        model_id: &str,
    ) -> Result<SessionSetModelResult> {
        self.call::<methods::SessionSetModel>(&SessionSetModelParams::new(session_id, model_id))
            .await
    }

    /// `session/setApprovalMode`.
    pub async fn set_approval_mode(
        &self,
        session_id: &str,
        mode: ApprovalMode,
    ) -> Result<SessionSetApprovalModeResult> {
        let params = SessionSetApprovalModeParams::new(session_id, mode);
        self.call::<methods::SessionSetApprovalMode>(&params).await
    }

    /// `item/readOutput`.
    pub async fn read_output(&self, params: &ItemReadOutputParams) -> Result<ItemReadOutputResult> {
        self.call::<methods::ItemReadOutput>(params).await
    }

    /// Closes Muse's stdin once the queued lines are written, then waits until Muse closes its
    /// stdout. `muse serve` first unloads each session (`session/closed`, `hostShutdown`) and
    /// then exits with code 0. Calls made after this fail with [`MspError::Closed`].
    ///
    /// Doesn't time out: a hung Muse needs killing, which [`crate::MuseProcess::shutdown`]
    /// does.
    pub async fn shutdown(&self) {
        self.shared.begin_shutdown();
        self.closed().await;
    }

    /// Whether the connection is closed.
    pub fn is_closed(&self) -> bool {
        *self.shared.closed.borrow()
    }

    /// Resolves once the connection is closed.
    pub async fn closed(&self) {
        let mut closed = self.shared.closed.subscribe();
        let _ = closed.wait_for(|closed| *closed).await;
    }
}

async fn read_loop<R: AsyncRead + Unpin>(mut lines: LineReader<R>, shared: Arc<Shared>) {
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => shared.dispatch(&line),
            Ok(None) => {
                debug!("muse closed its stdout");
                break;
            }
            Err(error) => {
                warn!(%error, "reading from muse failed");
                break;
            }
        }
    }
    shared.close();
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut queue: mpsc::UnboundedReceiver<Outgoing>,
    shared: Arc<Shared>,
) {
    let mut buffer = Vec::with_capacity(WRITE_BATCH_BYTES);
    let mut closing = false;
    while !closing {
        let Some(first) = queue.recv().await else {
            break;
        };
        buffer.clear();
        let mut next = Some(first);
        while let Some(message) = next.take() {
            match message {
                Outgoing::Line(line) => {
                    buffer.extend_from_slice(line.as_bytes());
                    buffer.push(b'\n');
                }
                Outgoing::Close => {
                    closing = true;
                    break;
                }
            }
            if buffer.len() < WRITE_BATCH_BYTES {
                next = queue.try_recv().ok();
            }
        }
        if buffer.is_empty() {
            continue;
        }
        let written = async {
            writer.write_all(&buffer).await?;
            writer.flush().await
        };
        if let Err(error) = written.await {
            debug!(%error, "writing to muse failed");
            shared.close();
            return;
        }
    }
    let _ = writer.shutdown().await;
    drop(writer);
    debug!("closed muse's stdin");
}
