//! The MSP adapter: Muse Code sessions over `muse serve`, through `burrow-msp` (D-026).
//!
//! # Starting
//!
//! [`MspAdapter::start`] runs Muse's versioned binary, not its `muse.cmd` launcher (D-023), as
//! `muse-bin-<version>.exe serve --disable-request-user-input`, through
//! [`launcher::spawn`](crate::launcher::spawn), so it runs in a Job Object like every agent. It
//! hands the child's stdio to [`MspClient`], opens a session in [`SessionConfig::cwd`] with
//! `session/start`, and reads the model list and the subscription usage.
//!
//! - **User input.** Muse starts with `--disable-request-user-input`: its model can't ask the
//!   user questions, which Burrow can't show yet (D-027). A question that arrives anyway is
//!   declined at once.
//! - **The sandbox.** Muse's shell tool fails on Windows until `muse sandbox windows setup` has
//!   run in an elevated terminal. Burrow doesn't pass `--disable-sandbox`; [`MspAdapter::detect`]
//!   reports the missing setup as the agent's `problem` instead (D-027).
//! - **Tests.** `BURROW_MUSE_PROVIDER` (see [`PROVIDER_ENV`]) picks Muse's provider, for example
//!   `echo`, which is free; sessions are then memory-only (`--no-session-log`).
//!   `BURROW_MUSE_DISABLE_SANDBOX=1` ([`NO_SANDBOX_ENV`]) turns the sandbox off, so that Muse
//!   asks before shell commands and a live test has an approval to answer.
//!
//! # Running
//!
//! [`session`] holds the command handle and the loop that reads MSP's events; [`translate`]
//! maps them to [`AgentEvent`](crate::AgentEvent)s: text, reasoning, tool calls with diffs,
//! approvals as permission requests, token usage, limits and the turn's end.

mod session;
mod translate;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::Duration;

use burrow_msp::{ConnectOptions, MspClient, MuseCommand, SandboxAvailability};
use burrow_protocol::{
    AgentId, AgentInfo, AgentLogin, Integration, LoginState, ModelInfo, Timestamp,
};
use futures::future::BoxFuture;
use tracing::{debug, warn};

use crate::adapter::{AgentAdapter, AgentError, SessionConfig, StartedSession};
use crate::detect;
use crate::launcher::{self, LaunchSpec};

/// The environment variable that picks Muse's provider, for tests: `echo` is free and needs no
/// login. Unset, Muse uses its default (`meta`) and keeps its sessions on disk; set, sessions
/// are memory-only.
pub const PROVIDER_ENV: &str = "BURROW_MUSE_PROVIDER";

/// The environment variable that, set to `1`, starts Muse with `--disable-sandbox`. For tests
/// only, and logged as a warning: Burrow never disables the sandbox by itself (D-027). With
/// the sandbox off, Muse asks before every shell command its policy doesn't already allow, so a
/// live test gets an approval to answer; with a working sandbox it runs them sandboxed without
/// asking.
pub const NO_SANDBOX_ENV: &str = "BURROW_MUSE_DISABLE_SANDBOX";

/// What `problem` says while Muse's Windows sandbox isn't set up.
pub const SANDBOX_PROBLEM: &str = "Muse's shell tool won't run until its Windows sandbox is set up: run `muse sandbox windows setup` in an elevated terminal. Its other tools work.";

/// How long one detection command may take.
const DETECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long `muse serve` gets to answer `initialize` when a session starts.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Muse Code, through MSP.
#[derive(Clone, Copy, Debug, Default)]
pub struct MspAdapter;

impl AgentAdapter for MspAdapter {
    fn id(&self) -> AgentId {
        AgentId::Muse
    }

    fn name(&self) -> &str {
        detect::MUSE.name
    }

    fn integration(&self) -> Integration {
        Integration::Msp
    }

    fn detect(&self) -> BoxFuture<'_, AgentInfo> {
        Box::pin(detect_muse())
    }

    fn start(&self, config: SessionConfig) -> BoxFuture<'_, Result<StartedSession, AgentError>> {
        Box::pin(start(config))
    }
}

/// Muse's versioned binary, which its `muse.cmd` launcher would run (D-023): found on `PATH` or
/// in `%LOCALAPPDATA%\Programs\muse`. The launcher itself if the layout isn't there.
pub fn muse_binary() -> Option<PathBuf> {
    let found = detect::MUSE.find()?;
    Some(burrow_msp::launcher_binary(&found).unwrap_or(found))
}

/// The provider `BURROW_MUSE_PROVIDER` asks for, if any.
fn provider_override() -> Option<String> {
    std::env::var(PROVIDER_ENV)
        .ok()
        .map(|provider| provider.trim().to_owned())
        .filter(|provider| !provider.is_empty())
}

/// `muse serve` as Burrow runs it for sessions.
fn serve_command(program: &Path) -> MuseCommand {
    let mut command = MuseCommand::new(program).disable_request_user_input();
    if let Some(provider) = provider_override() {
        command = command.provider(provider).no_session_log();
    }
    if std::env::var(NO_SANDBOX_ENV).is_ok_and(|value| value.trim() == "1") {
        warn!("{NO_SANDBOX_ENV}=1: Muse's shell commands run without its sandbox");
        command = command.disable_sandbox();
    }
    command
}

fn launch_spec(command: &MuseCommand, label: String) -> LaunchSpec {
    LaunchSpec::new(command.program(), command.args().iter().cloned(), label)
}

async fn start(config: SessionConfig) -> Result<StartedSession, AgentError> {
    let program = muse_binary().ok_or_else(|| {
        AgentError::Unavailable("Muse Code isn't installed: `muse` isn't on PATH".into())
    })?;
    let command = serve_command(&program);
    let spec = launch_spec(&command, format!("muse {}", config.session_id)).cwd(&config.cwd);
    let mut process = launcher::spawn(&spec).map_err(|error| {
        AgentError::Unavailable(format!("can't start {}: {error}", program.display()))
    })?;
    let handle = process.handle();
    let stdin = process.take_stdin().expect("stdin is piped");
    let stdout = process.take_stdout().expect("stdout is piped");
    // If this future is dropped from here on, `process` and `handle` go with it, the job's last
    // handle closes, and the whole tree dies.
    let options = ConnectOptions {
        handshake_timeout: HANDSHAKE_TIMEOUT,
        ..ConnectOptions::default()
    };
    let (client, events) = match MspClient::connect(stdout, stdin, options).await {
        Ok(connected) => connected,
        Err(error) => {
            let exit = tokio::time::timeout(session::STOP_GRACE, handle.exited()).await;
            let exit = exit.map_or_else(|_| "still running".to_owned(), |e| e.description);
            handle.kill();
            return Err(AgentError::Unavailable(format!(
                "`muse serve` didn't start ({exit}): {error}. {}",
                handle.stderr_tail().join(" | ")
            )));
        }
    };
    if !client.schema_matches() {
        warn!(
            muse = client.server().server_info.version,
            "this Muse speaks a newer MSP schema than burrow-msp was written against"
        );
    }
    drop(process);
    session::open(client, events, Some(handle), &config).await
}

/// What a throwaway `muse serve --provider echo` says: the sandbox's state and the model
/// catalog. Echo spends nothing and needs no login.
#[derive(Debug)]
struct Probe {
    version: String,
    sandbox: Option<SandboxAvailability>,
    models: Vec<ModelInfo>,
}

async fn probe(program: &Path) -> Result<Probe, String> {
    let command = MuseCommand::new(program)
        .provider("echo")
        .no_session_log()
        .disable_request_user_input();
    let mut process = launcher::spawn(&launch_spec(&command, "muse serve (detection)".into()))
        .map_err(|error| error.to_string())?;
    let handle = process.handle();
    let stdin = process.take_stdin().expect("stdin is piped");
    let stdout = process.take_stdout().expect("stdout is piped");
    let asked = tokio::time::timeout(DETECT_TIMEOUT, async {
        let (client, _events) = MspClient::connect(stdout, stdin, ConnectOptions::default())
            .await
            .map_err(|error| error.to_string())?;
        let server = client.server().clone();
        let models = match client.list_models(None).await {
            Ok(list) => session::models(&list, Some(&list.provider_id)),
            Err(error) => {
                debug!(%error, "model/list failed");
                Vec::new()
            }
        };
        client.shutdown().await;
        Ok(Probe {
            version: server.server_info.version,
            sandbox: server.sandbox_availability,
            models,
        })
    })
    .await;
    handle.stop(Duration::from_secs(2)).await;
    asked.unwrap_or_else(|_| Err("`muse serve` didn't answer in time".into()))
}

/// Detects Muse: its version and login from the binary's `--version` and `usage`, and its
/// sandbox and models from a throwaway echo host. Spends no quota.
async fn detect_muse() -> AgentInfo {
    let mut info = AgentInfo {
        id: AgentId::Muse,
        name: detect::MUSE.name.to_owned(),
        integration: Integration::Msp,
        installed: false,
        version: None,
        path: None,
        login: AgentLogin {
            state: LoginState::Unknown,
            account: None,
            detail: None,
        },
        models: Vec::new(),
        available: false,
        problem: None,
        detected_at: Timestamp::now(),
    };
    let Some(program) = muse_binary() else {
        info.problem = Some("`muse` isn't on PATH".into());
        return info;
    };
    info.installed = true;
    info.path = Some(program.display().to_string());

    let run = |args: &'static [&'static str]| {
        let spec = LaunchSpec::new(&program, args.iter().copied(), format!("muse {}", args[0]));
        async move { launcher::run_capture(&spec, DETECT_TIMEOUT).await }
    };
    let (version, usage, probed) =
        tokio::join!(run(&["--version"]), run(&["usage"]), probe(&program));

    info.version = version
        .ok()
        .and_then(|output| detect::parse_version(&output.text()));
    info.login = match usage {
        Ok(output) => detect::parse_muse_usage(&output.text()),
        Err(error) => AgentLogin {
            state: LoginState::Unknown,
            account: None,
            detail: Some(format!("checking the login failed: {error}")),
        },
    };
    let mut problems = Vec::new();
    if info.login.state == LoginState::LoggedOut {
        problems.push("Not logged in: run `muse` and log in.".to_owned());
    }
    match probed {
        Ok(probe) => {
            info.version.get_or_insert(probe.version);
            info.models = probe.models;
            if probe.sandbox != Some(SandboxAvailability::Available) {
                problems.push(SANDBOX_PROBLEM.to_owned());
            }
            info.available = info.login.state != LoginState::LoggedOut;
        }
        Err(error) => problems.push(format!("`muse serve` doesn't work: {error}.")),
    }
    info.problem = (!problems.is_empty()).then(|| problems.join(" "));
    info
}
