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
//! | [`AgentSession::prompt`], [`cancel`](AgentSession::cancel), [`set_model`](AgentSession::set_model), [`set_agent_mode`](AgentSession::set_agent_mode), [`stop`](AgentSession::stop) | [`AgentEvent`]s, in order, ending with [`AgentEvent::Exited`] |
//! | the [`PermissionReply`] of a request | [`AgentEvent::Permission`] |
//!
//! # Rules for adapters
//!
//! - **Turns.** `prompt` returns once the agent has the prompt; it never waits for the turn. Each
//!   prompt that succeeds ends with exactly one [`AgentEvent::TurnEnded`], also after `cancel`
//!   (reason `cancelled`). Events that belong to the turn come before it.
//! - **Permissions.** Send [`AgentEvent::Permission`] and wait for its [`PermissionReply`]. If
//!   the reply is dropped unanswered, treat it as [`PermissionOutcome::Cancelled`]. When the
//!   agent stops waiting for an answer by itself (ACP's `$/cancel_request`, an approval Muse
//!   settled, a timeout), send [`AgentEvent::PermissionWithdrawn`] with the reply's
//!   [`PermissionReply::id`], so the question leaves the inbox at once. Withdrawing a request
//!   that was answered meanwhile is harmless; the manager ignores it.
//! - **Limits.** When the provider reports a usage limit, send [`AgentEvent::LimitReached`],
//!   with `resets_at` when known, then end the turn with reason `limit`. Report usage figures
//!   with [`AgentEvent::Usage`] (per turn) and [`AgentEvent::ProviderUsage`] (allowances).
//! - **Processes.** Start agent processes with [`crate::launcher::spawn`], so they run in a Job
//!   Object. Dropping the future returned by `start` must stop anything it started; holding the
//!   [`crate::launcher::AgentProcess`] inside that future does this.
//! - **Exit.** When the agent process ends, send [`AgentEvent::Exited`] last, then close the
//!   channel. `error` is `None` only when [`AgentSession::stop`] asked for it.
//! - **Modes.** An agent with modes of its own (ACP session modes, such as Claude Code's
//!   `default` or Codex's `read-only`, or Muse's approval modes) reports them with
//!   [`AgentSession::agent_modes`] and starts in the mode that asks before changes. The manager
//!   switches to the mode `session/create` asked for with [`AgentSession::set_agent_mode`].
//! - **Models.** A requested model may name a model by its id or by its display name
//!   ([`find_model`]); resolve it before passing it on, and report the id in
//!   [`StartedSession::model`].
//! - **Installing.** An agent that needs something Burrow installs, such as a pinned npm
//!   adapter, does it in [`AgentAdapter::install`], never as a side effect of `start`.
//! - **Never block on the event channel.** It is unbounded; nothing the manager does waits for
//!   an adapter's event to be read.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use burrow_protocol::{
    AgentId, AgentInfo, AgentMode, ContentBlock, Integration, ModelInfo, PermissionOption, Plan,
    ProviderUsage, Timestamp, ToolCall, ToolCallUpdate, TurnEndReason,
};
use futures::future::BoxFuture;
use tokio::sync::{mpsc, oneshot};

use crate::launcher::LaunchSpec;

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

    /// The command that runs the agent's own interactive TUI, for a terminal session (M4). The
    /// caller starts it on a pseudo console with [`crate::launcher::spawn_pty`]; this only says
    /// what to run, with [`crate::terminal::for_session`] applied. The model and modes are the
    /// TUI's business.
    ///
    /// The default: the agent has no TUI Burrow can run, which is [`AgentError::Unavailable`].
    fn terminal(&self, config: &SessionConfig) -> Result<LaunchSpec, AgentError> {
        let _ = config;
        Err(AgentError::Unavailable(format!(
            "{} has no terminal mode",
            self.name()
        )))
    }

    /// Installs what Burrow manages for this agent, such as the pinned npm adapter an ACP agent
    /// runs through, and resolves once it is in place. Installing what is already installed
    /// does nothing. Dropping the future stops the install and leaves nothing half-installed.
    ///
    /// The default: there is nothing to install, which is [`AgentError::InvalidState`].
    fn install(&self) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            Err(AgentError::InvalidState(format!(
                "{} needs nothing installed by Burrow",
                self.name()
            )))
        })
    }
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

    /// The agent's own modes for this session, if it has any: the current one and the ones
    /// [`AgentSession::set_agent_mode`] accepts. The default: no modes.
    fn agent_modes(&self) -> Option<AgentModes> {
        None
    }

    /// Switches the agent's own mode. The agent may confirm with
    /// [`AgentEvent::AgentModeChanged`]; the manager records the mode once this succeeds. An
    /// unknown mode is [`AgentError::Invalid`].
    ///
    /// The default: the agent has no modes, which is [`AgentError::Invalid`].
    fn set_agent_mode(&self, mode: String) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            Err(AgentError::Invalid(format!(
                "this agent has no modes, so it can't switch to {mode:?}"
            )))
        })
    }

    /// Ends the agent process and everything it started, and resolves once they are gone.
    /// Calling it again does nothing.
    fn stop(&self) -> BoxFuture<'_, ()>;
}

/// An agent's own modes in one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentModes {
    /// The current mode's id.
    pub current: String,
    /// Every mode the session offers.
    pub available: Vec<AgentMode>,
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
    /// The agent no longer waits for the answer to an open permission request, for example
    /// because it timed out or someone answered it elsewhere. Its inbox item expires at once.
    PermissionWithdrawn(PermissionId),
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
    /// The agent now runs in another of its own modes, for example because it left a planning
    /// mode by itself.
    AgentModeChanged(String),
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
pub struct PermissionReply {
    id: PermissionId,
    sender: oneshot::Sender<PermissionOutcome>,
}

impl PermissionReply {
    /// A reply and the receiver the adapter waits on.
    pub fn channel() -> (Self, oneshot::Receiver<PermissionOutcome>) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let (sender, receiver) = oneshot::channel();
        let id = PermissionId(NEXT.fetch_add(1, Ordering::Relaxed));
        (Self { id, sender }, receiver)
    }

    /// The request's id, unique within the process: what [`AgentEvent::PermissionWithdrawn`]
    /// names.
    pub fn id(&self) -> PermissionId {
        self.id
    }

    /// Sends the answer. Does nothing if the adapter stopped waiting.
    pub fn send(self, outcome: PermissionOutcome) {
        let _ = self.sender.send(outcome);
    }
}

/// Names one permission request, so an adapter can withdraw it
/// ([`AgentEvent::PermissionWithdrawn`]). Opaque; unique within the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PermissionId(u64);

impl fmt::Display for PermissionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "permission {}", self.0)
    }
}

/// The model `wanted` names among `models`: the one whose id it is, or else the only one whose
/// display name it is, ignoring case. So `gpt-5.5` finds Cursor's
/// `gpt-5.5[context=272k,reasoning=medium,fast=false]` (named `gpt-5.5`), and `auto` its
/// `default[]` (named `Auto`). `None` when it names none, or more than one by name.
pub fn find_model<'a>(models: &'a [ModelInfo], wanted: &str) -> Option<&'a ModelInfo> {
    if let Some(model) = models.iter().find(|model| model.id == wanted) {
        return Some(model);
    }
    let mut named = models.iter().filter(|model| {
        model
            .name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
    });
    let first = named.next()?;
    named.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, name: Option<&str>) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            name: name.map(Into::into),
        }
    }

    #[test]
    fn models_are_found_by_id_or_by_a_unique_name() {
        let models = [
            model("default[]", Some("Auto")),
            model("gpt-5.5[context=272k]", Some("gpt-5.5")),
            model("a[x]", Some("twin")),
            model("a[y]", Some("twin")),
            model("plain", None),
        ];
        let id = |wanted| find_model(&models, wanted).map(|model| model.id.as_str());
        assert_eq!(id("gpt-5.5[context=272k]"), Some("gpt-5.5[context=272k]"));
        assert_eq!(id("gpt-5.5"), Some("gpt-5.5[context=272k]"));
        assert_eq!(id("auto"), Some("default[]"));
        assert_eq!(id("plain"), Some("plain"));
        assert_eq!(id("twin"), None, "ambiguous names find nothing");
        assert_eq!(id("gpt-5.4"), None);
    }

    #[test]
    fn every_reply_has_its_own_id() {
        let (first, _) = PermissionReply::channel();
        let (second, _) = PermissionReply::channel();
        assert_ne!(first.id(), second.id());
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
    /// The request names something the agent doesn't offer, such as an unknown model or mode.
    #[error("{0}")]
    Invalid(String),
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
