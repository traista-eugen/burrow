//! **Phase 2 slot:** the ACP adapter for Claude Code, Codex and Cursor.
//!
//! Until it lands, [`AcpAdapter`] detects the three CLIs (see [`crate::detect`]) and reports
//! them unavailable; [`AcpAdapter::start`] fails with [`AgentError::Unavailable`].
//!
//! # How phase 2 fills it in
//!
//! `start` should:
//! 1. Build a [`LaunchSpec`](crate::launcher::LaunchSpec) for the agent's ACP server:
//!    `agent acp` for Cursor (find it with [`detect::CURSOR`]`.find()`), or `npx` with
//!    `@agentclientprotocol/claude-agent-acp` / `@agentclientprotocol/codex-acp`. Set `cwd` to
//!    [`SessionConfig::cwd`].
//! 2. [`launcher::spawn`](crate::launcher::spawn) it, then hand
//!    [`AgentProcess::take_stdin`](crate::launcher::AgentProcess::take_stdin) and
//!    [`take_stdout`](crate::launcher::AgentProcess::take_stdout) to the `burrow-acp` client.
//!    Keep a [`ProcessHandle`](crate::launcher::ProcessHandle) for `stop`.
//! 3. `initialize`, then `session/new { cwd }`, and set the model if one was asked for.
//! 4. Map each `session/update` to an [`AgentEvent`](crate::AgentEvent): message and thought
//!    chunks, tool calls and updates (statuses are camelCase in Burrow, `in_progress` →
//!    `inProgress`), plans. `session/request_permission` becomes
//!    [`AgentEvent::Permission`](crate::AgentEvent::Permission); answer ACP with the
//!    [`PermissionOutcome`](crate::PermissionOutcome).
//! 5. `prompt` sends `session/prompt` and returns without waiting for its response. A task
//!    awaits it and sends [`AgentEvent::TurnEnded`](crate::AgentEvent::TurnEnded), mapping the
//!    stop reason: `end_turn` → `completed`, `cancelled` → `cancelled`, everything else →
//!    `error` with the reason as message, or `limit` after a usage limit.
//! 6. `cancel` sends `session/cancel`. `stop` closes stdin and calls
//!    [`ProcessHandle::stop`](crate::launcher::ProcessHandle::stop). Dropping the session kills
//!    the tree.
//!
//! [`detect`] should stay as it is, plus whatever the adapter needs checked, for example that
//! `npx` exists; set `available` and `problem` accordingly.

use burrow_protocol::{AgentId, AgentInfo, Integration};
use futures::future::BoxFuture;

use crate::adapter::{AgentAdapter, AgentError, SessionConfig, StartedSession};
use crate::detect::{self, CliAgent};

/// Claude Code, Codex or Cursor, through ACP. Not built yet: detection only.
#[derive(Clone, Copy, Debug)]
pub struct AcpAdapter {
    agent: &'static CliAgent,
}

impl AcpAdapter {
    /// Claude Code.
    pub fn claude() -> Self {
        Self {
            agent: &detect::CLAUDE,
        }
    }

    /// Codex.
    pub fn codex() -> Self {
        Self {
            agent: &detect::CODEX,
        }
    }

    /// The Cursor CLI.
    pub fn cursor() -> Self {
        Self {
            agent: &detect::CURSOR,
        }
    }

    fn not_built(&self) -> String {
        format!(
            "Burrow can't drive {} yet: its ACP adapter is still being built (M2 phase 2)",
            self.agent.name
        )
    }
}

impl AgentAdapter for AcpAdapter {
    fn id(&self) -> AgentId {
        self.agent.agent_id()
    }

    fn name(&self) -> &str {
        self.agent.name
    }

    fn integration(&self) -> Integration {
        Integration::Acp
    }

    fn detect(&self) -> BoxFuture<'_, AgentInfo> {
        Box::pin(async move {
            let mut info = detect::detect(self.agent).await;
            if info.installed {
                info.available = false;
                info.problem = Some(self.not_built());
            }
            info
        })
    }

    fn start(&self, _config: SessionConfig) -> BoxFuture<'_, Result<StartedSession, AgentError>> {
        Box::pin(async move { Err(AgentError::Unavailable(self.not_built())) })
    }
}
