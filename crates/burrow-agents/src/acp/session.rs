//! One ACP session: the agent process, its `burrow-acp` connection, the task that turns its
//! events into [`AgentEvent`]s, and the watchdog that rescues a turn the agent won't end.
//!
//! # Turns and the watchdog (D-025)
//!
//! `prompt` sends `session/prompt` from a task of its own and returns once the request is
//! queued; `burrow-acp` puts the turn's end on the event stream after its last update, and the
//! pump turns that into [`AgentEvent::TurnEnded`].
//!
//! codex-acp sometimes drops a `session/cancel` (codex-acp#641), and sometimes never answers
//! `session/prompt` at all (#600, #747), so a cancelled turn could wait forever. After
//! `cancel`, a watchdog waits [`Timing::cancel_grace`]. If the turn hasn't ended by then, the
//! adapter ends it as `cancelled` itself and restarts the agent: it kills the process tree,
//! starts a new one, and reopens the same ACP session with `session/resume` (or `session/load`,
//! discarding the replay), so the conversation carries on. Mode and model are set again. An
//! agent that can do neither, or a restart that fails, ends the session with
//! [`AgentEvent::Exited`] and an error.
//!
//! Every connection has a generation. The restart bumps it before killing the old process, so
//! nothing the old connection still says reaches Burrow.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Poll;
use std::time::Duration;

use burrow_acp::launch::AgentCommand;
use burrow_acp::{
    AcpClient, AcpError, AcpEvent, AcpEvents, ConfigCategory, ConfigOption, ConfigValue,
    ConnectOptions, Content, PermissionDecision, PromptContent, SessionEvent, SessionInfo, TurnEnd,
};
use burrow_protocol::{AgentId, AgentMode, ContentBlock, ModelInfo, TurnEndReason};
use futures::future::BoxFuture;
use tokio::sync::{Notify, mpsc};
use tracing::{debug, info, warn};

use super::map;
use crate::adapter::{
    AgentError, AgentEvent, AgentModes, AgentSession, PermissionOutcome, PermissionReply,
    PermissionRequest, SessionConfig, StartedSession,
};
use crate::launcher::{self, LaunchSpec, ProcessHandle};

/// Timeouts of one agent's sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// How long `initialize`, and then opening the session, may take.
    pub start_timeout: Duration,
    /// How long a cancelled turn may take to end before the watchdog ends it and restarts the
    /// agent.
    pub cancel_grace: Duration,
    /// `burrow-acp`'s guard for a usage-limit notice whose turn never ends (codex-acp#747).
    pub limit_grace: Duration,
    /// How long a stopping agent gets to exit by itself after its stdin closes.
    pub stop_grace: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            start_timeout: Duration::from_secs(60),
            cancel_grace: Duration::from_secs(10),
            limit_grace: ConnectOptions::default().limit_grace,
            stop_grace: Duration::from_secs(3),
        }
    }
}

/// Everything needed to start the agent, and to start it again.
#[derive(Clone, Debug)]
pub struct Launcher {
    /// The agent.
    pub id: AgentId,
    /// Its display name.
    pub name: &'static str,
    /// The mode that asks before changes, set when a session opens.
    pub default_mode: Option<&'static str>,
    /// The command that runs its ACP server.
    pub command: AgentCommand,
    /// The folder the agent works in.
    pub cwd: PathBuf,
    /// Names the process in logs.
    pub label: String,
    /// Timeouts.
    pub timing: Timing,
}

/// A started agent process with its connection.
struct Launched {
    client: AcpClient,
    events: AcpEvents,
    handle: ProcessHandle,
}

/// One connection to the agent. Replaced when the watchdog restarts the agent.
struct Connection {
    generation: u64,
    client: AcpClient,
    session_id: String,
    handle: ProcessHandle,
}

/// The agent's modes, and how to change them.
#[derive(Clone, Debug)]
struct ModeSet {
    current: String,
    available: Vec<AgentMode>,
    /// `None`: `session/set_mode`. `Some(id)`: the session config option `id`.
    config_id: Option<String>,
}

struct Shared {
    launcher: Launcher,
    events: mpsc::UnboundedSender<AgentEvent>,
    state: Mutex<State>,
    /// Signalled when a restart ends or the session starts stopping.
    changed: Notify,
}

struct State {
    conn: Option<Arc<Connection>>,
    /// The current connection's generation; a pump of another generation is ignored.
    generation: u64,
    /// Every process this session started that may still run, so stop and drop reach a
    /// process a restart is starting.
    processes: Vec<ProcessHandle>,
    stopping: bool,
    restarting: bool,
    /// Set once [`AgentEvent::Exited`] is sent.
    exited: bool,
    turn: Option<Turn>,
    next_turn: u64,
    modes: Option<ModeSet>,
    model: Option<String>,
    models: Vec<ModelInfo>,
}

#[derive(Clone, Copy, Debug)]
struct Turn {
    number: u64,
    cancel_sent: bool,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn send(&self, event: AgentEvent) {
        // The manager may have stopped listening; that's fine.
        let _ = self.events.send(event);
    }

    fn name(&self) -> &'static str {
        self.launcher.name
    }
}

/// Starts the agent, opens a session in `config.cwd`, sets the default mode and the requested
/// model, and starts reading its events.
pub async fn start(
    launcher: Launcher,
    config: SessionConfig,
) -> Result<StartedSession, AgentError> {
    let name = launcher.name;
    let Launched {
        client,
        events,
        handle,
    } = launch(&launcher).await?;
    // From here on, returning early drops `handle`, which kills the process tree.
    let info = with_timeout(
        launcher.timing.start_timeout,
        client.new_session(&launcher.cwd),
        "opening a session",
    )
    .await
    .map_err(|error| open_error(name, error))?;
    let session_id = info.session_id.clone();

    let mut modes = mode_set(&info);
    if let (Some(default), Some(modes)) = (launcher.default_mode, &mut modes)
        && modes.current != default
    {
        if modes.available.iter().any(|mode| mode.id == default) {
            match set_mode(&client, &session_id, modes, default).await {
                Ok(()) => modes.current = default.to_owned(),
                Err(error) => {
                    warn!(agent = name, mode = default, %error, "can't switch to the default mode")
                }
            }
        } else {
            warn!(
                agent = name,
                mode = default,
                "the agent doesn't offer its default mode"
            );
        }
    }

    let (mut model, models) = match &info.models {
        Some(state) => (
            Some(state.current.clone()),
            state
                .available
                .iter()
                .cloned()
                .map(map::model_info)
                .collect::<Vec<_>>(),
        ),
        None => (None, Vec::new()),
    };
    if let Some(wanted) = config.model.filter(|wanted| model.as_ref() != Some(wanted)) {
        check_model(name, &models, &wanted)?;
        client
            .set_model(&session_id, &wanted)
            .await
            .map_err(|error| model_error(name, error))?;
        model = Some(wanted);
    }

    let (sender, receiver) = mpsc::unbounded_channel();
    let conn = Arc::new(Connection {
        generation: 0,
        client,
        session_id: session_id.clone(),
        handle: handle.clone(),
    });
    let shared = Arc::new(Shared {
        launcher,
        events: sender,
        state: Mutex::new(State {
            conn: Some(conn.clone()),
            generation: 0,
            processes: vec![handle],
            stopping: false,
            restarting: false,
            exited: false,
            turn: None,
            next_turn: 1,
            modes,
            model: model.clone(),
            models: models.clone(),
        }),
        changed: Notify::new(),
    });
    tokio::spawn(pump(shared.clone(), conn, events, Translator::new(true)));
    info!(agent = name, session = session_id, model = ?model, "ACP session open");
    Ok(StartedSession {
        session: Box::new(AcpSession { shared }),
        events: receiver,
        model,
        models,
        agent_session_id: Some(session_id),
    })
}

/// Starts the agent's process in a Job Object and runs `initialize`.
async fn launch(launcher: &Launcher) -> Result<Launched, AgentError> {
    let name = launcher.name;
    let mut spec = LaunchSpec::new(
        &launcher.command.program,
        launcher.command.args.clone(),
        launcher.label.clone(),
    )
    .cwd(&launcher.cwd);
    for (key, value) in &launcher.command.env {
        spec = spec.env(key, value);
    }
    let mut process = launcher::spawn(&spec).map_err(|error| {
        AgentError::Unavailable(format!(
            "can't start {name} ({}): {error}",
            launcher.command.program.display()
        ))
    })?;
    let handle = process.handle();
    let stdin = process.take_stdin().expect("stdin is piped");
    let stdout = process.take_stdout().expect("stdout is piped");
    drop(process);
    let options = ConnectOptions {
        initialize_timeout: launcher.timing.start_timeout,
        limit_grace: launcher.timing.limit_grace,
        ..ConnectOptions::default()
    };
    match AcpClient::connect_tokio(stdin, stdout, options).await {
        Ok((client, events)) => Ok(Launched {
            client,
            events,
            handle,
        }),
        Err(error) => {
            let exit = tokio::time::timeout(Duration::from_secs(2), handle.exited()).await;
            let exit = exit.map_or_else(|_| "still running".to_owned(), |exit| exit.description);
            Err(AgentError::Unavailable(format!(
                "{name} didn't start ({exit}): {error}{}",
                stderr_suffix(&handle)
            )))
        }
    }
}

/// `: <last stderr line>`, or nothing.
fn stderr_suffix(handle: &ProcessHandle) -> String {
    handle
        .stderr_tail()
        .iter()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| format!("; it said: {}", line.trim()))
        .unwrap_or_default()
}

async fn with_timeout<T>(
    timeout: Duration,
    work: impl Future<Output = Result<T, AcpError>>,
    what: &str,
) -> Result<T, AcpError> {
    tokio::time::timeout(timeout, work)
        .await
        .unwrap_or_else(|_| {
            Err(AcpError::Protocol {
                method: what.to_owned(),
                message: format!("no answer within {timeout:?}"),
            })
        })
}

fn open_error(name: &str, error: AcpError) -> AgentError {
    match error {
        AcpError::AuthRequired { message, .. } => AgentError::Unavailable(format!(
            "{name} isn't logged in ({message}): log in with its own CLI, then try again"
        )),
        AcpError::LimitReached(info) => AgentError::LimitReached {
            message: info.message,
            resets_at: info.resets_at.and_then(map::timestamp),
        },
        other => AgentError::Unavailable(format!("{name} couldn't open a session: {other}")),
    }
}

fn model_error(name: &str, error: AcpError) -> AgentError {
    match error {
        AcpError::Unsupported(_) => {
            AgentError::Invalid(format!("{name} doesn't let Burrow choose its model"))
        }
        other => AgentError::Failed(format!("{name} couldn't switch models: {other}")),
    }
}

fn check_model(name: &str, models: &[ModelInfo], wanted: &str) -> Result<(), AgentError> {
    if models.is_empty() || models.iter().any(|model| model.id == wanted) {
        return Ok(());
    }
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    Err(AgentError::Invalid(format!(
        "{name} has no model {wanted:?}; it offers {}",
        ids.join(", ")
    )))
}

/// The session's modes: ACP's `modes`, or else a config option of category `mode`.
fn mode_set(info: &SessionInfo) -> Option<ModeSet> {
    if let Some(modes) = &info.modes {
        return Some(ModeSet {
            current: modes.current.clone(),
            available: modes
                .available
                .iter()
                .cloned()
                .map(map::agent_mode)
                .collect(),
            config_id: None,
        });
    }
    info.config_options.iter().find_map(mode_option)
}

fn mode_option(option: &ConfigOption) -> Option<ModeSet> {
    match (&option.category, &option.value) {
        (ConfigCategory::Mode, ConfigValue::Select { current, options }) => Some(ModeSet {
            current: current.clone(),
            available: options.iter().cloned().map(map::agent_mode).collect(),
            config_id: Some(option.id.clone()),
        }),
        _ => None,
    }
}

async fn set_mode(
    client: &AcpClient,
    session_id: &str,
    modes: &ModeSet,
    mode: &str,
) -> Result<(), AcpError> {
    match &modes.config_id {
        None => client.set_mode(session_id, mode).await,
        Some(config_id) => client
            .set_config_option(session_id, config_id, mode)
            .await
            .map(drop),
    }
}

/// The session handle the manager holds.
struct AcpSession {
    shared: Arc<Shared>,
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        // Dropped without `stop`, for example because burrowd is shutting down: kill every tree.
        let processes = {
            let mut state = self.shared.state();
            state.stopping = true;
            state.processes.clone()
        };
        self.shared.changed.notify_waiters();
        for process in processes {
            process.kill();
        }
    }
}

impl AcpSession {
    /// The connection to use, after any restart in progress.
    async fn connection(&self) -> Result<Arc<Connection>, AgentError> {
        let shared = &self.shared;
        let deadline = tokio::time::Instant::now() + shared.launcher.timing.start_timeout * 2;
        loop {
            let changed = shared.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = shared.state();
                if state.stopping || state.exited {
                    return Err(AgentError::InvalidState(format!(
                        "{} has stopped",
                        shared.name()
                    )));
                }
                if !state.restarting {
                    return state.conn.clone().ok_or_else(|| {
                        AgentError::InvalidState(format!("{} isn't running", shared.name()))
                    });
                }
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Err(AgentError::InvalidState(format!(
                    "{} is still restarting",
                    shared.name()
                )));
            }
        }
    }
}

impl AgentSession for AcpSession {
    fn prompt(&self, content: Vec<ContentBlock>) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            let conn = self.connection().await?;
            {
                let mut state = self.shared.state();
                if state.turn.is_some() {
                    return Err(AgentError::InvalidState("a turn is already running".into()));
                }
                let number = state.next_turn;
                state.next_turn += 1;
                state.turn = Some(Turn {
                    number,
                    cancel_sent: false,
                });
            }
            let text: Vec<&str> = content.iter().filter_map(ContentBlock::as_text).collect();
            let text = text.join("\n");
            let client = conn.client.clone();
            let session_id = conn.session_id.clone();
            let name = self.shared.name();
            let mut request = Box::pin(async move {
                let result = client
                    .prompt(&session_id, [PromptContent::Text(text)])
                    .await;
                match result {
                    Ok(result) => debug!(agent = name, stop = ?result.stop_reason, "turn ended"),
                    Err(error) => debug!(agent = name, %error, "turn ended with an error"),
                }
            });
            // Poll once, so `session/prompt` is queued before this returns and a cancel can't
            // overtake it. The turn's end arrives on the event stream.
            if futures::poll!(&mut request) == Poll::Pending {
                tokio::spawn(request);
            }
            Ok(())
        })
    }

    fn cancel(&self) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            let (conn, number, first) = {
                let mut state = self.shared.state();
                let Some(turn) = &mut state.turn else {
                    return Ok(());
                };
                let first = !turn.cancel_sent;
                turn.cancel_sent = true;
                let number = turn.number;
                let Some(conn) = state.conn.clone() else {
                    return Ok(());
                };
                (conn, number, first)
            };
            // Also answers the turn's open permission requests as cancelled.
            if let Err(error) = conn.client.cancel(&conn.session_id).await {
                debug!(agent = self.shared.name(), %error, "session/cancel failed");
            }
            if first {
                tokio::spawn(watchdog(self.shared.clone(), number));
            }
            Ok(())
        })
    }

    fn set_model(&self, model: String) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            let name = self.shared.name();
            let conn = self.connection().await?;
            check_model(name, &self.shared.state().models, &model)?;
            conn.client
                .set_model(&conn.session_id, &model)
                .await
                .map_err(|error| model_error(name, error))?;
            self.shared.state().model = Some(model);
            Ok(())
        })
    }

    fn agent_modes(&self) -> Option<AgentModes> {
        self.shared.state().modes.as_ref().map(|modes| AgentModes {
            current: modes.current.clone(),
            available: modes.available.clone(),
        })
    }

    fn set_agent_mode(&self, mode: String) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            let name = self.shared.name();
            let conn = self.connection().await?;
            let modes = self.shared.state().modes.clone().ok_or_else(|| {
                AgentError::Invalid(format!(
                    "{name} has no modes, so it can't switch to {mode:?}"
                ))
            })?;
            if !modes.available.iter().any(|known| known.id == mode) {
                let ids: Vec<&str> = modes.available.iter().map(|m| m.id.as_str()).collect();
                return Err(AgentError::Invalid(format!(
                    "{name} has no mode {mode:?}; it offers {}",
                    ids.join(", ")
                )));
            }
            set_mode(&conn.client, &conn.session_id, &modes, &mode)
                .await
                .map_err(|error| match error {
                    AcpError::Unsupported(_) => {
                        AgentError::Invalid(format!("{name} doesn't let Burrow switch modes"))
                    }
                    other => AgentError::Failed(format!("{name} couldn't switch modes: {other}")),
                })?;
            if let Some(modes) = &mut self.shared.state().modes {
                modes.current = mode;
            }
            Ok(())
        })
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let shared = &self.shared;
            let (conn, processes) = {
                let mut state = shared.state();
                state.stopping = true;
                (state.conn.clone(), state.processes.clone())
            };
            shared.changed.notify_waiters();
            if let Some(conn) = conn {
                // Closing stdin asks the agent to exit.
                conn.client.stop().await;
            }
            let grace = shared.launcher.timing.stop_grace;
            futures::future::join_all(processes.iter().map(|process| process.stop(grace))).await;
        })
    }
}

/// After a cancel: if the turn is still running after the grace period, end it and restart the
/// agent.
async fn watchdog(shared: Arc<Shared>, number: u64) {
    let grace = shared.launcher.timing.cancel_grace;
    tokio::time::sleep(grace).await;
    let old = {
        let mut state = shared.state();
        let still_running = state.turn.is_some_and(|turn| turn.number == number);
        if !still_running || state.stopping || state.exited {
            return;
        }
        state.turn = None;
        state.restarting = true;
        // From now on nothing the old connection says reaches Burrow.
        state.generation += 1;
        state.conn.take()
    };
    let name = shared.name();
    warn!(
        agent = name,
        "the turn didn't end {grace:?} after the cancel; ending it and restarting the agent"
    );
    shared.send(AgentEvent::TurnEnded {
        reason: TurnEndReason::Cancelled,
        message: Some(format!(
            "{name} didn't end the turn within {} s of the cancel, so Burrow ended it and \
             restarted the agent",
            grace.as_secs_f64()
        )),
    });
    restart(shared, old).await;
}

/// Replaces a stuck agent process with a new one that reopens the same ACP session.
async fn restart(shared: Arc<Shared>, old: Option<Arc<Connection>>) {
    let name = shared.name();
    let session_id = old.as_ref().map(|old| old.session_id.clone());
    if let Some(old) = &old {
        old.handle.kill();
        old.client.stop().await;
        old.handle.stop(Duration::ZERO).await;
        shared
            .state()
            .processes
            .retain(|process| process.pid() != old.handle.pid());
    }
    let result = match session_id {
        Some(session_id) => reopen(&shared, &session_id).await,
        None => Err(AgentError::Failed(
            "there was no connection to restart".into(),
        )),
    };

    let mut state = shared.state();
    state.restarting = false;
    let stopping = state.stopping;
    match result {
        Ok((launched, session_id)) if !stopping => {
            let conn = Arc::new(Connection {
                generation: state.generation,
                client: launched.client,
                session_id,
                handle: launched.handle,
            });
            state.conn = Some(conn.clone());
            drop(state);
            info!(agent = name, "restarted after a stuck turn");
            tokio::spawn(pump(
                shared.clone(),
                conn,
                launched.events,
                Translator::new(false),
            ));
        }
        Ok((launched, _)) => {
            launched.handle.kill();
            stopped_during_restart(&shared, state);
        }
        Err(_) if stopping => stopped_during_restart(&shared, state),
        Err(error) => {
            state.exited = true;
            drop(state);
            warn!(agent = name, %error, "restarting failed");
            shared.send(AgentEvent::Exited {
                error: Some(format!(
                    "{name} ignored a cancel and Burrow restarted it, but the restart failed: \
                     {error}"
                )),
            });
        }
    }
    shared.changed.notify_waiters();
}

/// `stop` arrived during a restart: nothing else will say the agent exited.
fn stopped_during_restart(shared: &Shared, mut state: MutexGuard<'_, State>) {
    if !state.exited {
        state.exited = true;
        drop(state);
        shared.send(AgentEvent::Exited { error: None });
    }
}

/// Starts the agent again and reopens `session_id`, with the mode and model it had.
async fn reopen(shared: &Shared, session_id: &str) -> Result<(Launched, String), AgentError> {
    let name = shared.name();
    let timing = shared.launcher.timing;
    let mut launched = launch(&shared.launcher).await?;
    {
        let mut state = shared.state();
        if state.stopping {
            launched.handle.kill();
            return Err(AgentError::InvalidState(format!("{name} is stopping")));
        }
        state.processes.push(launched.handle.clone());
    }
    let client = &launched.client;
    let capabilities = client.agent().capabilities.clone();
    let cwd = &shared.launcher.cwd;
    let info = if capabilities.resume_session {
        with_timeout(
            timing.start_timeout,
            client.resume_session(session_id, cwd),
            "session/resume",
        )
        .await
    } else if capabilities.load_session {
        let info = with_timeout(
            timing.start_timeout,
            client.load_session(session_id, cwd),
            "session/load",
        )
        .await;
        // The replayed history is already on screen.
        while launched.events.try_recv().is_some() {}
        info
    } else {
        return Err(AgentError::Failed(format!(
            "{name} can't reopen a session (it has neither session/resume nor session/load)"
        )));
    };
    let info =
        info.map_err(|error| AgentError::Failed(format!("reopening the session: {error}")))?;

    let (wanted_mode, wanted_model) = {
        let state = shared.state();
        (
            state.modes.as_ref().map(|modes| modes.current.clone()),
            state.model.clone(),
        )
    };
    if let (Some(wanted), Some(modes)) = (wanted_mode, mode_set(&info))
        && modes.current != wanted
        && let Err(error) = set_mode(client, session_id, &modes, &wanted).await
    {
        warn!(agent = name, mode = wanted, %error, "can't set the mode again after a restart");
    }
    let current_model = info.models.as_ref().map(|models| models.current.clone());
    if let Some(wanted) = wanted_model.filter(|wanted| current_model.as_ref() != Some(wanted))
        && let Err(error) = client.set_model(session_id, &wanted).await
    {
        warn!(agent = name, model = wanted, %error, "can't set the model again after a restart");
    }
    let session_id = info.session_id.clone();
    Ok((launched, session_id))
}

/// What the pump remembers between events.
struct Translator {
    /// Tool call titles by id, for permission requests that carry no title.
    titles: HashMap<String, String>,
    /// The session's cost so far, as the agent last reported it (USD).
    cost: Option<f64>,
    /// The cost already reported with earlier turns; `None` when unknown, as after a restart.
    reported_cost: Option<f64>,
}

impl Translator {
    fn new(fresh: bool) -> Self {
        Self {
            titles: HashMap::new(),
            cost: None,
            reported_cost: fresh.then_some(0.0),
        }
    }

    /// This turn's cost: the session's cost now, less what earlier turns reported.
    fn turn_cost(&mut self) -> Option<f64> {
        let cost = self.cost?;
        let turn = self
            .reported_cost
            .map(|reported| (cost - reported).max(0.0));
        self.reported_cost = Some(cost);
        turn
    }
}

/// Reads one connection's events until it closes.
async fn pump(
    shared: Arc<Shared>,
    conn: Arc<Connection>,
    mut events: AcpEvents,
    mut translator: Translator,
) {
    let mut killed = false;
    let error = loop {
        let event = tokio::select! {
            event = events.recv() => event,
            _ = conn.handle.exited(), if !killed => {
                // The agent's main process ended. Kill what is left, so pipes that a child
                // still holds close, and read on until the connection closes.
                killed = true;
                conn.handle.kill();
                continue;
            }
        };
        let current = shared.state().generation == conn.generation;
        match event {
            None => break None,
            Some(AcpEvent::Closed { error }) => break error,
            Some(_) if !current => {}
            Some(AcpEvent::Session { session_id, event }) => {
                if session_id == conn.session_id {
                    session_event(&shared, &mut translator, event);
                } else {
                    debug!(
                        agent = shared.name(),
                        session_id, "an update for another session"
                    );
                }
            }
            Some(AcpEvent::Permission(request)) => {
                if request.session_id == conn.session_id {
                    permission(&shared, &mut translator, request);
                }
            }
        }
    };
    closed(&shared, &conn, error).await;
}

/// The connection closed: unless a restart replaced it, the agent has exited.
async fn closed(shared: &Shared, conn: &Connection, error: Option<String>) {
    let stopping = {
        let mut state = shared.state();
        if state.generation != conn.generation || state.exited {
            return;
        }
        state.exited = true;
        state.turn = None;
        state.stopping
    };
    let error = if stopping {
        None
    } else {
        let exit = tokio::time::timeout(Duration::from_secs(2), conn.handle.exited()).await;
        let exit = exit.map_or_else(
            |_| error.unwrap_or_else(|| "its connection closed".to_owned()),
            |exit| exit.description,
        );
        Some(format!(
            "{} exited ({exit}){}",
            shared.name(),
            stderr_suffix(&conn.handle)
        ))
    };
    conn.handle.kill();
    shared.send(AgentEvent::Exited { error });
}

fn session_event(shared: &Shared, translator: &mut Translator, event: SessionEvent) {
    let name = shared.name();
    match event {
        SessionEvent::AgentMessageChunk { content, .. } => {
            if let Some(text) = chunk_text(content) {
                shared.send(AgentEvent::MessageChunk(text));
            }
        }
        SessionEvent::AgentThoughtChunk { content, .. } => {
            if let Some(text) = chunk_text(content) {
                shared.send(AgentEvent::ThoughtChunk(text));
            }
        }
        SessionEvent::ToolCall(call) => {
            translator
                .titles
                .insert(call.id.clone(), call.title.clone());
            shared.send(AgentEvent::ToolCall(map::tool_call(call)));
        }
        SessionEvent::ToolCallUpdate(update) => {
            if let Some(title) = &update.title {
                translator.titles.insert(update.id.clone(), title.clone());
            }
            shared.send(AgentEvent::ToolCallUpdate(map::tool_call_update(update)));
        }
        SessionEvent::Plan(entries) => shared.send(AgentEvent::Plan(map::plan(entries))),
        SessionEvent::ModeChanged { mode_id } => mode_changed(shared, mode_id),
        SessionEvent::ConfigOptionsChanged(options) => {
            for option in &options {
                if let Some(modes) = mode_option(option) {
                    mode_changed(shared, modes.current);
                }
                if let (ConfigCategory::Model, ConfigValue::Select { current, .. }) =
                    (&option.category, &option.value)
                {
                    let changed = {
                        let mut state = shared.state();
                        let changed = state.model.as_ref() != Some(current);
                        state.model = Some(current.clone());
                        changed
                    };
                    if changed {
                        shared.send(AgentEvent::ModelChanged(current.clone()));
                    }
                }
            }
        }
        SessionEvent::ContextUsage { cost, .. } => {
            if let Some(cost) = cost.filter(|cost| cost.currency.eq_ignore_ascii_case("USD")) {
                translator.cost = Some(cost.amount);
            }
        }
        SessionEvent::RateLimit(status) => {
            let provider = shared.launcher.id.to_string();
            shared.send(AgentEvent::ProviderUsage(map::provider_usage(
                &provider, status,
            )));
        }
        SessionEvent::LimitReached(info) => shared.send(AgentEvent::LimitReached {
            provider: None,
            resets_at: info.resets_at.and_then(map::timestamp),
            message: info.message,
        }),
        SessionEvent::TurnEnded(end) => turn_ended(shared, translator, end),
        SessionEvent::Notice {
            severity,
            title,
            description,
        } => info!(
            agent = name,
            severity, title, description, "notice from the agent"
        ),
        SessionEvent::UserMessageChunk { .. }
        | SessionEvent::AvailableCommands(_)
        | SessionEvent::SessionInfoChanged { .. } => {}
        SessionEvent::Other { kind, .. } => debug!(agent = name, kind, "an update Burrow ignores"),
    }
}

/// Agent text; other content in a message is described, not dropped silently.
fn chunk_text(content: Content) -> Option<String> {
    match content {
        Content::Text(text) => Some(text),
        other => map::content_block(other).map(|block| match block {
            ContentBlock::Text { text } => text,
            ContentBlock::Resource { uri } => format!("[{uri}]"),
            ContentBlock::Image { .. } => "[image]".to_owned(),
        }),
    }
}

fn mode_changed(shared: &Shared, mode: String) {
    let changed = {
        let mut state = shared.state();
        match &mut state.modes {
            Some(modes) if modes.current != mode => {
                modes.current = mode.clone();
                true
            }
            Some(_) => false,
            None => true,
        }
    };
    if changed {
        shared.send(AgentEvent::AgentModeChanged(mode));
    }
}

fn turn_ended(shared: &Shared, translator: &mut Translator, end: TurnEnd) {
    if shared.state().turn.take().is_none() {
        // The watchdog ended it already.
        debug!(
            agent = shared.name(),
            "a turn ended that Burrow had already ended"
        );
        return;
    }
    let cost = translator.turn_cost();
    if let TurnEnd::Stopped {
        usage: Some(usage), ..
    } = &end
    {
        shared.send(AgentEvent::Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: cost,
        });
    }
    let (reason, message) = map::turn_end(end);
    shared.send(AgentEvent::TurnEnded { reason, message });
}

/// A permission request becomes [`AgentEvent::Permission`]; a task passes the answer back.
fn permission(
    shared: &Shared,
    translator: &mut Translator,
    request: burrow_acp::PermissionRequest,
) {
    let id = request.tool_call.id.clone();
    let title = request
        .tool_call
        .title
        .clone()
        .or_else(|| translator.titles.get(&id).cloned())
        .unwrap_or_else(|| "Run a tool".to_owned());
    if !translator.titles.contains_key(&id) {
        // The agent asks about a tool call it never announced: announce it, so the question
        // points at something.
        translator.titles.insert(id.clone(), title.clone());
        shared.send(AgentEvent::ToolCall(map::tool_call_from_update(
            request.tool_call.clone(),
            title.clone(),
        )));
    }
    let options = request
        .options
        .iter()
        .cloned()
        .map(map::permission_option)
        .collect();
    let (reply, answer) = PermissionReply::channel();
    shared.send(AgentEvent::Permission(PermissionRequest {
        tool_call_id: id,
        title,
        options,
        reply,
    }));
    let name = shared.name();
    tokio::spawn(async move {
        let decision = tokio::select! {
            outcome = answer => match outcome {
                Ok(PermissionOutcome::Selected(option)) => PermissionDecision::Select(option),
                Ok(PermissionOutcome::Cancelled) | Err(_) => PermissionDecision::Cancelled,
            },
            () = request.withdrawn() => {
                debug!(agent = name, "the agent withdrew a permission request");
                return;
            }
        };
        if let Err(error) = request.respond(decision) {
            // Already answered as cancelled, because the turn was cancelled.
            debug!(agent = name, %error, "couldn't pass a permission answer on");
        }
    });
}
