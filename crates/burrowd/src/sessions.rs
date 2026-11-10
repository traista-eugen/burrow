//! The session manager: agent sessions, their `session:<id>` streams, and the inbox items their
//! permission requests become.
//!
//! # How a session runs
//!
//! Each session has a [`Slot`]: a snapshot for `session/get` and lists, and a [`Core`] behind an
//! async mutex. Everything that changes a session goes through its core, one thing at a time:
//! requests (`session/prompt`, `inbox/respond`, …) and the agent's events, which a pump task
//! reads from the adapter. So a session's events are appended in a consistent order.
//!
//! Every change is one [`Commit`] (D-017): the session stream's events, the `index` summaries
//! (`entityStateChanged`, `inboxItemAdded`, …), and the session and inbox rows, in one
//! transaction. Memory is updated only after the commit succeeds.
//!
//! - **Text** (`agentMessageChunk`, `agentThoughtChunk`) is buffered and committed about every
//!   100 ms, or sooner when anything else happens in the session, so order is kept and each
//!   append costs one fsync per 100 ms at most (D-012).
//! - **Permission requests** become inbox items of kind `permission`. The session is `waiting`
//!   while any is open. `inbox/respond` resolves the item and passes the answer to the agent;
//!   an item that is no longer open gets `conflict`. A request the agent withdraws
//!   ([`AgentEvent::PermissionWithdrawn`]) expires at once (D-031).
//! - **Activity.** `lastActivityAt` moves when a turn starts or ends. Every change of state or
//!   activity puts `entityStateChanged`, carrying both, on the `index`, so a sidebar sorted by
//!   activity stays current from the index alone. `session/list` sorts by it.
//! - **Keep-awake** is held while any session is `working` (not `waiting`, so a forgotten
//!   question doesn't keep the PC up for days).
//! - **Restarts.** Agent processes die with `burrowd` (Job Objects, D-007). On startup, sessions
//!   the rows still show as live are marked `stopped` and their open inbox items expire, with
//!   events, before any client connects. A clean shutdown does the same while stopping them.
//!
//! # Terminal sessions
//!
//! A session created with `mode: "terminal"` runs the agent's own TUI on a pseudo console
//! ([`crate::terminals`]) instead of a protocol adapter. Its stream is short: `stateChanged`
//! (`starting`), `terminalOpened`, `stateChanged` (`idle`) while the TUI runs, then
//! `terminalClosed` with the exit code and `stateChanged`: `stopped` when the program exited
//! with code 0 or was stopped, `failed` otherwise. It has no turns: its core holds a
//! [`TerminalSession`] that refuses prompts, models and modes, and stops the terminal.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use burrow_agents::{
    AgentAdapter, AgentError, AgentEvent, AgentSession, PermissionId, PermissionOutcome,
    PermissionReply, PermissionRequest, SessionConfig, find_model,
};
use burrow_protocol::{
    AgentMessageChunk, AgentModeChanged, AgentThoughtChunk, ContentBlock, EntityCreated,
    EntityKind, EntityRemoved, EntityRenamed, EntityStateChanged, ErrorKind, ErrorObject, Event,
    InboxAction, InboxActionStyle, InboxItem, InboxItemAdded, InboxItemIdParams, InboxItemKind,
    InboxItemResolved, InboxItemStatus, InboxListParams, InboxResolution, InboxRespondParams,
    InboxSource, LimitInfo, LimitReached, ModelChanged, PermissionOptionKind, PermissionRequested,
    PermissionResolved, ProviderUsage, Session, SessionCreateParams, SessionListParams,
    SessionMode, SessionPromptParams, SessionPromptResult, SessionRenameParams,
    SessionSetAgentModeParams, SessionSetModelParams, SessionState, SessionSummary, StateChanged,
    StreamName, TerminalClosed, TerminalOpened, TerminalSize, Timestamp, TurnEndReason, TurnEnded,
    Usage, UsageSource, UserMessage,
};
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::agents::Agents;
use crate::cancel::Cancel;
use crate::hub::{Commit, Hub, HubError};
use crate::ids;
use crate::keep_awake::KeepAwake;
use crate::projects::{self, Projects};
use crate::terminals::{Terminal, Terminals};

/// The entity kind of session rows.
const SESSION: &str = "session";
/// The entity kind of inbox item rows.
const INBOX: &str = "inbox";
/// How long agent text is buffered before it is committed.
const CHUNK_WINDOW: Duration = Duration::from_millis(100);
/// Buffered text beyond this is committed at once.
const CHUNK_MAX_BYTES: usize = 16 * 1024;
/// `limit` of a list when absent.
const DEFAULT_LIMIT: u32 = 100;
/// The most a list returns.
const MAX_LIMIT: u32 = 1000;
/// The longest title made from a prompt, in characters.
const TITLE_CHARS: usize = 60;
/// The longest title `session/rename` accepts, in characters.
const MAX_TITLE_CHARS: usize = 200;
/// How long a clean shutdown waits for sessions to stop.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(15);

/// Every session, live or not, and every inbox item.
pub struct Sessions {
    hub: Hub,
    agents: Arc<Agents>,
    projects: Arc<Projects>,
    keep_awake: KeepAwake,
    terminals: Arc<Terminals>,
    shared: Mutex<Shared>,
}

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions").finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Shared {
    /// By id; ids sort by creation time.
    slots: BTreeMap<String, Arc<Slot>>,
    /// Every inbox item, by id.
    inbox: BTreeMap<String, InboxItem>,
    /// Sessions that are `working`, for keep-awake.
    working: HashSet<String>,
}

/// One session: its snapshot for readers, and its core for changes.
struct Slot {
    snapshot: Arc<Mutex<Session>>,
    core: tokio::sync::Mutex<Core>,
    /// Set by `session/stop` and `session/archive`, so a start in progress gives up.
    stop: Cancel,
}

impl Slot {
    fn new(session: Session) -> Self {
        let snapshot = Arc::new(Mutex::new(session.clone()));
        Self {
            snapshot: snapshot.clone(),
            core: tokio::sync::Mutex::new(Core {
                stream: StreamName::session(&session.id),
                session,
                snapshot,
                agent: None,
                terminal: None,
                buffer: None,
                pending: BTreeMap::new(),
            }),
            stop: Cancel::new(),
        }
    }

    fn snapshot(&self) -> Session {
        lock(&self.snapshot).clone()
    }
}

/// The part of a session that changes, behind the slot's async mutex.
struct Core {
    /// The committed state.
    session: Session,
    snapshot: Arc<Mutex<Session>>,
    stream: StreamName,
    /// The running agent; `None` once stopped or failed.
    agent: Option<Box<dyn AgentSession>>,
    /// A terminal session's terminal.
    terminal: Option<Arc<Terminal>>,
    /// Agent text not committed yet.
    buffer: Option<TextBuffer>,
    /// Open permission requests, by inbox item id.
    pending: BTreeMap<String, Pending>,
}

struct TextBuffer {
    thought: bool,
    turn_id: String,
    text: String,
    due: Instant,
}

struct Pending {
    item: InboxItem,
    tool_call_id: String,
    reply: PermissionReply,
}

/// One change to a session, built up and then committed by [`Core::finish`].
struct Change {
    commit: Commit,
    /// The session as it will be.
    next: Session,
    /// Inbox items that changed.
    items: Vec<InboxItem>,
}

enum Start {
    Done(Result<burrow_agents::StartedSession, AgentError>),
    Cancelled,
    Stopped,
}

impl Sessions {
    /// Loads sessions and inbox items from the entities table, then marks sessions that were
    /// live as `stopped` and expires their open inbox items: their agents died with the last
    /// `burrowd`.
    pub async fn load(
        hub: Hub,
        agents: Arc<Agents>,
        projects: Arc<Projects>,
        keep_awake: KeepAwake,
        terminals: Arc<Terminals>,
    ) -> Result<Arc<Self>, HubError> {
        let mut sessions = BTreeMap::new();
        for row in hub.entities(SESSION).await? {
            match row.body::<Session>() {
                Ok(mut session) => {
                    // Rows from before `lastActivityAt` existed: no turn has been seen since.
                    session.last_activity_at.get_or_insert(session.created_at);
                    sessions.insert(session.id.clone(), session);
                }
                Err(error) => {
                    warn!(id = row.id, %error, "skipping a session row that doesn't decode")
                }
            }
        }
        let mut inbox = BTreeMap::new();
        for row in hub.entities(INBOX).await? {
            match row.body::<InboxItem>() {
                Ok(item) => {
                    inbox.insert(item.id.clone(), item);
                }
                Err(error) => {
                    warn!(id = row.id, %error, "skipping an inbox row that doesn't decode")
                }
            }
        }

        let now = Timestamp::now();
        let index = StreamName::index();
        let mut open: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
        for item in inbox.values() {
            if item.status == InboxItemStatus::Open {
                open.entry(item.source.session_id.clone())
                    .or_default()
                    .push(item.id.clone());
            }
        }
        for session in sessions.values_mut() {
            let items = open.remove(&Some(session.id.clone())).unwrap_or_default();
            if !session.state.is_live() && items.is_empty() {
                continue;
            }
            let stream = StreamName::session(&session.id);
            let mut commit = Commit::new();
            for id in items {
                let item = inbox.get_mut(&id).expect("listed above");
                expire(item, now);
                commit
                    .event(
                        &stream,
                        PermissionResolved {
                            inbox_item_id: id.clone(),
                            tool_call_id: item.source.tool_call_id.clone().unwrap_or_default(),
                            status: InboxItemStatus::Expired,
                            option_id: None,
                            by: None,
                        },
                    )
                    .event(&index, expired_event(&id))
                    .put(INBOX, &id, item);
            }
            if session.state.is_live() {
                if let Some(turn_id) = session.turn_id.take() {
                    commit.event(
                        &stream,
                        TurnEnded {
                            turn_id,
                            reason: TurnEndReason::Error,
                            message: Some("burrowd stopped during the turn".into()),
                        },
                    );
                    session.last_activity_at = Some(now);
                }
                if session.mode == SessionMode::Terminal
                    && let Some(terminal_id) = terminal_of(&hub, &stream).await?
                {
                    session
                        .terminal_id
                        .get_or_insert_with(|| terminal_id.clone());
                    commit.event(
                        &stream,
                        TerminalClosed {
                            terminal_id,
                            exit_code: None,
                        },
                    );
                }
                let reason = "burrowd stopped, and the agent process ended with it";
                session.state = SessionState::Stopped;
                session.reason = Some(reason.into());
                session.updated_at = now;
                commit
                    .event(
                        &stream,
                        StateChanged {
                            state: SessionState::Stopped,
                            reason: Some(reason.into()),
                        },
                    )
                    .event(&index, state_event(session));
            }
            commit.put(SESSION, &session.id, session);
            hub.commit(commit).await?;
            info!(session = session.id, "marked stopped after a restart");
        }
        // Open items whose session is gone can't be answered either.
        let orphans: Vec<String> = open.into_values().flatten().collect();
        if !orphans.is_empty() {
            let mut commit = Commit::new();
            for id in orphans {
                let item = inbox.get_mut(&id).expect("listed above");
                expire(item, now);
                commit
                    .event(&index, expired_event(&id))
                    .put(INBOX, &id, item);
            }
            hub.commit(commit).await?;
        }

        // What each agent's latest session offered is what agent/list reports (D-032, D-035).
        for session in sessions.values() {
            // Ids sort by creation, so later sessions replace earlier ones.
            if !session.models.is_empty() {
                agents.learn_models(&session.agent, session.models.clone());
            }
            if !session.agent_modes.is_empty() {
                agents.learn_agent_modes(&session.agent, session.agent_modes.clone());
            }
        }

        let slots = sessions
            .into_values()
            .map(|session| (session.id.clone(), Arc::new(Slot::new(session))))
            .collect();
        Ok(Arc::new(Self {
            hub,
            agents,
            projects,
            keep_awake,
            terminals,
            shared: Mutex::new(Shared {
                slots,
                inbox,
                working: HashSet::new(),
            }),
        }))
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        lock(&self.shared)
    }

    fn slot(&self, id: &str) -> Result<Arc<Slot>, ErrorObject> {
        self.shared()
            .slots
            .get(id)
            .cloned()
            .ok_or_else(|| ErrorObject::new(ErrorKind::NotFound, format!("no session {id}")))
    }

    /// After a commit: the inbox items that changed, and keep-awake.
    fn publish(&self, session: &Session, items: &[InboxItem]) {
        let mut shared = self.shared();
        for item in items {
            shared.inbox.insert(item.id.clone(), item.clone());
        }
        let changed = if session.state == SessionState::Working {
            shared.working.insert(session.id.clone())
        } else {
            shared.working.remove(&session.id)
        };
        let any = !shared.working.is_empty();
        drop(shared);
        if changed {
            self.keep_awake.set(any);
        }
    }

    /// `session/create`: commits the new session as `starting`, starts the agent, and answers
    /// once it is `idle`, or has `failed`.
    ///
    /// `cancel` (the request's `$/cancel`) and `session/stop` abort the start: the agent's
    /// process tree is killed and the session ends `stopped`.
    ///
    /// A terminal session answers once its TUI is started on its terminal.
    pub async fn create(
        self: &Arc<Self>,
        params: SessionCreateParams,
        cancel: &Cancel,
    ) -> Result<Session, ErrorObject> {
        if params.mode == SessionMode::Terminal {
            if params.model.is_some() {
                return Err(ErrorObject::invalid_params(
                    "a terminal session takes no model: choose it in the agent's TUI",
                ));
            }
            if params.agent_mode.is_some() {
                return Err(ErrorObject::invalid_params(
                    "a terminal session takes no agentMode: choose it in the agent's TUI",
                ));
            }
        }
        let size = terminal_size(&params)?;
        let Some(adapter) = self.agents.registry().get(&params.agent) else {
            let offered: Vec<String> = self
                .agents
                .registry()
                .ids()
                .iter()
                .map(ToString::to_string)
                .collect();
            return Err(ErrorObject::new(
                ErrorKind::Unavailable,
                format!(
                    "agent {} isn't available; this burrowd offers: {}",
                    params.agent,
                    offered.join(", ")
                ),
            ));
        };
        let project = match &params.project_id {
            Some(id) => Some(self.projects.get(id).await?),
            None => None,
        };
        let cwd: PathBuf = match (&params.cwd, &project) {
            (Some(cwd), _) => projects::folder(cwd)?,
            (None, Some(project)) => projects::folder(&project.path)?,
            (None, None) => {
                return Err(ErrorObject::invalid_params(
                    "session/create needs a cwd or a projectId",
                ));
            }
        };

        let now = Timestamp::now();
        let session = Session {
            id: ids::new_id("ses"),
            project_id: params.project_id.clone(),
            agent: params.agent.clone(),
            model: params.model.clone(),
            mode: params.mode,
            terminal_id: None,
            cwd: projects::display_path(&cwd),
            title: params
                .title
                .clone()
                .filter(|title| !title.trim().is_empty()),
            state: SessionState::Starting,
            reason: None,
            turn_id: None,
            models: Vec::new(),
            agent_mode: None,
            agent_modes: Vec::new(),
            usage: None,
            limit: None,
            archived: false,
            created_at: now,
            updated_at: now,
            last_activity_at: Some(now),
        };
        let slot = Arc::new(Slot::new(session.clone()));
        let mut core = slot.core.lock().await;
        let mut commit = Commit::new();
        commit
            .event(
                &core.stream,
                StateChanged {
                    state: SessionState::Starting,
                    reason: None,
                },
            )
            .event(
                &StreamName::index(),
                EntityCreated {
                    entity: EntityKind::Session,
                    id: session.id.clone(),
                    title: session.title.clone(),
                    project_id: session.project_id.clone(),
                    state: Some(SessionState::Starting.to_string()),
                    agent: Some(session.agent.clone()),
                    mode: Some(session.mode),
                },
            )
            .put(SESSION, &session.id, &session);
        self.hub.commit(commit).await.map_err(internal)?;
        self.shared().slots.insert(session.id.clone(), slot.clone());
        info!(session = session.id, agent = %session.agent, cwd = session.cwd, "starting a session");

        let config = SessionConfig {
            session_id: session.id.clone(),
            cwd,
            model: params.model.clone(),
        };
        if params.mode == SessionMode::Terminal {
            return self
                .start_terminal(slot.clone(), core, adapter.as_ref(), &config, size, cancel)
                .await;
        }
        let start = tokio::select! {
            result = start_in_mode(adapter.as_ref(), config, params.agent_mode.clone()) => {
                Start::Done(result)
            }
            () = cancel.cancelled() => Start::Cancelled,
            () = slot.stop.cancelled() => Start::Stopped,
        };
        let (state, reason, error) = match start {
            Start::Done(Ok(started)) => {
                let modes = started.session.agent_modes();
                if !started.models.is_empty() {
                    self.agents
                        .learn_models(&session.agent, started.models.clone());
                }
                if let Some(modes) = &modes
                    && !modes.available.is_empty()
                {
                    self.agents
                        .learn_agent_modes(&session.agent, modes.available.clone());
                }
                core.agent = Some(started.session);
                let mut change = core.begin();
                change.next.state = SessionState::Idle;
                if started.model.is_some() {
                    change.next.model = started.model;
                }
                change.next.models = started.models;
                if let Some(modes) = modes {
                    change.next.agent_mode = Some(modes.current);
                    change.next.agent_modes = modes.available;
                }
                core.finish(self, change).await?;
                let session = core.session.clone();
                drop(core);
                info!(session = session.id, model = ?session.model, "session ready");
                tokio::spawn(pump(self.clone(), slot, started.events));
                return Ok(session);
            }
            Start::Done(Err(error)) => {
                warn!(session = session.id, %error, "the agent didn't start");
                (SessionState::Failed, error.to_string(), agent_error(error))
            }
            Start::Cancelled => (
                SessionState::Stopped,
                "cancelled while starting".to_owned(),
                ErrorObject::new(ErrorKind::Cancelled, "session/create was cancelled"),
            ),
            Start::Stopped => (
                SessionState::Stopped,
                "stopped while starting".to_owned(),
                ErrorObject::new(
                    ErrorKind::Cancelled,
                    "the session was stopped while it started",
                ),
            ),
        };
        let mut change = core.begin();
        change.next.state = state;
        change.next.reason = Some(reason);
        core.finish(self, change).await?;
        Err(error)
    }

    /// Starts a terminal session's TUI on a new terminal of `size`, and makes the session `idle`;
    /// or `failed` when it can't start, `stopped` when the start was cancelled or stopped
    /// meanwhile.
    async fn start_terminal(
        self: &Arc<Self>,
        slot: Arc<Slot>,
        mut core: tokio::sync::MutexGuard<'_, Core>,
        adapter: &dyn AgentAdapter,
        config: &SessionConfig,
        size: TerminalSize,
        cancel: &Cancel,
    ) -> Result<Session, ErrorObject> {
        let opened = adapter.terminal(config).and_then(|spec| {
            self.terminals.open(&spec, size).map_err(|error| {
                AgentError::Unavailable(format!("can't start {}: {error}", spec.program.display()))
            })
        });
        let terminal = match opened {
            Ok(terminal) => terminal,
            Err(error) => {
                warn!(session = core.session.id, %error, "the terminal didn't start");
                let mut change = core.begin();
                change.next.state = SessionState::Failed;
                change.next.reason = Some(error.to_string());
                core.finish(self, change).await?;
                return Err(agent_error(error));
            }
        };
        if cancel.is_cancelled() || slot.stop.is_cancelled() {
            terminal.stop().await;
            self.terminals.remove(terminal.id());
            let mut change = core.begin();
            change.next.state = SessionState::Stopped;
            change.next.reason = Some("stopped while starting".to_owned());
            core.finish(self, change).await?;
            return Err(ErrorObject::new(
                ErrorKind::Cancelled,
                "the session was stopped or cancelled while it started",
            ));
        }
        core.terminal = Some(terminal.clone());
        core.agent = Some(Box::new(TerminalSession(terminal.clone())));
        let mut change = core.begin();
        change.commit.event(
            &core.stream,
            TerminalOpened {
                terminal_id: terminal.id().to_owned(),
            },
        );
        change.next.terminal_id = Some(terminal.id().to_owned());
        change.next.state = SessionState::Idle;
        core.finish(self, change).await?;
        let session = core.session.clone();
        drop(core);
        info!(
            session = session.id,
            terminal = terminal.id(),
            pid = terminal.pid(),
            "terminal session ready"
        );
        tokio::spawn(terminal_pump(self.clone(), slot, terminal));
        Ok(session)
    }

    /// `session/get`.
    pub fn get(&self, id: &str) -> Result<Session, ErrorObject> {
        Ok(self.slot(id)?.snapshot())
    }

    /// `session/list`: most recent activity first, then newest first. `before` names the last
    /// session of the previous page; the page goes on after it in the same order.
    pub fn list(&self, params: &SessionListParams) -> Result<Vec<SessionSummary>, ErrorObject> {
        let slots: Vec<Arc<Slot>> = self.shared().slots.values().cloned().collect();
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
        let mut sessions: Vec<Session> = slots.iter().map(|slot| slot.snapshot()).collect();
        let key = |session: &Session| (session.activity(), session.id.clone());
        sessions.sort_by(|a, b| (b.activity(), &b.id).cmp(&(a.activity(), &a.id)));
        let after = match &params.before {
            Some(before) => Some(
                sessions
                    .iter()
                    .find(|session| session.id == *before)
                    .map(key)
                    .ok_or_else(|| {
                        ErrorObject::invalid_params(format!(
                            "before: no session {before} to continue after"
                        ))
                    })?,
            ),
            None => None,
        };
        Ok(sessions
            .iter()
            .filter(|session| {
                after.as_ref().is_none_or(|(activity, id)| {
                    (session.activity(), &session.id) < (*activity, id)
                }) && params
                    .project_id
                    .as_ref()
                    .is_none_or(|project| session.project_id.as_ref() == Some(project))
                    && params
                        .state
                        .as_ref()
                        .is_none_or(|state| session.state == *state)
                    && (params.include_archived || !session.archived)
            })
            .take(limit)
            .map(SessionSummary::from)
            .collect())
    }

    /// `session/rename`: sets the title, of any session, live or not.
    pub async fn rename(&self, params: SessionRenameParams) -> Result<(), ErrorObject> {
        let title = params.title.trim();
        if title.is_empty() {
            return Err(ErrorObject::invalid_params("the title is empty"));
        }
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(ErrorObject::invalid_params(format!(
                "the title is longer than {MAX_TITLE_CHARS} characters"
            )));
        }
        let slot = self.slot(&params.session_id)?;
        let mut core = slot.core.lock().await;
        core.rename(self, title.to_owned()).await
    }

    /// `session/prompt`.
    pub async fn prompt(
        &self,
        params: SessionPromptParams,
    ) -> Result<SessionPromptResult, ErrorObject> {
        if params.content.is_empty() {
            return Err(ErrorObject::invalid_params("the prompt is empty"));
        }
        if let Some(block) = params
            .content
            .iter()
            .find(|block| block.as_text().is_none())
        {
            let kind = match block {
                ContentBlock::Image { .. } => "image",
                ContentBlock::Resource { .. } => "resource",
                ContentBlock::Text { .. } => "text",
            };
            return Err(ErrorObject::invalid_params(format!(
                "{kind} content isn't supported yet; send text"
            )));
        }
        let slot = self.slot(&params.session_id)?;
        let mut core = slot.core.lock().await;
        let turn_id = core.prompt(self, params.content).await?;
        Ok(SessionPromptResult { turn_id })
    }

    /// `session/cancel`: stops the running turn, if there is one.
    pub async fn cancel(&self, id: &str) -> Result<(), ErrorObject> {
        let slot = self.slot(id)?;
        let mut core = slot.core.lock().await;
        core.cancel(self).await
    }

    /// `session/setModel`.
    pub async fn set_model(&self, params: SessionSetModelParams) -> Result<(), ErrorObject> {
        if params.model.trim().is_empty() {
            return Err(ErrorObject::invalid_params("the model is empty"));
        }
        let slot = self.slot(&params.session_id)?;
        let mut core = slot.core.lock().await;
        core.set_model(self, params.model).await
    }

    /// `session/setAgentMode`.
    pub async fn set_agent_mode(
        &self,
        params: SessionSetAgentModeParams,
    ) -> Result<(), ErrorObject> {
        if params.agent_mode.trim().is_empty() {
            return Err(ErrorObject::invalid_params("the agent mode is empty"));
        }
        let slot = self.slot(&params.session_id)?;
        let mut core = slot.core.lock().await;
        core.set_agent_mode(self, params.agent_mode).await
    }

    /// `session/stop`: ends the agent's process tree. Stopping a stopped session does nothing.
    pub async fn stop(&self, id: &str) -> Result<(), ErrorObject> {
        let slot = self.slot(id)?;
        slot.stop.cancel();
        let mut core = slot.core.lock().await;
        core.stop(self, "stopped by request").await
    }

    /// `session/archive`: stops the session if needed and hides it from default lists.
    pub async fn archive(&self, id: &str) -> Result<(), ErrorObject> {
        let slot = self.slot(id)?;
        slot.stop.cancel();
        let mut core = slot.core.lock().await;
        core.stop(self, "archived").await?;
        // Its last screen goes too.
        if let Some(terminal) = &core.terminal {
            self.terminals.remove(terminal.id());
        }
        if core.session.archived {
            return Ok(());
        }
        let mut change = core.begin();
        change.next.archived = true;
        change.commit.event(
            &StreamName::index(),
            EntityRemoved {
                entity: EntityKind::Session,
                id: id.to_owned(),
            },
        );
        core.finish(self, change).await
    }

    /// `inbox/list`: newest first.
    pub fn inbox_list(&self, params: &InboxListParams) -> Vec<InboxItem> {
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
        self.shared()
            .inbox
            .values()
            .rev()
            .filter(|item| {
                params
                    .before
                    .as_ref()
                    .is_none_or(|before| item.id < *before)
                    && params.status.is_none_or(|status| item.status == status)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    /// `inbox/get`.
    pub fn inbox_get(&self, params: &InboxItemIdParams) -> Result<InboxItem, ErrorObject> {
        self.shared()
            .inbox
            .get(&params.item_id)
            .cloned()
            .ok_or_else(|| {
                ErrorObject::new(
                    ErrorKind::NotFound,
                    format!("no inbox item {}", params.item_id),
                )
            })
    }

    /// `inbox/respond`: resolves an open item and passes the answer on. `conflict` if it isn't
    /// open any more.
    pub async fn inbox_respond(
        &self,
        params: InboxRespondParams,
        by: &str,
    ) -> Result<InboxItem, ErrorObject> {
        let item = self.inbox_get(&InboxItemIdParams {
            item_id: params.item_id.clone(),
        })?;
        if item.status != InboxItemStatus::Open {
            return Err(already_closed(&item));
        }
        let Some(session_id) = item.source.session_id.clone() else {
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                "only permission requests can be answered so far",
            ));
        };
        let slot = self.slot(&session_id)?;
        let mut core = slot.core.lock().await;
        core.respond(self, &params.item_id, params.action, params.payload, by)
            .await
    }

    /// Stops every live session, for a clean shutdown.
    pub async fn shutdown(&self) {
        let slots: Vec<Arc<Slot>> = self.shared().slots.values().cloned().collect();
        let stops = slots
            .iter()
            .filter(|slot| slot.snapshot().state.is_live())
            .map(|slot| async move {
                slot.stop.cancel();
                let mut core = slot.core.lock().await;
                if let Err(error) = core.stop(self, "burrowd shut down").await {
                    warn!(session = core.session.id, %error, "stopping failed");
                }
            });
        let all = futures::future::join_all(stops);
        if tokio::time::timeout(SHUTDOWN_WAIT, all).await.is_err() {
            warn!("some sessions didn't stop in time; their processes die with burrowd");
        }
    }
}

impl Core {
    /// Starts a change. Buffered text is committed first, so it stays before what follows.
    fn begin(&mut self) -> Change {
        let mut commit = Commit::new();
        if let Some(buffer) = self.buffer.take() {
            let event: Event = if buffer.thought {
                AgentThoughtChunk {
                    turn_id: buffer.turn_id,
                    text: buffer.text,
                }
                .into()
            } else {
                AgentMessageChunk {
                    turn_id: buffer.turn_id,
                    text: buffer.text,
                }
                .into()
            };
            commit.event(&self.stream, event);
        }
        Change {
            commit,
            next: self.session.clone(),
            items: Vec::new(),
        }
    }

    /// Commits a change: adds `stateChanged` if the state changed, `entityStateChanged` if the
    /// state or the last activity did, and the rows of whatever changed. Memory follows only
    /// once the commit succeeded.
    async fn finish(&mut self, ctx: &Sessions, mut change: Change) -> Result<(), ErrorObject> {
        let state_changed = change.next.state != self.session.state;
        if state_changed {
            let reason = match change.next.state {
                SessionState::Stopped | SessionState::Failed => change.next.reason.clone(),
                _ => None,
            };
            change.commit.event(
                &self.stream,
                StateChanged {
                    state: change.next.state.clone(),
                    reason,
                },
            );
        }
        if state_changed || change.next.last_activity_at != self.session.last_activity_at {
            change
                .commit
                .event(&StreamName::index(), state_event(&change.next));
        }
        let session_changed = change.next != self.session;
        if session_changed {
            change.next.updated_at = Timestamp::now();
            change.commit.put(SESSION, &change.next.id, &change.next);
        }
        for item in &change.items {
            change.commit.put(INBOX, &item.id, item);
        }
        if !change.commit.is_empty() {
            ctx.hub.commit(change.commit).await.map_err(internal)?;
        }
        self.session = change.next;
        if session_changed {
            *lock(&self.snapshot) = self.session.clone();
        }
        ctx.publish(&self.session, &change.items);
        Ok(())
    }

    /// Commits buffered text now.
    async fn flush(&mut self, ctx: &Sessions) -> Result<(), ErrorObject> {
        if self.buffer.is_none() {
            return Ok(());
        }
        let change = self.begin();
        self.finish(ctx, change).await
    }

    /// When buffered text is due.
    fn due(&self) -> Option<Instant> {
        self.buffer.as_ref().map(|buffer| buffer.due)
    }

    async fn flush_due(&mut self, ctx: &Sessions) -> Result<(), ErrorObject> {
        match self.due() {
            Some(due) if due <= Instant::now() => self.flush(ctx).await,
            _ => Ok(()),
        }
    }

    /// Handles one event from the agent.
    async fn handle(&mut self, ctx: &Sessions, event: AgentEvent) -> Result<(), ErrorObject> {
        if self.agent.is_none() {
            // Stopped already: whatever the agent still says is moot.
            if let AgentEvent::Permission(request) = event {
                request.reply.send(PermissionOutcome::Cancelled);
            }
            return Ok(());
        }
        match event {
            AgentEvent::MessageChunk(text) => self.text(ctx, false, text).await,
            AgentEvent::ThoughtChunk(text) => self.text(ctx, true, text).await,
            AgentEvent::ToolCall(mut call) => {
                if call.turn_id.is_none() {
                    call.turn_id.clone_from(&self.session.turn_id);
                }
                let mut change = self.begin();
                change.commit.event(&self.stream, call);
                self.finish(ctx, change).await
            }
            AgentEvent::ToolCallUpdate(update) => {
                let mut change = self.begin();
                change.commit.event(&self.stream, update);
                self.finish(ctx, change).await
            }
            AgentEvent::Plan(plan) => {
                let mut change = self.begin();
                change.commit.event(&self.stream, plan);
                self.finish(ctx, change).await
            }
            AgentEvent::Permission(request) => self.permission(ctx, request).await,
            AgentEvent::PermissionWithdrawn(permission) => self.withdrawn(ctx, permission).await,
            AgentEvent::TurnEnded { reason, message } => {
                if self.session.turn_id.is_none() {
                    debug!(
                        session = self.session.id,
                        "a turn ended that wasn't running"
                    );
                    return Ok(());
                }
                let mut change = self.begin();
                self.expire_pending(&mut change);
                let limited = reason == TurnEndReason::Limit || change.next.limit.is_some();
                self.end_turn(&mut change, reason, message);
                change.next.state = if limited {
                    SessionState::Limited
                } else {
                    SessionState::Idle
                };
                self.finish(ctx, change).await
            }
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
                cost_usd,
            } => {
                let mut change = self.begin();
                change.commit.event(
                    &self.stream,
                    Usage {
                        turn_id: self.session.turn_id.clone(),
                        input_tokens,
                        output_tokens,
                        cost_usd,
                    },
                );
                let total = change.next.usage.get_or_insert_with(Default::default);
                total.input_tokens += input_tokens;
                total.output_tokens += output_tokens;
                total.cost_usd = match (total.cost_usd, cost_usd) {
                    (Some(total), Some(more)) => Some(total + more),
                    (total, more) => total.or(more),
                };
                self.finish(ctx, change).await
            }
            AgentEvent::LimitReached {
                provider,
                resets_at,
                message,
            } => {
                let provider = provider.unwrap_or_else(|| self.session.agent.to_string());
                ctx.agents.report_usage(ProviderUsage {
                    provider: provider.clone(),
                    agent: Some(self.session.agent.clone()),
                    used: None,
                    limit: None,
                    unit: None,
                    limited: true,
                    resets_at,
                    message: Some(message.clone()),
                    source: UsageSource::Reported,
                    updated_at: Timestamp::now(),
                });
                let mut change = self.begin();
                change.commit.event(
                    &self.stream,
                    LimitReached {
                        provider: provider.clone(),
                        resets_at,
                        message: message.clone(),
                    },
                );
                change.next.limit = Some(LimitInfo {
                    provider,
                    resets_at,
                    message,
                });
                change.next.state = SessionState::Limited;
                self.finish(ctx, change).await
            }
            AgentEvent::ProviderUsage(usage) => {
                ctx.agents.report_usage(usage);
                Ok(())
            }
            AgentEvent::ModelChanged(model) => self.model_changed(ctx, model).await,
            AgentEvent::AgentModeChanged(mode) => self.agent_mode_changed(ctx, mode).await,
            AgentEvent::Exited { error } => {
                self.agent = None;
                let mut change = self.begin();
                self.expire_pending(&mut change);
                let (state, reason, turn_reason) = match error {
                    Some(error) => (SessionState::Failed, error, TurnEndReason::Error),
                    None => (
                        SessionState::Stopped,
                        "the agent exited".to_owned(),
                        TurnEndReason::Cancelled,
                    ),
                };
                warn!(session = self.session.id, %state, reason, "the agent process ended");
                self.end_turn(&mut change, turn_reason, Some(reason.clone()));
                change.next.state = state;
                change.next.reason = Some(reason);
                self.finish(ctx, change).await
            }
        }
    }

    /// Buffers agent text, committing what was buffered before if it was another kind or turn.
    async fn text(
        &mut self,
        ctx: &Sessions,
        thought: bool,
        text: String,
    ) -> Result<(), ErrorObject> {
        let Some(turn_id) = self.session.turn_id.clone() else {
            debug!(
                session = self.session.id,
                "dropping agent text outside a turn"
            );
            return Ok(());
        };
        if let Some(buffer) = &mut self.buffer
            && buffer.thought == thought
            && buffer.turn_id == turn_id
        {
            buffer.text.push_str(&text);
            if buffer.text.len() >= CHUNK_MAX_BYTES {
                return self.flush(ctx).await;
            }
            return Ok(());
        }
        self.flush(ctx).await?;
        self.buffer = Some(TextBuffer {
            thought,
            turn_id,
            text,
            due: Instant::now() + CHUNK_WINDOW,
        });
        Ok(())
    }

    /// A permission request becomes an open inbox item; the session waits.
    async fn permission(
        &mut self,
        ctx: &Sessions,
        request: PermissionRequest,
    ) -> Result<(), ErrorObject> {
        let PermissionRequest {
            tool_call_id,
            title,
            options,
            reply,
        } = request;
        let agent = ctx
            .agents
            .registry()
            .get(&self.session.agent)
            .map_or_else(|| self.session.agent.to_string(), |a| a.name().to_owned());
        let label = self
            .session
            .title
            .clone()
            .unwrap_or_else(|| self.session.id.clone());
        let source = InboxSource {
            session_id: Some(self.session.id.clone()),
            tool_call_id: Some(tool_call_id.clone()),
            ..InboxSource::default()
        };
        let item = InboxItem {
            id: ids::new_id("inb"),
            kind: InboxItemKind::Permission,
            title: title.clone(),
            body: Some(format!(
                "**{agent}** asks before this tool call, in session *{label}*."
            )),
            source: source.clone(),
            actions: options
                .iter()
                .map(|option| InboxAction {
                    id: option.option_id.clone(),
                    label: option.name.clone(),
                    style: match option.kind {
                        PermissionOptionKind::AllowOnce => Some(InboxActionStyle::Primary),
                        PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways => {
                            Some(InboxActionStyle::Danger)
                        }
                        _ => None,
                    },
                })
                .collect(),
            form: None,
            attachments: Vec::new(),
            status: InboxItemStatus::Open,
            resolution: None,
            created_at: Timestamp::now(),
            expires_at: None,
            closed_at: None,
        };
        let mut change = self.begin();
        change
            .commit
            .event(
                &self.stream,
                PermissionRequested {
                    inbox_item_id: item.id.clone(),
                    turn_id: self.session.turn_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    title: title.clone(),
                    options,
                },
            )
            .event(
                &StreamName::index(),
                InboxItemAdded {
                    item_id: item.id.clone(),
                    kind: InboxItemKind::Permission,
                    title,
                    source,
                    actions: Some(item.actions.clone()),
                },
            );
        if change.next.state == SessionState::Working {
            change.next.state = SessionState::Waiting;
        }
        change.items.push(item.clone());
        info!(
            session = self.session.id,
            item = item.id,
            "permission requested"
        );
        self.pending.insert(
            item.id.clone(),
            Pending {
                item,
                tool_call_id,
                reply,
            },
        );
        self.finish(ctx, change).await
    }

    /// The agent withdrew a permission request: its item expires now, and the session works
    /// again once no other request is open. A request answered or expired meanwhile is ignored.
    async fn withdrawn(
        &mut self,
        ctx: &Sessions,
        permission: PermissionId,
    ) -> Result<(), ErrorObject> {
        let Some(id) = self
            .pending
            .iter()
            .find(|(_, pending)| pending.reply.id() == permission)
            .map(|(id, _)| id.clone())
        else {
            debug!(
                session = self.session.id,
                %permission,
                "the agent withdrew a request that is no longer open"
            );
            return Ok(());
        };
        let pending = self.pending.remove(&id).expect("found above");
        let mut change = self.begin();
        // The agent stopped waiting: its reply is dropped, which an adapter reads as cancelled.
        drop(expire_one(&self.stream, &mut change, &id, pending));
        if self.pending.is_empty() && change.next.state == SessionState::Waiting {
            change.next.state = SessionState::Working;
        }
        info!(
            session = self.session.id,
            item = id,
            "the agent withdrew its permission request"
        );
        self.finish(ctx, change).await
    }

    /// Expires every open permission request and tells the agent nobody answered.
    fn expire_pending(&mut self, change: &mut Change) {
        for (id, pending) in std::mem::take(&mut self.pending) {
            let reply = expire_one(&self.stream, change, &id, pending);
            reply.send(PermissionOutcome::Cancelled);
        }
        if change.next.state == SessionState::Waiting {
            change.next.state = SessionState::Working;
        }
    }

    /// Ends the running turn, if any, and records the activity.
    fn end_turn(&mut self, change: &mut Change, reason: TurnEndReason, message: Option<String>) {
        if let Some(turn_id) = change.next.turn_id.take() {
            change.commit.event(
                &self.stream,
                TurnEnded {
                    turn_id,
                    reason,
                    message,
                },
            );
            change.next.last_activity_at = Some(Timestamp::now());
        }
    }

    async fn rename(&mut self, ctx: &Sessions, title: String) -> Result<(), ErrorObject> {
        if self.session.title.as_deref() == Some(title.as_str()) {
            return Ok(());
        }
        let mut change = self.begin();
        change.commit.event(
            &StreamName::index(),
            EntityRenamed {
                entity: EntityKind::Session,
                id: self.session.id.clone(),
                title: title.clone(),
            },
        );
        change.next.title = Some(title);
        self.finish(ctx, change).await
    }

    async fn prompt(
        &mut self,
        ctx: &Sessions,
        content: Vec<ContentBlock>,
    ) -> Result<String, ErrorObject> {
        if self.session.mode == SessionMode::Terminal {
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                "a terminal session takes its input through terminal/input",
            ));
        }
        if self.agent.is_none() {
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                format!("the session is {}", self.session.state),
            ));
        }
        if self.session.turn_id.is_some() {
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                "a turn is already running; cancel it or wait for it to end",
            ));
        }
        if self.session.state == SessionState::Limited
            && let Some(limit) = &self.session.limit
            && let Some(resets_at) = limit.resets_at
            && resets_at > Timestamp::now()
        {
            return Err(ErrorObject::limit_reached(
                format!("{}: {}", limit.provider, limit.message),
                Some(resets_at),
            ));
        }

        let turn_id = ids::new_id("trn");
        let mut change = self.begin();
        change.commit.event(
            &self.stream,
            UserMessage {
                turn_id: turn_id.clone(),
                content: content.clone(),
            },
        );
        if change.next.title.is_none()
            && let Some(title) = title_from(&content)
        {
            change.commit.event(
                &StreamName::index(),
                EntityRenamed {
                    entity: EntityKind::Session,
                    id: self.session.id.clone(),
                    title: title.clone(),
                },
            );
            change.next.title = Some(title);
        }
        change.next.turn_id = Some(turn_id.clone());
        change.next.state = SessionState::Working;
        change.next.limit = None;
        change.next.last_activity_at = Some(Timestamp::now());
        self.finish(ctx, change).await?;

        let sent = self
            .agent
            .as_ref()
            .expect("checked above")
            .prompt(content)
            .await;
        if let Err(error) = sent {
            let mut change = self.begin();
            self.end_turn(&mut change, TurnEndReason::Error, Some(error.to_string()));
            change.next.state = SessionState::Idle;
            self.finish(ctx, change).await?;
            return Err(agent_error(error));
        }
        Ok(turn_id)
    }

    async fn cancel(&mut self, ctx: &Sessions) -> Result<(), ErrorObject> {
        if self.agent.is_none() || self.session.turn_id.is_none() {
            return Ok(());
        }
        if !self.pending.is_empty() {
            let mut change = self.begin();
            self.expire_pending(&mut change);
            self.finish(ctx, change).await?;
        }
        let cancelled = self.agent.as_ref().expect("checked above").cancel().await;
        // The turn ends when the agent says so, with turnEnded (cancelled).
        cancelled.map_err(agent_error)
    }

    async fn set_model(&mut self, ctx: &Sessions, model: String) -> Result<(), ErrorObject> {
        let Some(agent) = &self.agent else {
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                format!("the session is {}", self.session.state),
            ));
        };
        // A model may be named by its display name, such as Cursor's `gpt-5.5` for
        // `gpt-5.5[context=272k,…]` (D-032). The agent gets, and the session records, the id.
        let model =
            find_model(&self.session.models, &model).map_or(model, |found| found.id.clone());
        agent.set_model(model.clone()).await.map_err(agent_error)?;
        self.model_changed(ctx, model).await
    }

    async fn model_changed(&mut self, ctx: &Sessions, model: String) -> Result<(), ErrorObject> {
        if self.session.model.as_deref() == Some(model.as_str()) {
            return Ok(());
        }
        let mut change = self.begin();
        change.commit.event(
            &self.stream,
            ModelChanged {
                model: model.clone(),
            },
        );
        change.next.model = Some(model);
        self.finish(ctx, change).await
    }

    async fn set_agent_mode(&mut self, ctx: &Sessions, mode: String) -> Result<(), ErrorObject> {
        let Some(agent) = &self.agent else {
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                format!("the session is {}", self.session.state),
            ));
        };
        agent
            .set_agent_mode(mode.clone())
            .await
            .map_err(agent_error)?;
        self.agent_mode_changed(ctx, mode).await
    }

    async fn agent_mode_changed(
        &mut self,
        ctx: &Sessions,
        mode: String,
    ) -> Result<(), ErrorObject> {
        if self.session.agent_mode.as_deref() == Some(mode.as_str()) {
            return Ok(());
        }
        let mut change = self.begin();
        change.commit.event(
            &self.stream,
            AgentModeChanged {
                agent_mode: mode.clone(),
            },
        );
        change.next.agent_mode = Some(mode);
        self.finish(ctx, change).await
    }

    /// Ends the agent's process tree and marks the session `stopped`.
    async fn stop(&mut self, ctx: &Sessions, reason: &str) -> Result<(), ErrorObject> {
        let Some(agent) = self.agent.take() else {
            return Ok(());
        };
        agent.stop().await;
        drop(agent);
        let mut change = self.begin();
        if let Some(terminal) = &self.terminal {
            change.commit.event(
                &self.stream,
                TerminalClosed {
                    terminal_id: terminal.id().to_owned(),
                    exit_code: terminal.exit().and_then(|exit| exit.exit_code),
                },
            );
        }
        self.expire_pending(&mut change);
        self.end_turn(
            &mut change,
            TurnEndReason::Cancelled,
            Some("the session was stopped".into()),
        );
        change.next.state = SessionState::Stopped;
        change.next.reason = Some(reason.to_owned());
        self.finish(ctx, change).await?;
        info!(session = self.session.id, reason, "session stopped");
        Ok(())
    }

    async fn respond(
        &mut self,
        ctx: &Sessions,
        item_id: &str,
        action: String,
        payload: Option<Value>,
        by: &str,
    ) -> Result<InboxItem, ErrorObject> {
        let Some(pending) = self.pending.get(item_id) else {
            // Answered or expired while this request waited for the session.
            let item = ctx.inbox_get(&InboxItemIdParams {
                item_id: item_id.to_owned(),
            })?;
            return Err(already_closed(&item));
        };
        if !pending.item.actions.iter().any(|known| known.id == action) {
            let actions: Vec<&str> = pending
                .item
                .actions
                .iter()
                .map(|known| known.id.as_str())
                .collect();
            return Err(ErrorObject::invalid_params(format!(
                "{action:?} isn't an action of {item_id}; it has {}",
                actions.join(", ")
            )));
        }
        let Pending {
            mut item,
            tool_call_id,
            reply,
        } = self.pending.remove(item_id).expect("found above");
        let now = Timestamp::now();
        item.status = InboxItemStatus::Resolved;
        item.resolution = Some(InboxResolution {
            action: action.clone(),
            payload,
            by: by.to_owned(),
            at: now,
        });
        item.closed_at = Some(now);
        let mut change = self.begin();
        change
            .commit
            .event(
                &self.stream,
                PermissionResolved {
                    inbox_item_id: item.id.clone(),
                    tool_call_id,
                    status: InboxItemStatus::Resolved,
                    option_id: Some(action.clone()),
                    by: Some(by.to_owned()),
                },
            )
            .event(
                &StreamName::index(),
                InboxItemResolved {
                    item_id: item.id.clone(),
                    status: InboxItemStatus::Resolved,
                    action: Some(action.clone()),
                    by: Some(by.to_owned()),
                },
            );
        if self.pending.is_empty() && change.next.state == SessionState::Waiting {
            change.next.state = SessionState::Working;
        }
        change.items.push(item.clone());
        self.finish(ctx, change).await?;
        info!(
            session = self.session.id,
            item = item.id,
            action,
            "permission answered"
        );
        reply.send(PermissionOutcome::Selected(action));
        Ok(item)
    }
}

/// Reads one session's agent events until the agent exits, committing buffered text when due.
async fn pump(
    sessions: Arc<Sessions>,
    slot: Arc<Slot>,
    mut events: mpsc::UnboundedReceiver<AgentEvent>,
) {
    let mut due: Option<Instant> = None;
    loop {
        let next = match due {
            Some(at) => tokio::select! {
                event = events.recv() => Some(event),
                () = tokio::time::sleep_until(at) => None,
            },
            None => Some(events.recv().await),
        };
        let mut core = slot.core.lock().await;
        let last = match next {
            None => {
                if let Err(error) = core.flush_due(&sessions).await {
                    warn!(session = core.session.id, %error, "committing agent text failed");
                }
                false
            }
            Some(event) => {
                let event = event.unwrap_or_else(|| AgentEvent::Exited {
                    error: Some("the adapter stopped without saying why".into()),
                });
                let last = matches!(event, AgentEvent::Exited { .. });
                if let Err(error) = core.handle(&sessions, event).await {
                    warn!(session = core.session.id, %error, "handling an agent event failed");
                }
                last
            }
        };
        due = core.due();
        drop(core);
        if last {
            break;
        }
    }
}

/// Waits for a terminal session's program to end by itself, then ends the session: `stopped`
/// for exit code 0, `failed` otherwise. A session stopped meanwhile was ended by its stop.
async fn terminal_pump(sessions: Arc<Sessions>, slot: Arc<Slot>, terminal: Arc<Terminal>) {
    let exit = terminal.closed().await;
    let mut core = slot.core.lock().await;
    if core.agent.take().is_none() {
        return;
    }
    let mut change = core.begin();
    change.commit.event(
        &core.stream,
        TerminalClosed {
            terminal_id: terminal.id().to_owned(),
            exit_code: exit.exit_code,
        },
    );
    let (state, reason) = match exit.exit_code {
        Some(0) => (SessionState::Stopped, "the program exited".to_owned()),
        Some(code) => (
            SessionState::Failed,
            format!("the program exited with code {code}"),
        ),
        None => (
            SessionState::Failed,
            "the program ended; its exit code is unknown".to_owned(),
        ),
    };
    info!(session = core.session.id, terminal = terminal.id(), ?exit.exit_code, "the terminal's program ended");
    change.next.state = state;
    change.next.reason = Some(reason);
    if let Err(error) = core.finish(&sessions, change).await {
        warn!(session = core.session.id, %error, "ending the terminal session failed");
    }
}

/// A terminal session's handle in its core: there are no turns, models or modes to drive, only
/// a terminal to stop. Dropping it kills the program's tree.
struct TerminalSession(Arc<Terminal>);

impl AgentSession for TerminalSession {
    fn prompt(&self, _content: Vec<ContentBlock>) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async {
            Err(AgentError::InvalidState(
                "a terminal session takes its input through terminal/input".into(),
            ))
        })
    }

    fn cancel(&self) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async { Ok(()) })
    }

    fn set_model(&self, _model: String) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async {
            Err(AgentError::InvalidState(
                "a terminal session's model is chosen in the agent's TUI".into(),
            ))
        })
    }

    fn set_agent_mode(&self, _mode: String) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async {
            Err(AgentError::InvalidState(
                "a terminal session's mode is chosen in the agent's TUI".into(),
            ))
        })
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {
            self.0.stop().await;
        })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.0.kill();
    }
}

/// The size a terminal session starts at: `session/create`'s `cols` and `rows`, else 120 × 32.
/// Both or neither, within `terminal/resize`'s bounds, and only for terminal sessions.
fn terminal_size(params: &SessionCreateParams) -> Result<TerminalSize, ErrorObject> {
    let size = match (params.cols, params.rows) {
        (None, None) => return Ok(TerminalSize::DEFAULT),
        (Some(cols), Some(rows)) => TerminalSize { cols, rows },
        _ => {
            return Err(ErrorObject::invalid_params(
                "session/create takes cols and rows together",
            ));
        }
    };
    if params.mode != SessionMode::Terminal {
        return Err(ErrorObject::invalid_params(
            "cols and rows are for terminal sessions only",
        ));
    }
    if !size.is_valid() {
        return Err(ErrorObject::invalid_params(format!(
            "a terminal is {}–{} columns by {}–{} rows, not {} × {}",
            TerminalSize::MIN_COLS,
            TerminalSize::MAX_COLS,
            TerminalSize::MIN_ROWS,
            TerminalSize::MAX_ROWS,
            size.cols,
            size.rows
        )));
    }
    Ok(size)
}

/// The terminal a terminal session's stream announced, from its first events.
async fn terminal_of(hub: &Hub, stream: &StreamName) -> Result<Option<String>, HubError> {
    let events = hub.read(stream.clone(), 0, 16).await?;
    Ok(events
        .iter()
        .find_map(|event| match event.payload::<Event>() {
            Ok(Event::TerminalOpened(opened)) => Some(opened.terminal_id),
            _ => None,
        }))
}

/// Starts the agent, then switches it to the mode `session/create` asked for. If that fails, the
/// agent is stopped again.
async fn start_in_mode(
    adapter: &dyn AgentAdapter,
    config: SessionConfig,
    mode: Option<String>,
) -> Result<burrow_agents::StartedSession, AgentError> {
    let started = adapter.start(config).await?;
    let Some(mode) = mode else {
        return Ok(started);
    };
    let current = started.session.agent_modes().map(|modes| modes.current);
    if current.as_deref() == Some(mode.as_str()) {
        return Ok(started);
    }
    if let Err(error) = started.session.set_agent_mode(mode).await {
        started.session.stop().await;
        return Err(error);
    }
    Ok(started)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn internal(error: HubError) -> ErrorObject {
    ErrorObject::internal(error.to_string())
}

fn agent_error(error: AgentError) -> ErrorObject {
    match error {
        AgentError::Unavailable(message) => ErrorObject::new(ErrorKind::Unavailable, message),
        AgentError::InvalidState(message) => ErrorObject::new(ErrorKind::InvalidState, message),
        AgentError::Invalid(message) => ErrorObject::invalid_params(message),
        AgentError::LimitReached { message, resets_at } => {
            ErrorObject::limit_reached(message, resets_at)
        }
        AgentError::Failed(message) => ErrorObject::internal(message),
    }
}

fn already_closed(item: &InboxItem) -> ErrorObject {
    let status = match item.status {
        InboxItemStatus::Open => "open",
        InboxItemStatus::Resolved => "answered",
        InboxItemStatus::Expired => "expired",
    };
    ErrorObject::new(
        ErrorKind::Conflict,
        format!("inbox item {} is already {status}", item.id),
    )
}

fn expire(item: &mut InboxItem, now: Timestamp) {
    item.status = InboxItemStatus::Expired;
    item.closed_at = Some(now);
}

/// Expires one open permission request in `change`: `permissionResolved` and
/// `inboxItemResolved`, both `expired`, and the item's row. Returns its reply, for the caller
/// to answer or drop.
fn expire_one(
    stream: &StreamName,
    change: &mut Change,
    id: &str,
    pending: Pending,
) -> PermissionReply {
    let Pending {
        mut item,
        tool_call_id,
        reply,
    } = pending;
    expire(&mut item, Timestamp::now());
    change
        .commit
        .event(
            stream,
            PermissionResolved {
                inbox_item_id: id.to_owned(),
                tool_call_id,
                status: InboxItemStatus::Expired,
                option_id: None,
                by: None,
            },
        )
        .event(&StreamName::index(), expired_event(id));
    change.items.push(item);
    reply
}

fn expired_event(item_id: &str) -> InboxItemResolved {
    InboxItemResolved {
        item_id: item_id.to_owned(),
        status: InboxItemStatus::Expired,
        action: None,
        by: None,
    }
}

fn state_event(session: &Session) -> EntityStateChanged {
    EntityStateChanged {
        entity: EntityKind::Session,
        id: session.id.clone(),
        state: session.state.to_string(),
        last_activity_at: Some(session.activity()),
    }
}

/// A title from the first line of a prompt, at most [`TITLE_CHARS`] characters.
fn title_from(content: &[ContentBlock]) -> Option<String> {
    let text = content.iter().find_map(ContentBlock::as_text)?;
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= TITLE_CHARS {
        return Some(line);
    }
    let mut title: String = line.chars().take(TITLE_CHARS - 1).collect();
    title.push('…');
    Some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_come_from_the_first_line_and_are_shortened() {
        let text = |text: &str| vec![ContentBlock::text(text)];
        assert_eq!(
            title_from(&text("\n  Fix   the login bug \nmore")).as_deref(),
            Some("Fix the login bug")
        );
        let long = "x".repeat(100);
        let title = title_from(&text(&long)).unwrap();
        assert_eq!(title.chars().count(), TITLE_CHARS);
        assert!(title.ends_with('…'));
        assert_eq!(title_from(&text("   \n ")), None);
    }
}
