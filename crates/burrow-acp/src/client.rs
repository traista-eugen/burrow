//! The ACP client: one connection to one agent process.

use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema as acp;
use agent_client_protocol::v1::{ConnectionTo, Responder};
use agent_client_protocol::{Agent, ByteStreams, Client, RequestCancellation, UntypedMessage};
use futures::{AsyncRead, AsyncWrite};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::error::{AUTH_REQUIRED, AcpError, METHOD_NOT_FOUND};
use crate::limit::{self, LimitInfo};
use crate::types::{
    self, AgentInfo, ConfigOption, PermissionKind, PermissionOption, PromptContent, SessionEvent,
    SessionInfo, StopReason, TokenUsage, ToolCallUpdate, TurnEnd,
};

/// How long [`AcpClient::stop`] waits for the connection to wind down.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// How much of an agent message is kept for limit detection. Notices are
/// far shorter.
const MESSAGE_TEXT_LIMIT: usize = 4 * 1024;

/// Options for [`AcpClient::connect`].
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// The client name sent in `initialize`.
    pub client_name: String,
    /// The client version sent in `initialize`.
    pub client_version: String,
    /// How long to wait for the agent to answer `initialize`. Agents started
    /// through `npx` can take a while the first time.
    pub initialize_timeout: Duration,
    /// How long a turn may go on after the agent showed a usage-limit notice
    /// before the client cancels it, and then how long it waits for the
    /// cancelled turn to end. codex-acp sometimes never ends such a turn
    /// (codex-acp#747).
    pub limit_grace: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            client_name: "burrow".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            initialize_timeout: Duration::from_secs(60),
            limit_grace: Duration::from_secs(15),
        }
    }
}

/// Something the agent sent that isn't an answer to a request.
#[derive(Debug)]
pub enum AcpEvent {
    /// A session event.
    Session {
        /// The agent's session id.
        session_id: String,
        /// What happened.
        event: SessionEvent,
    },
    /// The agent asks for permission to run a tool call. Answer it with
    /// [`PermissionRequest::respond`]. It's answered as cancelled if dropped
    /// unanswered, and when the session's turn is cancelled.
    Permission(PermissionRequest),
    /// The connection closed, ending any open turn. No more events follow.
    Closed {
        /// Why, if it wasn't a clean shutdown.
        error: Option<String>,
    },
}

/// The events of one connection, in the order the agent sent them. Implements
/// [`futures::Stream`]; works on any executor.
#[derive(Debug)]
pub struct AcpEvents {
    rx: mpsc::UnboundedReceiver<AcpEvent>,
}

impl AcpEvents {
    /// The next event, or `None` after [`AcpEvent::Closed`].
    pub async fn recv(&mut self) -> Option<AcpEvent> {
        self.rx.recv().await
    }

    /// The next event if one is waiting.
    pub fn try_recv(&mut self) -> Option<AcpEvent> {
        self.rx.try_recv().ok()
    }
}

impl futures::Stream for AcpEvents {
    type Item = AcpEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<AcpEvent>> {
        self.rx.poll_recv(cx)
    }
}

/// The caller's answer to a [`PermissionRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Pick one of the request's options by id.
    Select(String),
    /// Don't decide. The agent treats the tool call as cancelled.
    Cancelled,
}

/// A `session/request_permission` request from the agent, waiting for the
/// caller's decision.
pub struct PermissionRequest {
    /// The session.
    pub session_id: String,
    /// The tool call the agent wants to run. Usually only some fields are set;
    /// the full call came earlier as [`SessionEvent::ToolCall`].
    pub tool_call: ToolCallUpdate,
    /// The answers the agent offers.
    pub options: Vec<PermissionOption>,
    /// The raw request params.
    pub raw: Value,
    slot: Arc<PermissionSlot>,
    cancellation: RequestCancellation,
}

impl std::fmt::Debug for PermissionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionRequest")
            .field("session_id", &self.session_id)
            .field("tool_call", &self.tool_call)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl PermissionRequest {
    /// The first option of `kind`, if the agent offers one.
    pub fn option(&self, kind: &PermissionKind) -> Option<&PermissionOption> {
        self.options.iter().find(|option| &option.kind == kind)
    }

    /// Answers the request. Fails if it was already answered, which happens
    /// when the turn was cancelled in the meantime.
    pub fn respond(&self, decision: PermissionDecision) -> Result<(), AcpError> {
        if let PermissionDecision::Select(id) = &decision
            && !self.options.iter().any(|option| &option.id == id)
        {
            return Err(AcpError::Protocol {
                method: "session/request_permission".into(),
                message: format!("{id:?} is not one of the offered options"),
            });
        }
        if self.slot.answer(decision) {
            Ok(())
        } else {
            Err(AcpError::Protocol {
                method: "session/request_permission".into(),
                message: "the request was already answered".into(),
            })
        }
    }

    /// Allows the tool call once, if the agent offers that; otherwise allows
    /// it always.
    pub fn allow_once(&self) -> Result<(), AcpError> {
        let option = self
            .option(&PermissionKind::AllowOnce)
            .or_else(|| self.option(&PermissionKind::AllowAlways))
            .ok_or_else(|| AcpError::Unsupported("allowing this tool call".into()))?;
        self.respond(PermissionDecision::Select(option.id.clone()))
    }

    /// Rejects the tool call once, if the agent offers that; otherwise answers
    /// cancelled.
    pub fn reject_once(&self) -> Result<(), AcpError> {
        match self.option(&PermissionKind::RejectOnce) {
            Some(option) => self.respond(PermissionDecision::Select(option.id.clone())),
            None => self.respond(PermissionDecision::Cancelled),
        }
    }

    /// Whether the request is still waiting for an answer.
    pub fn is_open(&self) -> bool {
        self.slot.is_open() && !self.cancellation.is_cancelled()
    }

    /// Completes when the agent withdraws the request (`$/cancel_request`).
    pub async fn withdrawn(&self) {
        self.cancellation.cancelled().await;
    }
}

impl Drop for PermissionRequest {
    fn drop(&mut self) {
        // ACP: a request the client won't answer is answered as cancelled.
        self.slot.answer(PermissionDecision::Cancelled);
    }
}

/// Holds the responder of one permission request until somebody answers.
struct PermissionSlot {
    session_id: String,
    responder: Mutex<Option<Responder<Value>>>,
}

impl PermissionSlot {
    /// Sends `decision` unless the request was already answered. Returns
    /// whether this call answered it.
    fn answer(&self, decision: PermissionDecision) -> bool {
        let Some(responder) = lock(&self.responder).take() else {
            return false;
        };
        let outcome = match decision {
            PermissionDecision::Select(id) => json!({"outcome": "selected", "optionId": id}),
            PermissionDecision::Cancelled => json!({"outcome": "cancelled"}),
        };
        if let Err(error) = responder.respond(json!({ "outcome": outcome })) {
            tracing::debug!(%error, "couldn't answer a permission request");
        }
        true
    }

    fn is_open(&self) -> bool {
        lock(&self.responder).is_some()
    }
}

/// State shared between the client and the connection's message handlers.
#[derive(Default)]
struct Shared {
    /// Open permission requests, so cancelling a turn can answer them.
    permissions: Vec<std::sync::Weak<PermissionSlot>>,
    /// Sessions with a prompt in flight.
    turns: HashMap<String, Turn>,
    /// Each session's model selector, so `set_model` knows how to change it.
    model_config: HashMap<String, Option<String>>,
    /// The last reset time of a rate limit the agent reported as rejected,
    /// per session: the fallback when a limit notice has no reset time.
    rejected_resets: HashMap<String, jiff::Timestamp>,
    /// Set when the connection has closed; nothing is sent after `Closed`.
    closed: bool,
}

impl Shared {
    fn send(&self, events: &mpsc::UnboundedSender<AcpEvent>, event: AcpEvent) {
        if !self.closed {
            let _ = events.send(event);
        }
    }
}

/// What the client tracks about a prompt in flight.
struct Turn {
    /// The text of the agent's latest message.
    message: String,
    /// That message's id, if the agent gives ids.
    message_id: Option<String>,
    /// Whether something other than message text came since, so the next
    /// chunk starts a new message.
    interrupted: bool,
    /// A usage-limit notice the latest message reads as.
    notice: Option<LimitInfo>,
    /// Signalled when `notice` is first set.
    notice_seen: Arc<Notify>,
}

impl Turn {
    fn on_event(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::AgentMessageChunk {
                content,
                message_id,
            } => {
                let Some(text) = content.as_text() else {
                    return;
                };
                if self.interrupted || (message_id.is_some() && *message_id != self.message_id) {
                    self.message.clear();
                    self.interrupted = false;
                }
                self.message_id.clone_from(message_id);
                append_bounded(&mut self.message, text);
                let notice = limit::detect_in_reply(&self.message);
                if notice.is_some() && self.notice.is_none() {
                    self.notice_seen.notify_one();
                }
                self.notice = notice;
            }
            // The agent carried on working: what came before wasn't the end.
            SessionEvent::AgentThoughtChunk { .. }
            | SessionEvent::ToolCall(_)
            | SessionEvent::ToolCallUpdate(_) => {
                self.interrupted = true;
                self.notice = None;
            }
            _ => {}
        }
    }
}

struct Inner {
    connection: ConnectionTo<Agent>,
    agent: AgentInfo,
    events: mpsc::UnboundedSender<AcpEvent>,
    shared: Arc<Mutex<Shared>>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
    driver: Mutex<Option<JoinHandle<()>>>,
    limit_grace: Duration,
}

/// A connection to one ACP agent. Cheap to clone; clones share the
/// connection, so one task can prompt while another cancels.
///
/// Every method that talks to the agent must run outside a handler of the
/// connection, which this API guarantees: handlers only queue events.
#[derive(Clone)]
pub struct AcpClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpClient")
            .field("agent", &self.inner.agent.name)
            .finish_non_exhaustive()
    }
}

impl AcpClient {
    /// Connects to an agent over its stdio and runs `initialize`.
    ///
    /// `writer` is the agent's stdin and `reader` its stdout. The caller
    /// starts the process (Burrow starts agents inside a Job Object) and keeps
    /// it; [`AcpClient::stop`] only closes the connection. Must be called on a
    /// tokio runtime, which drives the connection.
    ///
    /// Burrow offers the agent no client capabilities: no `fs/*` and no
    /// `terminal/*` (D-021). Agents use their own tools, and permission
    /// requests are the only requests Burrow answers.
    pub async fn connect<W, R>(
        writer: W,
        reader: R,
        options: ConnectOptions,
    ) -> Result<(AcpClient, AcpEvents), AcpError>
    where
        W: AsyncWrite + Send + 'static,
        R: AsyncRead + Send + 'static,
    {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (connection_tx, connection_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();

        let notification_tx = events_tx.clone();
        let notification_shared = shared.clone();
        let request_tx = events_tx.clone();
        let request_shared = shared.clone();
        let closed_tx = events_tx.clone();
        let closed_shared = shared.clone();

        let connection = Client
            .v1()
            .name(options.client_name.clone())
            .on_receive_notification(
                async move |message: UntypedMessage, _cx| {
                    on_notification(&notification_tx, &notification_shared, message);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |message: UntypedMessage, responder: Responder<Value>, _cx| {
                    on_request(&request_tx, &request_shared, message, responder);
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(
                ByteStreams::new(writer, reader),
                async move |cx: ConnectionTo<Agent>| {
                    let _ = connection_tx.send(cx.clone());
                    // Run until the caller stops us or the agent goes away.
                    tokio::select! {
                        _ = stop_rx => {}
                        () = cx.incoming_closed() => {}
                    }
                    Ok(())
                },
            );
        let driver = tokio::spawn(async move {
            let error = connection.await.err().map(|error| error.to_string());
            if let Some(error) = &error {
                tracing::debug!(%error, "ACP connection ended with an error");
            }
            let mut shared = lock(&closed_shared);
            shared.send(&closed_tx, AcpEvent::Closed { error });
            shared.closed = true;
        });

        let connection = connection_rx.await.map_err(|_| AcpError::Closed)?;
        let initialize = acp::InitializeRequest::new(ProtocolVersion::V1)
            .client_capabilities(acp::ClientCapabilities::default())
            .client_info(acp::Implementation::new(
                options.client_name,
                options.client_version,
            ));
        let raw = match tokio::time::timeout(
            options.initialize_timeout,
            send(&connection, "initialize", &initialize),
        )
        .await
        {
            Ok(result) => result.map_err(|error| error.into_acp(&[])),
            Err(_) => Err(AcpError::Protocol {
                method: "initialize".into(),
                message: format!("no answer within {:?}", options.initialize_timeout),
            }),
        };
        let raw = match raw {
            Ok(raw) => raw,
            Err(error) => {
                let _ = stop_tx.send(());
                return Err(error);
            }
        };
        let agent = types::agent_info_from_json(raw);
        let client = AcpClient {
            inner: Arc::new(Inner {
                connection,
                agent,
                events: events_tx,
                shared,
                stop: Mutex::new(Some(stop_tx)),
                driver: Mutex::new(Some(driver)),
                limit_grace: options.limit_grace,
            }),
        };
        Ok((client, AcpEvents { rx: events_rx }))
    }

    /// Connects over tokio I/O handles, such as a child's stdin and stdout.
    pub async fn connect_tokio<W, R>(
        writer: W,
        reader: R,
        options: ConnectOptions,
    ) -> Result<(AcpClient, AcpEvents), AcpError>
    where
        W: tokio::io::AsyncWrite + Send + 'static,
        R: tokio::io::AsyncRead + Send + 'static,
    {
        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
        Self::connect(writer.compat_write(), reader.compat(), options).await
    }

    /// What the agent said about itself in `initialize`.
    pub fn agent(&self) -> &AgentInfo {
        &self.inner.agent
    }

    /// Authenticates with one of [`AgentInfo::auth_methods`]. Only needed
    /// when a call fails with [`AcpError::AuthRequired`].
    pub async fn authenticate(&self, method_id: &str) -> Result<(), AcpError> {
        self.call(
            "authenticate",
            &acp::AuthenticateRequest::new(method_id.to_owned()),
        )
        .await?;
        Ok(())
    }

    /// Starts a new session working in `cwd`, which must be absolute.
    pub async fn new_session(&self, cwd: &Path) -> Result<SessionInfo, AcpError> {
        let raw = self
            .call("session/new", &acp::NewSessionRequest::new(cwd))
            .await?;
        if raw.get("sessionId").and_then(Value::as_str).is_none() {
            return Err(AcpError::Protocol {
                method: "session/new".into(),
                message: "the result has no sessionId".into(),
            });
        }
        Ok(self.remember_session(None, raw))
    }

    /// Reopens a session. The agent replays its history as session events
    /// (user and agent message chunks, tool calls) before this returns.
    /// Needs [`crate::AgentCapabilities::load_session`].
    pub async fn load_session(
        &self,
        session_id: &str,
        cwd: &Path,
    ) -> Result<SessionInfo, AcpError> {
        if !self.inner.agent.capabilities.load_session {
            return Err(AcpError::Unsupported("session/load".into()));
        }
        let raw = self
            .call(
                "session/load",
                &acp::LoadSessionRequest::new(session_id.to_owned(), cwd),
            )
            .await?;
        Ok(self.remember_session(Some(session_id), raw))
    }

    /// Reopens a session without replaying its history. Needs
    /// [`crate::AgentCapabilities::resume_session`].
    pub async fn resume_session(
        &self,
        session_id: &str,
        cwd: &Path,
    ) -> Result<SessionInfo, AcpError> {
        if !self.inner.agent.capabilities.resume_session {
            return Err(AcpError::Unsupported("session/resume".into()));
        }
        let raw = self
            .call(
                "session/resume",
                &acp::ResumeSessionRequest::new(session_id.to_owned(), cwd),
            )
            .await?;
        Ok(self.remember_session(Some(session_id), raw))
    }

    /// Closes a session and frees what the agent holds for it. Needs
    /// [`crate::AgentCapabilities::close_session`]; without it, stopping the
    /// connection is the way to end a session.
    pub async fn close_session(&self, session_id: &str) -> Result<(), AcpError> {
        if !self.inner.agent.capabilities.close_session {
            return Err(AcpError::Unsupported("session/close".into()));
        }
        self.call(
            "session/close",
            &acp::CloseSessionRequest::new(session_id.to_owned()),
        )
        .await?;
        lock(&self.inner.shared).model_config.remove(session_id);
        Ok(())
    }

    /// Sends a prompt and waits for the turn to end. The reply streams as
    /// [`AcpEvent::Session`] events meanwhile, and ends with
    /// [`SessionEvent::TurnEnded`]. One prompt at a time per session.
    ///
    /// A usage limit fails with [`AcpError::LimitReached`], whether the agent
    /// reported it as an error or only as reply text (see [`crate::limit`]);
    /// a [`SessionEvent::LimitReached`] event precedes the turn's end. If the
    /// agent shows a limit notice but doesn't end the turn within
    /// [`ConnectOptions::limit_grace`], the client cancels the turn.
    pub async fn prompt(
        &self,
        session_id: &str,
        content: impl IntoIterator<Item = PromptContent>,
    ) -> Result<PromptResult, AcpError> {
        let prompt: Vec<_> = content
            .into_iter()
            .map(types::prompt_content_to_acp)
            .collect();
        let notice_seen = Arc::new(Notify::new());
        lock(&self.inner.shared).turns.insert(
            session_id.to_owned(),
            Turn {
                message: String::new(),
                message_id: None,
                interrupted: false,
                notice: None,
                notice_seen: notice_seen.clone(),
            },
        );
        let params = acp::PromptRequest::new(session_id.to_owned(), prompt);
        let request = send(&self.inner.connection, "session/prompt", &params);
        tokio::pin!(request);
        let grace = self.inner.limit_grace;
        let result = loop {
            tokio::select! {
                result = &mut request => break result,
                () = notice_seen.notified() => {
                    if let Ok(result) = tokio::time::timeout(grace, &mut request).await {
                        break result;
                    }
                    let still_limited = lock(&self.inner.shared)
                        .turns
                        .get(session_id)
                        .is_some_and(|turn| turn.notice.is_some());
                    if still_limited {
                        tracing::warn!(
                            session_id,
                            "the agent showed a usage limit but didn't end the turn; cancelling it"
                        );
                        let _ = self.cancel(session_id).await;
                        break tokio::time::timeout(grace, &mut request)
                            .await
                            .unwrap_or(Err(CallError::Abandoned));
                    }
                }
            }
        };
        let (notice, rejected_reset) = {
            let mut shared = lock(&self.inner.shared);
            let notice = shared.turns.remove(session_id).and_then(|turn| turn.notice);
            (notice, shared.rejected_resets.get(session_id).copied())
        };
        let with_reset = |mut info: LimitInfo, other: Option<&LimitInfo>| {
            if info.resets_at.is_none() {
                info.resets_at = other.and_then(|other| other.resets_at).or(rejected_reset);
            }
            info
        };

        let outcome = match (result, notice) {
            // Cursor ends the turn normally after its notice; so does a turn
            // the client cancelled after one.
            (Ok(_), Some(notice)) => Err(AcpError::LimitReached(with_reset(notice, None))),
            (Ok(raw), None) => Ok(PromptResult {
                stop_reason: raw
                    .get("stopReason")
                    .map(types::stop_reason_from_json)
                    .unwrap_or(StopReason::Other(String::new())),
                usage: raw.get("usage").and_then(types::token_usage_from_json),
                raw,
            }),
            (Err(error), notice) => match error.into_acp(&self.inner.agent.auth_methods) {
                AcpError::LimitReached(info) => {
                    Err(AcpError::LimitReached(with_reset(info, notice.as_ref())))
                }
                other => match notice {
                    Some(notice) => Err(AcpError::LimitReached(with_reset(notice, None))),
                    None => Err(other),
                },
            },
        };

        let turn_end = match &outcome {
            Ok(result) => TurnEnd::Stopped {
                reason: result.stop_reason.clone(),
                usage: result.usage.clone(),
            },
            Err(AcpError::LimitReached(info)) => {
                self.emit(session_id, SessionEvent::LimitReached(info.clone()));
                TurnEnd::Limit(info.clone())
            }
            Err(error) => TurnEnd::Error {
                code: error.code(),
                message: error.to_string(),
            },
        };
        self.emit(session_id, SessionEvent::TurnEnded(turn_end));
        outcome
    }

    /// Cancels the session's current turn. The turn still ends normally,
    /// with [`StopReason::Cancelled`]. Open permission requests of the session
    /// are answered as cancelled, as ACP requires.
    pub async fn cancel(&self, session_id: &str) -> Result<(), AcpError> {
        let slots: Vec<_> = {
            let mut shared = lock(&self.inner.shared);
            shared.permissions.retain(|slot| slot.strong_count() > 0);
            shared
                .permissions
                .iter()
                .filter_map(std::sync::Weak::upgrade)
                .filter(|slot| slot.session_id == session_id)
                .collect()
        };
        for slot in slots {
            slot.answer(PermissionDecision::Cancelled);
        }
        let notification = UntypedMessage::new(
            "session/cancel",
            acp::CancelNotification::new(session_id.to_owned()),
        )
        .map_err(|error| CallError::Rpc(error).into_acp(&[]))?;
        self.inner
            .connection
            .send_notification(notification)
            .map_err(|error| CallError::Rpc(error).into_acp(&[]))
    }

    /// Switches the session's mode, for example to a planning mode.
    pub async fn set_mode(&self, session_id: &str, mode_id: &str) -> Result<(), AcpError> {
        self.call(
            "session/set_mode",
            &acp::SetSessionModeRequest::new(session_id.to_owned(), mode_id.to_owned()),
        )
        .await?;
        Ok(())
    }

    /// Changes a session config option to one of its values. Returns every
    /// config option as it now is.
    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<ConfigOption>, AcpError> {
        let raw = self
            .call(
                "session/set_config_option",
                &acp::SetSessionConfigOptionRequest::new(
                    session_id.to_owned(),
                    config_id.to_owned(),
                    acp::SessionConfigOptionValue::value_id(value.to_owned()),
                ),
            )
            .await?;
        Ok(types::config_options_from_json(raw.get("configOptions")))
    }

    /// Changes the session's model, through the model config option when the
    /// agent has one, or else through the older `session/set_model`.
    pub async fn set_model(&self, session_id: &str, model_id: &str) -> Result<(), AcpError> {
        let config_id = lock(&self.inner.shared)
            .model_config
            .get(session_id)
            .cloned()
            .ok_or_else(|| AcpError::Unsupported("choosing a model for this session".into()))?;
        match config_id {
            Some(config_id) => {
                self.set_config_option(session_id, &config_id, model_id)
                    .await?;
            }
            None => {
                self.call(
                    "session/set_model",
                    &json!({"sessionId": session_id, "modelId": model_id}),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Closes the connection. The agent sees its stdin close and normally
    /// exits; the caller still owns the process and should make sure it ends
    /// (Burrow closes its Job Object). Open requests fail with
    /// [`AcpError::Closed`].
    pub async fn stop(&self) {
        if let Some(stop) = lock(&self.inner.stop).take() {
            let _ = stop.send(());
        }
        let driver = lock(&self.inner.driver).take();
        if let Some(driver) = driver
            && tokio::time::timeout(STOP_TIMEOUT, driver).await.is_err()
        {
            tracing::debug!("ACP connection didn't wind down in time");
        }
    }

    /// Whether the connection is closed.
    pub fn is_closed(&self) -> bool {
        self.inner.connection.is_incoming_closed()
            || lock(&self.inner.driver)
                .as_ref()
                .is_none_or(JoinHandle::is_finished)
    }

    /// Sends any request and returns the raw result: for methods this crate
    /// doesn't wrap.
    pub async fn call(&self, method: &str, params: &impl Serialize) -> Result<Value, AcpError> {
        send(&self.inner.connection, method, params)
            .await
            .map_err(|error| error.into_acp(&self.inner.agent.auth_methods))
    }

    fn remember_session(&self, session_id: Option<&str>, raw: Value) -> SessionInfo {
        let info = types::session_info_from_json(session_id, raw);
        let model_config = info.models.as_ref().map(|models| models.config_id.clone());
        let mut shared = lock(&self.inner.shared);
        match model_config {
            Some(config_id) => {
                shared
                    .model_config
                    .insert(info.session_id.clone(), config_id);
            }
            None => {
                shared.model_config.remove(&info.session_id);
            }
        }
        info
    }

    fn emit(&self, session_id: &str, event: SessionEvent) {
        lock(&self.inner.shared).send(
            &self.inner.events,
            AcpEvent::Session {
                session_id: session_id.to_owned(),
                event,
            },
        );
    }
}

/// The result of a prompt turn that ended without an error.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptResult {
    /// Why the turn ended.
    pub stop_reason: StopReason,
    /// Tokens used in the turn, if the agent reports them.
    pub usage: Option<TokenUsage>,
    /// The raw `session/prompt` result.
    pub raw: Value,
}

/// A failed call: either the agent's JSON-RPC error, or the connection.
enum CallError {
    Rpc(agent_client_protocol::Error),
    Encode(String),
    /// The client stopped waiting: the agent showed a limit notice, and
    /// didn't end the turn even when cancelled.
    Abandoned,
}

impl CallError {
    fn into_acp(self, auth_methods: &[types::AuthMethod]) -> AcpError {
        let error = match self {
            CallError::Encode(message) => {
                return AcpError::Protocol {
                    method: String::new(),
                    message,
                };
            }
            CallError::Abandoned => {
                return AcpError::Protocol {
                    method: "session/prompt".into(),
                    message: "the turn didn't end after it was cancelled".into(),
                };
            }
            CallError::Rpc(error) => error,
        };
        if agent_client_protocol::is_connection_terminated(&error)
            || agent_client_protocol::is_incoming_transport_closed(&error)
        {
            return AcpError::Closed;
        }
        let code = error.code.value();
        if let Some(info) = limit::detect_in_error(code, &error.message, error.data.as_ref()) {
            return AcpError::LimitReached(info);
        }
        match code {
            AUTH_REQUIRED => AcpError::AuthRequired {
                message: error.message,
                methods: auth_methods.to_vec(),
            },
            METHOD_NOT_FOUND => AcpError::Unsupported(error.message),
            _ => AcpError::Agent {
                code,
                message: error.message,
                data: error.data,
            },
        }
    }
}

/// Sends a request built from any serializable params, and returns the raw
/// result. Requests go out untyped so that a result this SDK version can't
/// parse still reaches the caller.
async fn send(
    connection: &ConnectionTo<Agent>,
    method: &str,
    params: &impl Serialize,
) -> Result<Value, CallError> {
    let message = UntypedMessage::new(method, params)
        .map_err(|error| CallError::Encode(error.to_string()))?;
    connection
        .send_request(message)
        .block_task()
        .await
        .map_err(CallError::Rpc)
}

/// Handles a notification from the agent. Runs inside the connection's
/// dispatch loop, so it only queues.
fn on_notification(
    events: &mpsc::UnboundedSender<AcpEvent>,
    shared: &Mutex<Shared>,
    message: UntypedMessage,
) {
    if message.method() != "session/update" {
        tracing::trace!(method = message.method(), "ignoring a notification");
        return;
    }
    let (_, params) = message.into_parts();
    let Some(session_id) = params
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        tracing::debug!("session/update without a sessionId");
        return;
    };
    let update = params.get("update").cloned().unwrap_or(Value::Null);
    let rate_limit = limit::rate_limit_status(&update);
    let event = types::session_event_from_json(update);

    let mut shared = lock(shared);
    if let Some(turn) = shared.turns.get_mut(&session_id) {
        turn.on_event(&event);
    }
    shared.send(
        events,
        AcpEvent::Session {
            session_id: session_id.clone(),
            event,
        },
    );
    if let Some(status) = rate_limit {
        if status.status == "rejected"
            && let Some(resets_at) = status.resets_at
        {
            shared.rejected_resets.insert(session_id.clone(), resets_at);
        }
        shared.send(
            events,
            AcpEvent::Session {
                session_id,
                event: SessionEvent::RateLimit(status),
            },
        );
    }
}

/// Handles a request from the agent. Runs inside the connection's dispatch
/// loop, so it never waits: a permission request goes to the caller, who
/// answers later.
fn on_request(
    events: &mpsc::UnboundedSender<AcpEvent>,
    shared: &Mutex<Shared>,
    message: UntypedMessage,
    responder: Responder<Value>,
) {
    if message.method() != "session/request_permission" {
        // Burrow offers no fs/* or terminal/* capabilities (D-021).
        tracing::debug!(
            method = message.method(),
            "rejecting an unsupported request"
        );
        let _ = responder.respond_with_error(agent_client_protocol::Error::method_not_found());
        return;
    }
    let (_, params) = message.into_parts();
    let request: acp::RequestPermissionRequest = match serde_json::from_value(params.clone()) {
        Ok(request) => request,
        Err(error) => {
            tracing::debug!(%error, "unreadable permission request");
            let _ = responder.respond_with_error(
                agent_client_protocol::Error::invalid_params().data(error.to_string()),
            );
            return;
        }
    };
    let session_id = request.session_id.0.to_string();
    let cancellation = responder.cancellation();
    let slot = Arc::new(PermissionSlot {
        session_id: session_id.clone(),
        responder: Mutex::new(Some(responder)),
    });
    {
        let mut shared = lock(shared);
        shared.permissions.retain(|slot| slot.strong_count() > 0);
        shared.permissions.push(Arc::downgrade(&slot));
    }
    let request = PermissionRequest {
        session_id,
        tool_call: types::tool_call_update_from_acp(request.tool_call),
        options: request
            .options
            .into_iter()
            .map(types::permission_option_from_acp)
            .collect(),
        raw: params,
        slot,
        cancellation,
    };
    // If nobody listens, dropping the request answers it as cancelled.
    lock(shared).send(events, AcpEvent::Permission(request));
}

/// Appends `text`, keeping only the last [`MESSAGE_TEXT_LIMIT`] bytes.
fn append_bounded(buffer: &mut String, text: &str) {
    buffer.push_str(text);
    if buffer.len() > MESSAGE_TEXT_LIMIT {
        let mut cut = buffer.len() - MESSAGE_TEXT_LIMIT;
        while !buffer.is_char_boundary(cut) {
            cut += 1;
        }
        buffer.drain(..cut);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<AcpClient>();
        check::<PermissionRequest>();
        check::<AcpEvents>();
    }

    #[test]
    fn append_bounded_keeps_the_tail_on_a_char_boundary() {
        let mut buffer = String::new();
        append_bounded(&mut buffer, &"é".repeat(MESSAGE_TEXT_LIMIT));
        append_bounded(&mut buffer, "end");
        assert!(buffer.len() <= MESSAGE_TEXT_LIMIT);
        assert!(buffer.ends_with("end"));
    }
}
