//! `burrowctl`: a command-line client for `burrowd`, for scripting and trying things by hand.
//!
//! ```text
//! burrowctl status
//! burrowctl events tail index --after 0
//! burrowctl debug append index '{"type":"entityCreated","entity":"session","id":"ses_1"}'
//! ```

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use burrow_client::{CallError, Client, ConnectError, ConnectOptions};
use burrow_protocol::methods::{DebugAppend, Ping};
use burrow_protocol::{
    ClientInfo, ClientKind, DebugAppendParams, Empty, Event, StreamName, SubscribeParams, code,
};
use clap::{Parser, Subcommand};

/// How often `events tail` tries to reconnect after losing burrowd.
const RECONNECT_INTERVAL: Duration = Duration::from_millis(500);

/// Command-line client for burrowd.
#[derive(Debug, Parser)]
#[command(name = "burrowctl", version, about, long_about = None)]
struct Cli {
    /// The pipe burrowd serves: a bare name such as `burrow-dev`, or a full path
    /// [default: the current user's pipe, \\.\pipe\burrow-<user SID>]
    #[arg(long, global = true, env = "BURROW_PIPE", value_name = "NAME")]
    pipe: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Connect, initialize, and print what burrowd reports about itself
    Status,
    /// Event streams
    #[command(subcommand)]
    Events(EventsCommand),
    /// Development helpers; burrowd must run with --dev
    #[command(subcommand)]
    Debug(DebugCommand),
}

#[derive(Debug, Subcommand)]
enum EventsCommand {
    /// Follow a stream, printing each event as one line of JSON. If burrowd restarts, reconnects
    /// and carries on from the last event printed. Stop with Ctrl+C.
    Tail {
        /// The stream: `index`, `session:<id>` or `run:<id>`
        stream: String,
        /// First replay the events after this seq (0 for the whole history). Without it, only
        /// new events are printed.
        #[arg(long, value_name = "SEQ")]
        after: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
enum DebugCommand {
    /// Append an event to a stream with `debug/append`, and print its seq
    Append {
        /// The stream: `index`, `session:<id>`, `run:<id>`, or any other name
        stream: String,
        /// The event: a JSON object with a `type`, or `-` to read it from stdin
        event: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let pipe = cli.pipe.as_deref();
    let result = match cli.command {
        Command::Status => status(pipe).await,
        Command::Events(EventsCommand::Tail { stream, after }) => tail(pipe, stream, after).await,
        Command::Debug(DebugCommand::Append { stream, event }) => {
            debug_append(pipe, stream, &event).await
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("burrowctl: {message}");
            ExitCode::FAILURE
        }
    }
}

fn options(pipe: Option<&str>) -> ConnectOptions {
    let options = ConnectOptions::new(ClientInfo {
        name: "burrowctl".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        kind: ClientKind::Cli,
    });
    match pipe {
        Some(pipe) => options.pipe(pipe),
        None => options,
    }
}

async fn connect(pipe: Option<&str>) -> Result<Client, String> {
    Client::connect(options(pipe))
        .await
        .map_err(|error| match error {
            ConnectError::NotRunning { pipe } => {
                format!("burrowd isn't running: nothing serves {pipe}. Start it with `burrowd`.")
            }
            other => other.to_string(),
        })
}

async fn status(pipe: Option<&str>) -> Result<(), String> {
    let client = connect(pipe).await?;
    let started = Instant::now();
    client
        .call::<Ping>(&Empty {})
        .await
        .map_err(|error| error.to_string())?;
    let ping = started.elapsed();

    let info = client.server();
    println!(
        "{} {} (protocol {})",
        info.server.name, info.server.version, info.protocol_version
    );
    println!("pipe:         {}", client.pipe());
    println!("connection:   {}", info.connection_id);
    println!(
        "capabilities: flows={} remote={}",
        info.capabilities.flows, info.capabilities.remote
    );
    println!("ping:         {:.2} ms", ping.as_secs_f64() * 1000.0);
    Ok(())
}

async fn tail(pipe: Option<&str>, stream: String, after: Option<u64>) -> Result<(), String> {
    let mut client = connect(pipe).await?;
    let mut params = SubscribeParams {
        stream: StreamName::new(stream),
        after,
    };
    let mut stdout = io::stdout().lock();
    loop {
        match client.subscribe_with(params.clone()).await {
            Ok(mut subscription) => {
                if subscription.reset() {
                    eprintln!(
                        "burrowctl: {} has no events after seq {}; following from its head, seq {}",
                        params.stream,
                        params.after.unwrap_or_default(),
                        subscription.head()
                    );
                }
                while let Some(event) = subscription.next().await {
                    let line = serde_json::to_string(&event).map_err(|error| error.to_string())?;
                    if writeln!(stdout, "{line}")
                        .and_then(|()| stdout.flush())
                        .is_err()
                    {
                        // Whoever reads our output has gone away.
                        return Ok(());
                    }
                }
                // The subscription ends when the connection does.
                params = subscription.resume_params();
            }
            // burrowd went away before answering; reconnect as below.
            Err(CallError::Disconnected) => {}
            Err(error) => return Err(error.to_string()),
        }

        eprintln!("burrowctl: lost the connection to burrowd; reconnecting...");
        client = reconnect(pipe).await;
        eprintln!(
            "burrowctl: reconnected; resuming after seq {}",
            params.after.unwrap_or_default()
        );
    }
}

/// Tries to connect until it works.
async fn reconnect(pipe: Option<&str>) -> Client {
    loop {
        tokio::time::sleep(RECONNECT_INTERVAL).await;
        match Client::connect(options(pipe)).await {
            Ok(client) => return client,
            Err(ConnectError::NotRunning { .. } | ConnectError::Busy { .. }) => {}
            Err(error) => eprintln!("burrowctl: {error}"),
        }
    }
}

async fn debug_append(pipe: Option<&str>, stream: String, event: &str) -> Result<(), String> {
    let json = if event == "-" {
        let mut json = String::new();
        io::stdin()
            .read_to_string(&mut json)
            .map_err(|error| format!("can't read the event from stdin: {error}"))?;
        json
    } else {
        event.to_owned()
    };
    let event: Event = serde_json::from_str(&json)
        .map_err(|error| format!("the event must be a JSON object with a \"type\": {error}"))?;

    let client = connect(pipe).await?;
    let params = DebugAppendParams {
        stream: StreamName::new(stream),
        event,
    };
    let result = client
        .call::<DebugAppend>(&params)
        .await
        .map_err(|error| match error {
            CallError::Rpc(error) if error.code == code::METHOD_NOT_FOUND => {
                "burrowd doesn't accept debug/append: start it with --dev".to_owned()
            }
            other => other.to_string(),
        })?;
    println!(
        "{}",
        serde_json::to_string(&result).map_err(|error| error.to_string())?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_pipe_flag_works_after_the_subcommand() {
        let cli =
            Cli::try_parse_from(["burrowctl", "events", "tail", "index", "--pipe", "x"]).unwrap();
        assert_eq!(cli.pipe.as_deref(), Some("x"));
        let Command::Events(EventsCommand::Tail { stream, after }) = cli.command else {
            panic!("parsed the wrong command");
        };
        assert_eq!((stream.as_str(), after), ("index", None));
    }
}
