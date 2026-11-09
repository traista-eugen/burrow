//! The adapter interface: what every agent integration implements (D-018).
//!
//! An [`AgentAdapter`] knows one agent. It detects it ([`AgentAdapter::detect`]) and starts
//! sessions ([`AgentAdapter::start`]). A started session is an [`AgentSession`] handle for
//! commands plus a stream of [`AgentEvent`]s, normalized to Burrow's session events.
//!
//! The session manager in `burrowd` assigns turn ids, inbox items and seqs; adapters don't. An
//! adapter only translates:
//!
//! | Burrow → agent | Agent → Burrow |
//! |---|---|
//! | [`AgentSession::prompt`], [`cancel`](AgentSession::cancel), [`set_model`](AgentSession::set_model), [`stop`](AgentSession::stop) | [`AgentEvent`]s, in order, ending with [`AgentEvent::Exited`] |
//! | the [`PermissionReply`] of a request | [`AgentEvent::Permission`] |
//!
//! # Rules for adapters
//!
//! - **Turns.** `prompt` returns once the agent has the prompt; it never waits for the turn. Each
//!   prompt that succeeds ends with exactly one [`AgentEvent::TurnEnded`], also after `cancel`
//!   (reason `cancelled`). Events that belong to the turn come before it.
//! - **Permissions.** Send [`AgentEvent::Permission`] and wait for its [`PermissionReply`]. If
//!   the reply is dropped unanswered, treat it as [`PermissionOutcome::Cancelled`].
//! - **Limits.** When the provider reports a usage limit, send [`AgentEvent::LimitReached`],
//!   with `resets_at` when known, then end the turn with reason `limit`. Report usage figures
//!   with [`AgentEvent::Usage`] (per turn) and [`AgentEvent::ProviderUsage`] (allowances).
//! - **Processes.** Start agent processes with [`crate::launcher::spawn`], so they run in a Job
//!   Object. Dropping the future returned by `start` must stop anything it started; holding the
//!   [`crate::launcher::AgentProcess`] inside that future does this.
//! - **Exit.** When the agent process ends, send [`AgentEvent::Exited`] last, then close the
//!   channel. `error` is `None` only when [`AgentSession::stop`] asked for it.
//! - **Never block on the event channel.** It is unbounded; nothing the manager does waits for
//!   an adapter's event to be read.

use std::fmt;
use std::path::PathBuf;

use burrow_protocol::{
    AgentId, AgentInfo, ContentBlock, Integration, ModelInfo, PermissionOption, Plan,
    ProviderUsage, Timestamp, ToolCall, ToolCallUpdate, TurnEndReason,
};
use futures::future::BoxFuture;
use tokio::sync::{mpsc, oneshot};

/// One agent integration: detection plus starting sessions.
pub trait AgentAdapter: Send + Sync + 'static {
    /// Which agent this adapter drives.
    fn id(&self) -> AgentId;

    /// The agent's display name, for example `Claude Code`.
    fn name(&self) -> &str;

    /// How Burrow talks to the agent.
    fn integration(&self) -> Integration;

    /// Finds out whether the agent is installed, its version, login state and models (when
    /// cheap), and whether sessions can start. Must not spend the user's quota. The caller
    /// caches the result.
    fn detect(&self) -> BoxFuture<'_, AgentInfo>;

    /// Starts a session and resolves once the agent is ready for a prompt.
    ///
    /// Dropping the future before it resolves must stop everything it started.
    fn start(&self, config: SessionConfig) -> BoxFuture<'_, Result<StartedSession, AgentError>>;
}

/// How to start a session.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    /// Burrow's id for the session (`ses_…`), for logs.
    pub session_id: String,
    /// The folder the agent works in.
    pub cwd: PathBuf,
    /// The model to use; the agent's default when `None`.
    pub model: Option<String>,
}

/// A session that started: its command handle, its events, and what the agent said about it.
pub struct StartedSession {
    /// Commands: prompt, cancel, set the model, stop.
    pub session: Box<dyn AgentSession>,
    /// Everything the agent does, in order, ending with [`AgentEvent::Exited`].
    pub events: mpsc::UnboundedReceiver<AgentEvent>,
    /// The model in use, if the agent says.
    pub model: Option<String>,
    /// Models [`AgentSession::set_model`] accepts, if the agent says.
    pub models: Vec<ModelInfo>,
    /// The agent's own id for the session, for example ACP's `sessionId`, for resuming later.
    pub agent_session_id: Option<String>,
}

impl fmt::Debug for StartedSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartedSession")
            .field("model", &self.model)
            .field("models", &self.models)
            .field("agent_session_id", &self.agent_session_id)
            .finish_non_exhaustive()
    }
}

/// Commands to a running session. Every method may be called while events are being read.
///
/// Dropping the handle must kill the agent's process tree at once, without the grace period
/// [`AgentSession::stop`] may give it.
pub trait AgentSession: Send + Sync {
    /// Sends a prompt. Resolves once the agent has it; the turn's output and its end arrive as
    /// events. Only text blocks are passed for now.
    fn prompt(&self, content: Vec<ContentBlock>) -> BoxFuture<'_, Result<(), AgentError>>;

    /// Asks the agent to stop the running turn. The turn still ends with
    /// [`AgentEvent::TurnEnded`], reason `cancelled`.
    fn cancel(&self) -> BoxFuture<'_, Result<(), AgentError>>;

    /// Switches the model. The agent confirms with [`AgentEvent::ModelChanged`], or the manager
    /// records the model itself if this succeeds without one.
    fn set_model(&self, model: String) -> BoxFuture<'_, Result<(), AgentError>>;

    /// Ends the agent process and everything it started, and resolves once they are gone.
    /// Calling it again does nothing.
    fn stop(&self) -> BoxFuture<'_, ()>;
}

/// Something the agent did, normalized to Burrow's session events.
#[derive(Debug)]
pub enum AgentEvent {
    /// A piece of the answer. Becomes `agentMessageChunk`.
    MessageChunk(String),
    /// A piece of visible reasoning. Becomes `agentThoughtChunk`.
    ThoughtChunk(String),
    /// A tool call started or was proposed. Leave `turn_id` empty; the manager fills it in.
    ToolCall(ToolCall),
    /// A tool call changed.
    ToolCallUpdate(ToolCallUpdate),
    /// The agent's plan.
    Plan(Plan),
    /// The agent asks before a tool call. Becomes an inbox item.
    Permission(PermissionRequest),
    /// The running turn is over.
    TurnEnded {
        /// Why.
        reason: TurnEndReason,
        /// More detail, for example an error message.
        message: Option<String>,
    },
    /// Tokens and cost of the turn.
    Usage {
        /// Input tokens.
        input_tokens: u64,
        /// Output tokens.
        output_tokens: u64,
        /// Cost in US dollars, when known.
        cost_usd: Option<f64>,
    },
    /// A provider limit was hit. End the turn with reason `limit` afterwards.
    LimitReached {
        /// The provider; the agent's id when `None`.
        provider: Option<String>,
        /// When it resets, if known.
        resets_at: Option<Timestamp>,
        /// The provider's message.
        message: String,
    },
    /// A provider's allowance, for `usage/get`.
    ProviderUsage(ProviderUsage),
    /// The session now uses another model.
    ModelChanged(String),
    /// The agent process ended. Always the last event. `error` is `None` when
    /// [`AgentSession::stop`] ended it, and says what happened otherwise.
    Exited {
        /// Why it ended, if it wasn't asked to.
        error: Option<String>,
    },
}

/// A permission request, with the channel for its answer.
#[derive(Debug)]
pub struct PermissionRequest {
    /// The tool call it is about.
    pub tool_call_id: String,
    /// What the agent wants to do, for example `Edit README.md`.
    pub title: String,
    /// The answers the agent offers.
    pub options: Vec<PermissionOption>,
    /// Where the answer goes.
    pub reply: PermissionReply,
}

/// The answer to a permission request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// One of the offered options, by `optionId`.
    Selected(String),
    /// Nobody answered: the turn was cancelled or the session stopped.
    Cancelled,
}

/// Sends the answer to one permission request. Dropping it unanswered means
/// [`PermissionOutcome::Cancelled`].
#[derive(Debug)]
pub struct PermissionReply(oneshot::Sender<PermissionOutcome>);

impl PermissionReply {
    /// A reply and the receiver the adapter waits on.
    pub fn channel() -> (Self, oneshot::Receiver<PermissionOutcome>) {
        let (sender, receiver) = oneshot::channel();
        (Self(sender), receiver)
    }

    /// Sends the answer. Does nothing if the adapter stopped waiting.
    pub fn send(self, outcome: PermissionOutcome) {
        let _ = self.0.send(outcome);
    }
}

/// Why an adapter call failed.
#[derive(Clone, Debug, thiserror::Error)]
pub enum AgentError {
    /// Something needed is missing: the agent isn't installed, isn't logged in, its adapter isn't
    /// built yet, or its process couldn't start.
    #[error("{0}")]
    Unavailable(String),
    /// Not possible right now, for example a prompt while the agent is shutting down.
    #[error("{0}")]
    InvalidState(String),
    /// A provider limit was hit.
    #[error("{message}")]
    LimitReached {
        /// The provider's message.
        message: String,
        /// When it resets, if known.
        resets_at: Option<Timestamp>,
    },
    /// The agent or its protocol failed.
    #[error("{0}")]
    Failed(String),
}
