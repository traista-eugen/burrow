//! Starts an ACP agent, runs `initialize` and `session/new` (no prompt, so no
//! model call), prints what the agent reports, and how long each step took.
//!
//! ```text
//! cargo run -p burrow-acp --example probe -- <claude|codex|cursor> [--npx] [--agents-dir DIR]
//! ```
//!
//! `--agents-dir` is where the npm adapters are installed (default:
//! `BURROW_ACP_AGENTS_DIR`, else `%LOCALAPPDATA%\Burrow\agents`). `--npx`
//! starts the npm adapters through `npx -y` instead.

use std::path::PathBuf;
use std::time::Instant;

use burrow_acp::launch::{self, KnownAgent, NpmAdapter};
use burrow_acp::{AcpClient, ConnectOptions};
use tokio::io::AsyncBufReadExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let agent = match args.next().as_deref() {
        Some("claude") => KnownAgent::ClaudeCode,
        Some("codex") => KnownAgent::Codex,
        Some("cursor") => KnownAgent::Cursor,
        _ => return Err("usage: probe <claude|codex|cursor> [--npx] [--agents-dir DIR]".into()),
    };
    let mut npx = false;
    let mut agents_dir = std::env::var_os("BURROW_ACP_AGENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default())
                .join("Burrow")
                .join("agents")
        });
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--npx" => npx = true,
            "--agents-dir" => agents_dir = args.next().ok_or("--agents-dir needs a value")?.into(),
            other => return Err(format!("unknown argument {other}").into()),
        }
    }

    let command = if npx {
        let adapter = NpmAdapter::for_agent(agent).ok_or("--npx is for the npm adapters")?;
        adapter.npx_command(&launch::find_on_path("npx").ok_or("npx isn't on PATH")?)
    } else {
        launch::recommended_command(agent, &agents_dir).ok_or("the agent isn't installed")?
    };
    let cwd = tempfile::tempdir()?;
    println!("command: {command}");

    let started = Instant::now();
    let spawned = launch::spawn(&command, cwd.path())?;
    let stderr = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(spawned.stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[agent stderr] {line}");
        }
    });
    let mut child = spawned.child;
    let (client, _events) =
        AcpClient::connect_tokio(spawned.stdin, spawned.stdout, ConnectOptions::default()).await?;
    let initialized = started.elapsed();
    let info = client.agent();
    println!(
        "initialize: {initialized:?}\nagent: {:?} {:?}\ncapabilities: {:#?}\nauth methods: {:#?}\nraw: {}",
        info.name,
        info.version,
        info.capabilities,
        info.auth_methods,
        serde_json::to_string_pretty(&info.raw)?
    );

    let before = Instant::now();
    match client.new_session(cwd.path()).await {
        Ok(session) => {
            println!(
                "session/new: {:?}\nmodes: {:#?}\nmodels: {:#?}\nconfig options: {}",
                before.elapsed(),
                session.modes,
                session.models.as_ref().map(|models| (
                    &models.current,
                    &models.config_id,
                    models.available.iter().map(|m| &m.id).collect::<Vec<_>>()
                )),
                serde_json::to_string_pretty(&session.raw["configOptions"])?
            );
            if session.raw.get("models").is_some() {
                println!("legacy models field: present");
            }
        }
        Err(error) => println!("session/new failed: {error:?}"),
    }

    client.stop().await;
    let exit = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait()).await;
    println!("exit after stop: {exit:?}");
    if exit.is_err() {
        child.kill().await?;
    }
    stderr.abort();
    Ok(())
}
