//! `burrowd`: Burrow's per-user background service (D-002).
//!
//! It owns all state as append-only event streams (D-005) and serves clients JSON-RPC over the
//! named pipe `\\.\pipe\burrow-<user SID>` (D-013). In M1 it serves the connection methods,
//! event subscriptions and, with `--dev`, `debug/append`.
//!
//! How it fits together:
//!
//! - [`server`] accepts clients on the pipe, current user only, one `burrowd` per pipe.
//! - [`connection`] runs each client: the handshake, requests, subscriptions (one task each).
//! - [`hub`] is the store thread, the single owner of the event store (D-014).
//! - [`wire`] maps stored records to `events/event` notifications.
//! - [`logging`] writes `<data dir>\logs\burrowd.<date>.log`.

mod connection;
mod hub;
mod ids;
mod logging;
#[cfg(windows)]
mod server;
mod wire;

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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

    /// Development mode: accept development-only methods such as `debug/append`
    #[arg(long)]
    dev: bool,
}

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

    let result = start(args.pipe, &data_dir, args.dev);
    match &result {
        Ok(()) => tracing::info!("burrowd stopped"),
        Err(fatal) => tracing::error!("{fatal}"),
    }
    result
}

fn start(pipe: Option<String>, data_dir: &Path, dev: bool) -> Result<(), Fatal> {
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
    runtime.block_on(serve(pipe, data_dir, dev))
}

#[cfg(windows)]
async fn serve(pipe: String, data_dir: &Path, dev: bool) -> Result<(), Fatal> {
    use std::sync::Arc;

    use burrow_store::Store;
    use tracing::{info, warn};

    use crate::connection::Context;
    use crate::hub::Hub;
    use crate::server::{BindError, Listener};

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

    info!(
        version = env!("CARGO_PKG_VERSION"),
        protocol = burrow_protocol::PROTOCOL_VERSION,
        pipe = listener.path(),
        data_dir = %data_dir.display(),
        dev,
        "burrowd is listening"
    );
    if dev {
        warn!("development mode: development-only methods such as debug/append are enabled");
    }

    let context = Arc::new(Context { hub, dev });
    tokio::select! {
        () = server::serve(listener, context) => {}
        () = shutdown_signal() => info!("Ctrl+C: shutting down"),
    }
    Ok(())
}

#[cfg(not(windows))]
async fn serve(_pipe: String, _data_dir: &Path, _dev: bool) -> Result<(), Fatal> {
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
