//! The ACP adapter: Claude Code, Codex and Cursor as Burrow sessions, through `burrow-acp`.
//!
//! # Launching (D-020, D-024)
//!
//! | Agent | Runs | Needs |
//! |---|---|---|
//! | Claude Code | `node …\claude-agent-acp\0.101.0\node_modules\…\dist\index.js` | Node.js; the adapter installed with `agent/install` |
//! | Codex | `node …\codex-acp\2.6.0\node_modules\…\dist\index.js` | Node.js; the adapter installed with `agent/install` |
//! | Cursor | `%LOCALAPPDATA%\cursor-agent\versions\<newest>\node.exe index.js acp` | the Cursor CLI |
//!
//! The npm adapters live in Burrow's agents folder, `%LOCALAPPDATA%\Burrow\agents` by default,
//! one folder per pinned version. Nothing is installed implicitly: [`AcpAdapter::detect`]
//! reports a missing adapter, `session/create` answers `unavailable` with the command that
//! installs it, and [`AcpAdapter::install`] runs npm ([`install`]). There is no `npx` fallback.
//!
//! Every process runs in a Job Object through [`crate::launcher`]; its stdin and stdout go to
//! `burrow-acp`'s [`AcpClient`](burrow_acp::AcpClient).
//!
//! # Modes (D-024)
//!
//! Whether an agent asks before acting depends on its mode, so each session starts in the mode
//! that asks before changes:
//!
//! | Agent | Default mode | Asks before |
//! |---|---|---|
//! | Claude Code | `default` | edits and commands |
//! | Codex | `read-only` | edits and commands |
//! | Cursor | `agent` (its own default) | shell commands only; Cursor never asks before edits |
//!
//! `session/create`'s `agentMode` and `session/setAgentMode` choose another one.
//!
//! # Mapping
//!
//! [`session`] turns `burrow-acp` events into [`AgentEvent`](crate::AgentEvent)s with the
//! conversions in [`map`]: text and thought chunks, tool calls and their updates, plans,
//! permission requests, usage per turn, limits (with `resetsAt` when the agent says), Claude's
//! rate-limit status as provider usage, and the agent's own mode and model changes. It also
//! holds the watchdog for turns that don't end after a cancel (D-025).
//!
//! # The fake ACP agent
//!
//! [`fake_agent`] is a scripted ACP agent, run as `burrow-fake-acp-agent`. `burrowd --dev`
//! offers it as agent `fake-acp`, so the whole adapter is tested without spending quota.

pub mod fake_agent;
pub mod install;
pub mod map;
pub mod session;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use burrow_acp::launch::{self, AgentCommand, NpmAdapter};
use burrow_protocol::{
    AdapterInfo, AgentId, AgentInfo, AgentLogin, Integration, LoginState, Timestamp,
};
use futures::future::BoxFuture;

use self::install::Npm;
use self::session::{Launcher, Timing};
use crate::adapter::{AgentAdapter, AgentError, SessionConfig, StartedSession};
use crate::detect::{self, CliAgent};

/// The id `burrowd --dev` gives the fake ACP agent.
pub const FAKE_ACP_ID: &str = "fake-acp";
/// The fake ACP agent's executable name, without the platform's extension.
pub const FAKE_ACP_PROGRAM: &str = "burrow-fake-acp-agent";

/// How an agent's ACP server is started.
#[derive(Clone, Debug)]
enum Launch {
    /// Cursor's own `agent acp`, run with its bundled Node.
    Cursor,
    /// An npm adapter from Burrow's agents folder, run with Node.
    Npm(NpmAdapter),
    /// A program that speaks ACP itself: the fake.
    Program(PathBuf),
}

/// Claude Code, Codex, Cursor, or the fake ACP agent, through ACP.
#[derive(Clone, Debug)]
pub struct AcpAdapter {
    id: AgentId,
    name: &'static str,
    /// The CLI detection looks for; the fake has none.
    cli: Option<&'static CliAgent>,
    launch: Launch,
    default_mode: Option<&'static str>,
    agents_dir: PathBuf,
    timing: Timing,
    /// One install at a time.
    installing: Arc<tokio::sync::Mutex<()>>,
}

impl AcpAdapter {
    fn new(
        cli: &'static CliAgent,
        launch: Launch,
        default_mode: Option<&'static str>,
        agents_dir: PathBuf,
    ) -> Self {
        Self {
            id: cli.agent_id(),
            name: cli.name,
            cli: Some(cli),
            launch,
            default_mode,
            agents_dir,
            timing: Timing::default(),
            installing: Arc::default(),
        }
    }

    /// Claude Code, through `@agentclientprotocol/claude-agent-acp`, installed under
    /// `agents_dir`. Sessions start in mode `default`, which asks before edits and commands.
    pub fn claude(agents_dir: impl Into<PathBuf>) -> Self {
        Self::new(
            &detect::CLAUDE,
            Launch::Npm(launch::CLAUDE_AGENT_ACP),
            Some("default"),
            agents_dir.into(),
        )
    }

    /// Codex, through `@agentclientprotocol/codex-acp`, installed under `agents_dir`. Sessions
    /// start in mode `read-only`, which asks before edits and commands.
    pub fn codex(agents_dir: impl Into<PathBuf>) -> Self {
        Self::new(
            &detect::CODEX,
            Launch::Npm(launch::CODEX_ACP),
            Some("read-only"),
            agents_dir.into(),
        )
    }

    /// The Cursor CLI's built-in `agent acp`. Sessions keep Cursor's own default mode, `agent`:
    /// none of its modes asks before file edits.
    pub fn cursor() -> Self {
        Self::new(&detect::CURSOR, Launch::Cursor, None, PathBuf::new())
    }

    /// The scripted fake ACP agent at `program`, as agent `fake-acp`. Its default mode is
    /// `ask`, and a cancelled turn gets one second before the watchdog steps in.
    pub fn fake(program: impl Into<PathBuf>) -> Self {
        Self {
            id: AgentId::from(FAKE_ACP_ID),
            name: "Fake ACP agent",
            cli: None,
            launch: Launch::Program(program.into()),
            default_mode: Some(fake_agent::ASK_MODE),
            agents_dir: PathBuf::new(),
            timing: Timing {
                cancel_grace: Duration::from_secs(1),
                ..Timing::default()
            },
            installing: Arc::default(),
        }
    }

    /// The npm adapter this agent runs through, if it has one.
    pub fn npm_adapter(&self) -> Option<NpmAdapter> {
        match self.launch {
            Launch::Npm(adapter) => Some(adapter),
            _ => None,
        }
    }

    fn adapter_info(&self, adapter: &NpmAdapter) -> AdapterInfo {
        AdapterInfo {
            package: adapter.package.to_owned(),
            version: adapter.version.to_owned(),
            installed: adapter.is_installed(&self.agents_dir),
            path: Some(adapter.install_dir(&self.agents_dir).display().to_string()),
        }
    }

    /// What `burrowctl` command installs this agent's adapter.
    fn install_hint(&self) -> String {
        format!("burrowctl agent install {}", self.id)
    }

    /// The command that runs the agent's ACP server, or why there is none.
    fn command(&self) -> Result<AgentCommand, AgentError> {
        let name = self.name;
        match &self.launch {
            Launch::Npm(adapter) => {
                let node = install::find_node().ok_or_else(|| {
                    AgentError::Unavailable(format!(
                        "{name} runs through Node.js, which isn't on PATH: install Node.js 22 or \
                         later"
                    ))
                })?;
                adapter.command(&self.agents_dir, &node).ok_or_else(|| {
                    AgentError::Unavailable(format!(
                        "{name}'s ACP adapter ({} {}) isn't installed: run `{}`",
                        adapter.package,
                        adapter.version,
                        self.install_hint()
                    ))
                })
            }
            Launch::Cursor => {
                let dir = cursor_dir().ok_or_else(|| {
                    AgentError::Unavailable("can't find the Cursor CLI's install folder".into())
                })?;
                launch::cursor_command(&dir).ok_or_else(|| {
                    AgentError::Unavailable(format!(
                        "can't find Cursor's bundled node.exe and index.js under {}",
                        dir.join("versions").display()
                    ))
                })
            }
            Launch::Program(program) => {
                if program.is_file() {
                    Ok(AgentCommand::new(program))
                } else {
                    Err(AgentError::Unavailable(format!(
                        "{} doesn't exist",
                        program.display()
                    )))
                }
            }
        }
    }

    async fn detect_info(&self) -> AgentInfo {
        let mut info = match self.cli {
            Some(cli) => detect::detect(cli).await,
            None => self.fake_info(),
        };
        info.default_agent_mode = self.default_mode.map(str::to_owned);
        if let Launch::Npm(adapter) = &self.launch {
            info.adapter = Some(self.adapter_info(adapter));
        }
        if info.available
            && let Err(error) = self.command()
        {
            info.available = false;
            info.problem = Some(error.to_string());
        }
        info
    }

    fn fake_info(&self) -> AgentInfo {
        let Launch::Program(program) = &self.launch else {
            unreachable!("only the fake has no CLI");
        };
        let installed = program.is_file();
        AgentInfo {
            id: self.id.clone(),
            name: self.name.to_owned(),
            integration: Integration::Acp,
            installed,
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            path: Some(program.display().to_string()),
            login: AgentLogin {
                state: LoginState::LoggedIn,
                account: None,
                detail: Some("scripted; spends no quota".into()),
            },
            models: Vec::new(),
            agent_modes: Vec::new(),
            available: installed,
            problem: None,
            adapter: None,
            default_agent_mode: None,
            detected_at: Timestamp::now(),
        }
    }
}

/// Cursor's install folder: next to the `agent.cmd` that detection finds, when that folder has
/// `versions`, else `%LOCALAPPDATA%\cursor-agent`.
fn cursor_dir() -> Option<PathBuf> {
    detect::CURSOR
        .find()
        .and_then(|shim| shim.parent().map(Path::to_path_buf))
        .filter(|dir| dir.join("versions").is_dir())
        .or_else(launch::cursor_install_dir)
}

impl AgentAdapter for AcpAdapter {
    fn id(&self) -> AgentId {
        self.id.clone()
    }

    fn name(&self) -> &str {
        self.name
    }

    fn integration(&self) -> Integration {
        Integration::Acp
    }

    fn detect(&self) -> BoxFuture<'_, AgentInfo> {
        Box::pin(self.detect_info())
    }

    fn start(&self, config: SessionConfig) -> BoxFuture<'_, Result<StartedSession, AgentError>> {
        Box::pin(async move {
            let command = self.command()?;
            let launcher = Launcher {
                id: self.id.clone(),
                name: self.name,
                default_mode: self.default_mode,
                command,
                cwd: config.cwd.clone(),
                label: format!("{} {}", self.id, config.session_id),
                timing: self.timing,
            };
            session::start(launcher, config).await
        })
    }

    fn install(&self) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            let Launch::Npm(adapter) = &self.launch else {
                return Err(AgentError::InvalidState(format!(
                    "{} needs nothing installed by Burrow: it runs its own ACP server",
                    self.name
                )));
            };
            let _one_at_a_time = self.installing.lock().await;
            let npm = Npm::find().ok_or_else(|| {
                AgentError::Unavailable(
                    "npm isn't on PATH: install Node.js 22 or later, which includes it".into(),
                )
            })?;
            install::install(adapter, &self.agents_dir, &npm).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_agents_report_their_adapter_and_how_to_install_it() {
        let dir = tempfile::tempdir().unwrap();
        let claude = AcpAdapter::claude(dir.path());
        let adapter = claude.npm_adapter().unwrap();
        let info = claude.adapter_info(&adapter);
        assert_eq!(info.package, "@agentclientprotocol/claude-agent-acp");
        assert!(!info.installed);
        assert!(info.path.unwrap().ends_with("0.101.0"));

        // Not installed: unavailable, with the command that fixes it. (Unless Node is missing,
        // which is reported first.)
        let error = claude.command().unwrap_err().to_string();
        if install::find_node().is_some() {
            assert!(error.contains("burrowctl agent install claude"), "{error}");
        }

        // Installed: node runs the entry point.
        let entry = adapter.entry_point(dir.path());
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "").unwrap();
        assert!(claude.adapter_info(&adapter).installed);
        if install::find_node().is_some() {
            let command = claude.command().unwrap();
            assert_eq!(command.args, [entry.into_os_string()]);
        }
    }

    #[test]
    fn default_modes_ask_before_changes() {
        assert_eq!(AcpAdapter::claude("x").default_mode, Some("default"));
        assert_eq!(AcpAdapter::codex("x").default_mode, Some("read-only"));
        assert_eq!(AcpAdapter::cursor().default_mode, None);
        assert_eq!(AcpAdapter::fake("x").id, AgentId::from(FAKE_ACP_ID));
    }

    #[tokio::test]
    async fn cursor_and_the_fake_have_nothing_to_install() {
        let error = AcpAdapter::cursor().install().await.unwrap_err();
        assert!(matches!(error, AgentError::InvalidState(_)), "{error}");
        let error = AcpAdapter::fake("x").install().await.unwrap_err();
        assert!(matches!(error, AgentError::InvalidState(_)), "{error}");
    }
}
