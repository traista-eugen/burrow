//! `burrowd`: Burrow's per-user background service (D-002).
//!
//! It owns all state as append-only event streams (D-005) and serves clients JSON-RPC over the
//! named pipe `\\.\pipe\burrow-<user SID>` (D-013): the connection methods, event
//! subscriptions, projects, agents, sessions and the inbox, plus `debug/append` with `--dev`.
//!
//! How it fits together:
//!
//! - [`server`] accepts clients on the pipe, current user only, one `burrowd` per pipe.
//! - [`connection`] runs each client: the handshake, subscriptions, and one task per request.
//! - [`methods`] dispatches requests to the services below.
//! - [`hub`] is the store thread, the single owner of the event store (D-014), committing events
//!   and entity rows together (D-017).
//! - [`projects`], [`agents`] and [`sessions`] hold current state; [`sessions`] runs agent
//!   sessions through `burrow-agents` adapters and owns the inbox.
//! - [`keep_awake`] keeps Windows awake while sessions work.
//! - [`wire`] maps stored records to `events/event` notifications.
//! - [`logging`] writes `<data dir>\logs\burrowd.<date>.log`.

mod agents;
mod cancel;
mod connection;
mod hub;
mod ids;
mod keep_awake;
mod logging;
mod methods;
mod projects;
#[cfg(windows)]
mod server;
mod sessions;
mod terminals;
mod wire;

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use burrow_agents::acp::FAKE_ACP_ID;
use burrow_protocol::AgentId;
use clap::Parser;

/// Exit code when another `burrowd` already serves the pipe.
const EXIT_ALREADY_RUNNING: u8 = 3;

/// Burrow's per-user background service.
#[derive(Debug, Parser)]
#[command(name = "burrowd", version, about, long_about = None)]
struct Args {
    /// The pipe to serve: a bare name such as `burrow-dev`, or a full path such as
    /// `\\.\pipe\burrow-dev` [default: \\.\pipe\burrow-<your user SID>]
    #[arg(long, value_name = "NAME")]
    pipe: Option<String>,

    /// Where the database (`burrow.db`) and logs (`logs\`) live
    /// [default: %LOCALAPPDATA%\Burrow]
    #[arg(long, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// Development mode: accept development-only methods such as `debug/append`, and offer the
    /// scripted fake agents (`fake`, and `fake-acp`, which speaks ACP)
    #[arg(long)]
    dev: bool,

    /// The agents to offer, comma-separated [default: claude,codex,cursor,muse, plus fake and
    /// fake-acp with --dev]
    #[arg(long, value_delimiter = ',', value_name = "IDS")]
    agents: Option<Vec<String>>,

    /// Where `agent install` puts the ACP adapters Claude Code and Codex run through
    /// [default: %LOCALAPPDATA%\Burrow\agents]
    #[arg(long, value_name = "DIR")]
    agents_dir: Option<PathBuf>,
}

/// The agents offered when `--agents` isn't given.
const DEFAULT_AGENTS: &[&str] = &["claude", "codex", "cursor", "muse"];

/// Why `burrowd` couldn't start or keep running.
enum Fatal {
    AlreadyRunning(String),
    Other(String),
}

impl Fatal {
    fn other(message: impl fmt::Display) -> Self {
        Self::Other(message.to_string())
    }
}

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning(pipe) => {
                write!(f, "already running for this user on {pipe}")
            }
            Self::Other(message) => f.write_str(message),
        }
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(fatal) => {
            eprintln!("burrowd: {fatal}");
            match fatal {
                Fatal::AlreadyRunning(_) => ExitCode::from(EXIT_ALREADY_RUNNING),
                Fatal::Other(_) => ExitCode::FAILURE,
            }
        }
    }
}

fn run(args: Args) -> Result<(), Fatal> {
    let data_dir = match args.data_dir {
        Some(dir) => dir,
        None => std::env::var_os("LOCALAPPDATA")
            .map(|dir| PathBuf::from(dir).join("Burrow"))
            .ok_or_else(|| Fatal::other("%LOCALAPPDATA% isn't set; pass --data-dir"))?,
    };
    std::fs::create_dir_all(&data_dir)
        .map_err(|error| Fatal::other(format!("can't create {}: {error}", data_dir.display())))?;
    logging::init(&data_dir.join("logs"))
        .map_err(|error| Fatal::other(format!("can't start logging: {error}")))?;

    let agents_dir = match args.agents_dir {
        Some(dir) => dir,
        None => std::env::var_os("LOCALAPPDATA")
            .map(|dir| PathBuf::from(dir).join("Burrow").join("agents"))
            .unwrap_or_else(|| data_dir.join("agents")),
    };
    let agents = agent_ids(args.agents.as_deref(), args.dev);
    let result = start(args.pipe, &data_dir, &agents_dir, args.dev, agents);
    match &result {
        Ok(()) => tracing::info!("burrowd stopped"),
        Err(fatal) => tracing::error!("{fatal}"),
    }
    result
}

/// The agents to offer: `--agents`, or the default list. `fake` and `fake-acp` only with
/// `--dev`.
fn agent_ids(requested: Option<&[String]>, dev: bool) -> Vec<AgentId> {
    let mut ids: Vec<AgentId> = match requested {
        Some(ids) => ids
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .map(AgentId::from)
            .collect(),
        None => {
            let mut ids: Vec<AgentId> = DEFAULT_AGENTS.iter().copied().map(AgentId::from).collect();
            if dev {
                ids.push(AgentId::Fake);
                ids.push(AgentId::from(FAKE_ACP_ID));
            }
            ids
        }
    };
    ids.retain(|id| match id {
        AgentId::Fake if !dev => {
            tracing::warn!("the fake agent is offered only with --dev");
            false
        }
        AgentId::Other(other) if other == FAKE_ACP_ID => {
            if !dev {
                tracing::warn!("the fake ACP agent is offered only with --dev");
            }
            dev
        }
        AgentId::Other(other) => {
            tracing::warn!(agent = other, "no such agent; ignoring it");
            false
        }
        _ => true,
    });
    ids
}

fn start(
    pipe: Option<String>,
    data_dir: &Path,
    agents_dir: &Path,
    dev: bool,
    agents: Vec<AgentId>,
) -> Result<(), Fatal> {
    let pipe = match pipe {
        Some(name) => burrow_transport::pipe_path(&name),
        None => burrow_transport::default_pipe_name()
            .map_err(|error| Fatal::other(format!("can't find the current user's SID: {error}")))?,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("burrowd")
        .build()
        .map_err(|error| Fatal::other(format!("can't start the async runtime: {error}")))?;
    runtime.block_on(serve(pipe, data_dir, agents_dir, dev, agents))
}

#[cfg(windows)]
async fn serve(
    pipe: String,
    data_dir: &Path,
    agents_dir: &Path,
    dev: bool,
    agent_ids: Vec<AgentId>,
) -> Result<(), Fatal> {
    use std::sync::Arc;

    use burrow_agents::{Registry, RegistryOptions};
    use burrow_store::Store;
    use tracing::{info, warn};

    use crate::agents::Agents;
    use crate::connection::Context;
    use crate::hub::Hub;
    use crate::keep_awake::KeepAwake;
    use crate::projects::Projects;
    use crate::server::{BindError, Listener};
    use crate::sessions::Sessions;
    use crate::terminals::Terminals;

    // The pipe first: it is what makes this the only burrowd for the pipe, so a second one exits
    // before it touches the database.
    let listener = Listener::bind(&pipe).await.map_err(|error| match error {
        BindError::AlreadyRunning => Fatal::AlreadyRunning(pipe.clone()),
        other => Fatal::other(format!("{pipe}: {other}")),
    })?;
    let db = data_dir.join("burrow.db");
    let store = Store::open(&db)
        .map_err(|error| Fatal::other(format!("can't open {}: {error}", db.display())))?;
    let hub = Hub::start(store)
        .map_err(|error| Fatal::other(format!("can't start the store thread: {error}")))?;

    let exe = std::env::current_exe().ok();
    let next_to_exe = |program: &str| {
        exe.as_ref()
            .map(|exe| exe.with_file_name(format!("{program}{}", std::env::consts::EXE_SUFFIX)))
    };
    let options = RegistryOptions {
        fake_program: next_to_exe(burrow_agents::fake::PROGRAM),
        fake_acp_program: next_to_exe(burrow_agents::acp::FAKE_ACP_PROGRAM),
        agents_dir: agents_dir.to_owned(),
    };
    let registry = Registry::standard(&agent_ids, &options);
    let agents = Arc::new(Agents::new(registry));
    let projects = Arc::new(
        Projects::load(hub.clone())
            .await
            .map_err(|error| Fatal::other(format!("can't load projects: {error}")))?,
    );
    let keep_awake = KeepAwake::start()
        .map_err(|error| Fatal::other(format!("can't start the keep-awake thread: {error}")))?;
    // Marks the sessions that were live when the last burrowd stopped as stopped, before any
    // client can see them.
    let terminals = Terminals::new();
    let sessions = Sessions::load(
        hub.clone(),
        agents.clone(),
        projects.clone(),
        keep_awake,
        terminals.clone(),
    )
    .await
    .map_err(|error| Fatal::other(format!("can't load sessions: {error}")))?;
    agents.detect_in_background();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        protocol = burrow_protocol::PROTOCOL_VERSION,
        pipe = listener.path(),
        data_dir = %data_dir.display(),
        agents_dir = %agents_dir.display(),
        dev,
        agents = ?agent_ids,
        "burrowd is listening"
    );
    if dev {
        warn!("development mode: development-only methods such as debug/append are enabled");
    }

    let context = Arc::new(Context {
        hub,
        dev,
        projects,
        agents,
        sessions: sessions.clone(),
        terminals,
    });
    tokio::select! {
        () = server::serve(listener, context) => {}
        () = shutdown_signal() => info!("Ctrl+C: shutting down"),
    }
    sessions.shutdown().await;
    Ok(())
}

#[cfg(not(windows))]
async fn serve(
    _pipe: String,
    _data_dir: &Path,
    _agents_dir: &Path,
    _dev: bool,
    _agents: Vec<AgentId>,
) -> Result<(), Fatal> {
    Err(Fatal::other("burrowd runs only on Windows for now"))
}

/// Resolves on Ctrl+C. If the handler can't be installed, never resolves.
#[cfg(windows)]
async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::warn!(%error, "can't listen for Ctrl+C");
        std::future::pending::<()>().await;
    }
}
