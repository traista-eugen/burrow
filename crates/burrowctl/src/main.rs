//! `burrowctl`: a command-line client for `burrowd`, for scripting and trying things by hand.
//!
//! ```text
//! burrowctl status
//! burrowctl agent list
//! burrowctl session new --agent fake
//! burrowctl session prompt ses_… "Hello" --watch
//! burrowctl inbox respond inb_… allow
//! burrowctl events tail index --after 0
//! ```
//!
//! Output is readable text; `--json` prints JSON instead, one value per line.

mod render;

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use burrow_client::{CallError, Client, ConnectError, ConnectOptions};
use burrow_protocol::methods::{
    AgentList, AgentRefresh, DebugAppend, InboxList, InboxRespond, Ping, ProjectAdd, ProjectList,
    SessionArchive, SessionCancel, SessionCreate, SessionList, SessionPrompt, SessionSetModel,
    SessionStop, UsageGet,
};
use burrow_protocol::{
    AgentId, ClientInfo, ClientKind, ContentBlock, DebugAppendParams, Empty, Event,
    InboxItemStatus, InboxListParams, InboxRespondParams, ProjectAddParams, SessionCreateParams,
    SessionIdParams, SessionListParams, SessionMode, SessionPromptParams, SessionSetModelParams,
    StreamName, SubscribeParams, code,
};
use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::render::Transcript;

/// How often `events tail` and `session watch` try to reconnect after losing burrowd.
const RECONNECT_INTERVAL: Duration = Duration::from_millis(500);

/// Command-line client for burrowd.
#[derive(Debug, Parser)]
#[command(name = "burrowctl", version, about, long_about = None)]
struct Cli {
    /// The pipe burrowd serves: a bare name such as `burrow-dev`, or a full path
    /// [default: the current user's pipe, \\.\pipe\burrow-<user SID>]
    #[arg(long, global = true, env = "BURROW_PIPE", value_name = "NAME")]
    pipe: Option<String>,

    /// Print JSON, one value per line, instead of readable text
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Connect, initialize, and print what burrowd reports about itself
    Status,
    /// Projects: folders Burrow works in
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Agents: what is installed and logged in
    #[command(subcommand)]
    Agent(AgentCommand),
    /// Usage and limits each provider reported
    Usage,
    /// Agent sessions
    #[command(subcommand)]
    Session(SessionCommand),
    /// Things that wait on you, such as permission requests
    #[command(subcommand)]
    Inbox(InboxCommand),
    /// Event streams
    #[command(subcommand)]
    Events(EventsCommand),
    /// Development helpers; burrowd must run with --dev
    #[command(subcommand)]
    Debug(DebugCommand),
}

#[derive(Debug, Subcommand)]
enum ProjectCommand {
    /// Make a folder a project, detecting its git origin remote
    Add {
        /// The folder; relative paths are taken from the current directory
        path: PathBuf,
    },
    /// List projects
    List,
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// List the agents burrowd offers: version, login state, availability
    List {
        /// Detect them again first, for example after installing or logging in
        #[arg(long)]
        refresh: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// Start a session and wait until the agent is ready; prints its id
    New {
        /// The agent: claude, codex, cursor, muse, or fake (with burrowd --dev)
        #[arg(long)]
        agent: String,
        /// The model; the agent's default without it
        #[arg(long)]
        model: Option<String>,
        /// The folder the agent works in [default: the project's folder, else the current
        /// directory]
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// The project to work in
        #[arg(long, value_name = "PROJECT_ID")]
        project: Option<String>,
        /// A title; otherwise the first prompt's first line
        #[arg(long)]
        title: Option<String>,
    },
    /// List sessions, newest first
    List {
        /// Include archived sessions
        #[arg(long)]
        all: bool,
    },
    /// Send a prompt; prints the turn's id, or the turn itself with --watch
    Prompt {
        /// The session
        id: String,
        /// The prompt
        text: String,
        /// Print the turn as it happens, until it ends
        #[arg(long)]
        watch: bool,
    },
    /// Follow a session's events. Stop with Ctrl+C
    Watch {
        /// The session
        id: String,
        /// Replay the events after this seq first [default: 0, the whole history]
        #[arg(long, value_name = "SEQ")]
        after: Option<u64>,
    },
    /// Stop the running turn; the session stays open
    Cancel {
        /// The session
        id: String,
    },
    /// Switch the session's model
    Model {
        /// The session
        id: String,
        /// The model
        model: String,
    },
    /// End the agent process and everything it started
    Stop {
        /// The session
        id: String,
    },
    /// Stop the session if needed and hide it from lists
    Archive {
        /// The session
        id: String,
    },
}

#[derive(Debug, Subcommand)]
enum InboxCommand {
    /// List open inbox items, newest first
    List {
        /// Include resolved and expired items
        #[arg(long)]
        all: bool,
    },
    /// Answer an inbox item with one of its actions
    Respond {
        /// The inbox item
        id: String,
        /// The action, for example `allow`
        action: String,
    },
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
    let ctl = Ctl {
        pipe: cli.pipe,
        json: cli.json,
    };
    let result = match cli.command {
        Command::Status => ctl.status().await,
        Command::Project(command) => ctl.project(command).await,
        Command::Agent(AgentCommand::List { refresh }) => ctl.agents(refresh).await,
        Command::Usage => ctl.usage().await,
        Command::Session(command) => ctl.session(command).await,
        Command::Inbox(command) => ctl.inbox(command).await,
        Command::Events(EventsCommand::Tail { stream, after }) => ctl.tail(stream, after).await,
        Command::Debug(DebugCommand::Append { stream, event }) => {
            ctl.debug_append(stream, &event).await
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

/// The global options, and the commands.
struct Ctl {
    pipe: Option<String>,
    json: bool,
}

impl Ctl {
    fn options(&self) -> ConnectOptions {
        let options = ConnectOptions::new(ClientInfo {
            name: "burrowctl".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            kind: ClientKind::Cli,
        });
        match &self.pipe {
            Some(pipe) => options.pipe(pipe),
            None => options,
        }
    }

    async fn connect(&self) -> Result<Client, String> {
        Client::connect(self.options())
            .await
            .map_err(|error| match error {
                ConnectError::NotRunning { pipe } => format!(
                    "burrowd isn't running: nothing serves {pipe}. Start it with `burrowd`."
                ),
                other => other.to_string(),
            })
    }

    /// Tries to connect until it works.
    async fn reconnect(&self) -> Client {
        loop {
            tokio::time::sleep(RECONNECT_INTERVAL).await;
            match Client::connect(self.options()).await {
                Ok(client) => return client,
                Err(ConnectError::NotRunning { .. } | ConnectError::Busy { .. }) => {}
                Err(error) => eprintln!("burrowctl: {error}"),
            }
        }
    }

    /// Prints `value` as JSON when `--json` is on, else the readable text.
    fn print<T: Serialize>(&self, value: &T, text: impl FnOnce() -> String) -> Result<(), String> {
        if self.json {
            println!("{}", to_json(value)?);
        } else {
            println!("{}", text());
        }
        Ok(())
    }

    /// Prints each item as a JSON line, or as readable lines; `empty` when there are none.
    fn print_list<T: Serialize>(
        &self,
        items: &[T],
        line: impl Fn(&T) -> String,
        empty: &str,
    ) -> Result<(), String> {
        if self.json {
            for item in items {
                println!("{}", to_json(item)?);
            }
        } else if items.is_empty() {
            eprintln!("{empty}");
        } else {
            for item in items {
                println!("{}", line(item));
            }
        }
        Ok(())
    }

    async fn status(&self) -> Result<(), String> {
        let client = self.connect().await?;
        let started = Instant::now();
        client
            .call::<Ping>(&Empty {})
            .await
            .map_err(|error| error.to_string())?;
        let ping = started.elapsed();

        let info = client.server();
        if self.json {
            println!("{}", to_json(info)?);
            return Ok(());
        }
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

    async fn project(&self, command: ProjectCommand) -> Result<(), String> {
        let client = self.connect().await?;
        match command {
            ProjectCommand::Add { path } => {
                let path = absolute(&path)?;
                let project = client
                    .call::<ProjectAdd>(&ProjectAddParams {
                        path: path.display().to_string(),
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                self.print(&project, || render::project(&project))
            }
            ProjectCommand::List => {
                let projects = client
                    .call::<ProjectList>(&Empty {})
                    .await
                    .map_err(|error| error.to_string())?;
                self.print_list(&projects, render::project, "no projects")
            }
        }
    }

    async fn agents(&self, refresh: bool) -> Result<(), String> {
        let client = self.connect().await?;
        let agents = if refresh {
            client.call::<AgentRefresh>(&Empty {}).await
        } else {
            client.call::<AgentList>(&Empty {}).await
        }
        .map_err(|error| error.to_string())?;
        self.print_list(&agents, render::agent, "burrowd offers no agents")
    }

    async fn usage(&self) -> Result<(), String> {
        let client = self.connect().await?;
        let usage = client
            .call::<UsageGet>(&Empty {})
            .await
            .map_err(|error| error.to_string())?;
        self.print_list(&usage, render::usage, "no provider has reported usage yet")
    }

    async fn session(&self, command: SessionCommand) -> Result<(), String> {
        let client = self.connect().await?;
        match command {
            SessionCommand::New {
                agent,
                model,
                cwd,
                project,
                title,
            } => {
                let cwd =
                    match (cwd, &project) {
                        (Some(cwd), _) => Some(absolute(&cwd)?),
                        (None, Some(_)) => None,
                        (None, None) => Some(std::env::current_dir().map_err(|error| {
                            format!("can't read the current directory: {error}")
                        })?),
                    };
                let params = SessionCreateParams {
                    project_id: project,
                    agent: AgentId::from(agent),
                    model,
                    mode: SessionMode::Structured,
                    cwd: cwd.map(|cwd| cwd.display().to_string()),
                    title,
                };
                let session = client
                    .call::<SessionCreate>(&params)
                    .await
                    .map_err(|error| error.to_string())?;
                self.print(&session, || render::session(&session))
            }
            SessionCommand::List { all } => {
                let sessions = client
                    .call::<SessionList>(&SessionListParams {
                        include_archived: all,
                        ..SessionListParams::default()
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                self.print_list(&sessions, render::session_summary, "no sessions")
            }
            SessionCommand::Prompt { id, text, watch } => {
                self.prompt(&client, id, text, watch).await
            }
            SessionCommand::Watch { id, after } => self.watch(client, id, after).await,
            SessionCommand::Cancel { id } => {
                call_empty::<SessionCancel>(&client, SessionIdParams { session_id: id }).await
            }
            SessionCommand::Model { id, model } => {
                call_empty::<SessionSetModel>(
                    &client,
                    SessionSetModelParams {
                        session_id: id,
                        model,
                    },
                )
                .await
            }
            SessionCommand::Stop { id } => {
                call_empty::<SessionStop>(&client, SessionIdParams { session_id: id }).await
            }
            SessionCommand::Archive { id } => {
                call_empty::<SessionArchive>(&client, SessionIdParams { session_id: id }).await
            }
        }
    }

    /// `session prompt`: with `--watch`, subscribes first so nothing of the turn is missed, then
    /// prints its events until it ends.
    async fn prompt(
        &self,
        client: &Client,
        id: String,
        text: String,
        watch: bool,
    ) -> Result<(), String> {
        let stream = StreamName::session(&id);
        let mut subscription = if watch {
            Some(
                client
                    .subscribe(stream, None)
                    .await
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let result = client
            .call::<SessionPrompt>(&SessionPromptParams {
                session_id: id,
                content: vec![ContentBlock::text(text)],
            })
            .await
            .map_err(|error| error.to_string())?;
        let Some(subscription) = &mut subscription else {
            return self.print(&result, || result.turn_id.clone());
        };

        let mut transcript = Transcript::new();
        let mut out = io::stdout().lock();
        while let Some(event) = subscription.next().await {
            self.show(&mut out, &mut transcript, &event)?;
            let done = match &event.event {
                Event::TurnEnded(ended) => ended.turn_id == result.turn_id,
                Event::StateChanged(changed) => !changed.state.is_live(),
                _ => false,
            };
            if done {
                transcript.end_line(&mut out).map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
        Err("lost the connection to burrowd during the turn".into())
    }

    /// `session watch`: the session's history, then live events, across reconnects.
    async fn watch(
        &self,
        mut client: Client,
        id: String,
        after: Option<u64>,
    ) -> Result<(), String> {
        let mut params = SubscribeParams {
            stream: StreamName::session(&id),
            after: Some(after.unwrap_or(0)),
        };
        let mut transcript = Transcript::new();
        let mut out = io::stdout().lock();
        loop {
            match client.subscribe_with(params.clone()).await {
                Ok(mut subscription) => {
                    while let Some(event) = subscription.next().await {
                        if self.show(&mut out, &mut transcript, &event).is_err() {
                            return Ok(());
                        }
                    }
                    params = subscription.resume_params();
                }
                Err(CallError::Disconnected) => {}
                Err(error) => return Err(error.to_string()),
            }
            let _ = transcript.end_line(&mut out);
            eprintln!("burrowctl: lost the connection to burrowd; reconnecting...");
            client = self.reconnect().await;
        }
    }

    fn show(
        &self,
        out: &mut impl Write,
        transcript: &mut Transcript,
        event: &burrow_protocol::StreamEvent,
    ) -> Result<(), String> {
        if self.json {
            to_json(event).and_then(|line| {
                writeln!(out, "{line}")
                    .and_then(|()| out.flush())
                    .map_err(|e| e.to_string())
            })
        } else {
            transcript.print(out, event).map_err(|e| e.to_string())
        }
    }

    async fn inbox(&self, command: InboxCommand) -> Result<(), String> {
        let client = self.connect().await?;
        match command {
            InboxCommand::List { all } => {
                let items = client
                    .call::<InboxList>(&InboxListParams {
                        status: (!all).then_some(InboxItemStatus::Open),
                        ..InboxListParams::default()
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                let empty = if all {
                    "the inbox is empty"
                } else {
                    "nothing waits on you"
                };
                self.print_list(&items, render::inbox_item, empty)
            }
            InboxCommand::Respond { id, action } => {
                let item = client
                    .call::<InboxRespond>(&InboxRespondParams {
                        item_id: id,
                        action,
                        payload: None,
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                self.print(&item, || render::inbox_item(&item))
            }
        }
    }

    async fn tail(&self, stream: String, after: Option<u64>) -> Result<(), String> {
        let mut client = self.connect().await?;
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
                        let line = to_json(&event)?;
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
            client = self.reconnect().await;
            eprintln!(
                "burrowctl: reconnected; resuming after seq {}",
                params.after.unwrap_or_default()
            );
        }
    }

    async fn debug_append(&self, stream: String, event: &str) -> Result<(), String> {
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

        let client = self.connect().await?;
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
        println!("{}", to_json(&result)?);
        Ok(())
    }
}

/// Calls a method that returns `{}`; prints nothing on success.
async fn call_empty<M>(client: &Client, params: M::Params) -> Result<(), String>
where
    M: burrow_protocol::Method<Result = Empty>,
{
    client
        .call::<M>(&params)
        .await
        .map(|Empty {}| ())
        .map_err(|error| error.to_string())
}

fn to_json<T: Serialize + ?Sized>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| error.to_string())
}

/// `path` made absolute against the current directory.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))
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

    #[test]
    fn session_commands_parse() {
        let cli = Cli::try_parse_from([
            "burrowctl",
            "session",
            "new",
            "--agent",
            "fake",
            "--model",
            "fake-alt",
            "--json",
        ])
        .unwrap();
        assert!(cli.json);
        let Command::Session(SessionCommand::New { agent, model, .. }) = cli.command else {
            panic!("parsed the wrong command");
        };
        assert_eq!(
            (agent.as_str(), model.as_deref()),
            ("fake", Some("fake-alt"))
        );

        let cli = Cli::try_parse_from([
            "burrowctl",
            "session",
            "prompt",
            "ses_1",
            "hello there",
            "--watch",
        ])
        .unwrap();
        let Command::Session(SessionCommand::Prompt { id, text, watch }) = cli.command else {
            panic!("parsed the wrong command");
        };
        assert_eq!(
            (id.as_str(), text.as_str(), watch),
            ("ses_1", "hello there", true)
        );

        let cli = Cli::try_parse_from(["burrowctl", "inbox", "respond", "inb_1", "allow"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Inbox(InboxCommand::Respond { .. })
        ));
    }
}
