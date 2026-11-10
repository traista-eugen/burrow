//! Shared state: what the app knows about `burrowd`, as GPUI entities that views observe.
//!
//! - [`AppState`] (one per app): the connection status, the lists from the `index` stream
//!   ([`index::IndexModel`]), the agents from `agent/list` with any installs running, and
//!   provider usage from `usage/get`. It also owns the connection loop and the index
//!   subscription, and has the calls views make (`prompt`, `respond`, `set_model`, …).
//! - [`session::SessionStore`] (one per open session): the session's events and metadata.
//! - [`terminals::TerminalHub`] (one per app, owned by [`AppState`]): attachments to the
//!   terminals of terminal sessions, routed to their views and kept across reconnects.
//! - [`transcript::TranscriptModel`]: a session's events grouped into the transcript's rows; the
//!   transcript view keeps one per open session.
//!
//! The plain models ([`index::IndexModel`], [`session::SessionLog`],
//! [`transcript::TranscriptModel`]) hold the logic and have no GPUI in them; the entities wire
//! them to `burrowd` and to views.

pub mod index;
pub mod session;
pub mod terminals;
pub mod transcript;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use burrow_client::{CallError, Client};
use burrow_protocol::methods::{
    AgentInstall, AgentList, AgentRefresh, InboxGet, InboxList, InboxRespond, ProjectAdd,
    ProjectGet, ProjectList, SessionArchive, SessionCancel, SessionCreate, SessionGet, SessionList,
    SessionPrompt, SessionRename, SessionSetAgentMode, SessionSetModel, SessionStop, UsageGet,
};
use burrow_protocol::{
    AgentId, AgentInfo, AgentInstallParams, ContentBlock, Empty, InboxItem, InboxItemIdParams,
    InboxItemStatus, InboxListParams, InboxRespondParams, Project, ProjectAddParams,
    ProjectIdParams, ProviderUsage, Session, SessionCreateParams, SessionIdParams,
    SessionListParams, SessionPromptParams, SessionPromptResult, SessionRenameParams,
    SessionSetAgentModeParams, SessionSetModelParams, SessionSummary, StreamName,
};
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Task};
use tracing::warn;

use self::index::{Applied, Fetch, IndexModel};
use self::terminals::TerminalHub;
use crate::backend::{self, Backend, ConnectionStatus, FollowEvent};

/// An `agent/install` this app started.
#[derive(Clone, Debug, PartialEq)]
pub enum InstallStatus {
    /// npm is running; it takes a few seconds.
    Running {
        /// When it started, for the elapsed time.
        since: Instant,
    },
    /// It failed; the message says why.
    Failed(String),
}

/// What [`AppState`] tells views besides "something changed".
#[derive(Clone, Debug, PartialEq)]
pub enum AppStateEvent {
    /// This app archived a session; a view showing it closes it.
    Archived(String),
}

/// What the app knows about `burrowd`. Views observe it and re-render when it notifies.
pub struct AppState {
    connection: ConnectionStatus,
    index: IndexModel,
    agents: Vec<AgentInfo>,
    agents_loaded: bool,
    /// `agent/install` calls started here, by agent; a success removes its entry.
    installs: HashMap<AgentId, InstallStatus>,
    /// Each provider's usage and limit, from `usage/get`.
    usage: Vec<ProviderUsage>,
    /// Bumped by every list load, so the answer of a superseded load is ignored.
    load_generation: u64,
    /// Fetches in flight, so one event storm doesn't fetch the same thing twice.
    fetching: HashSet<Fetch>,
    terminals: Entity<TerminalHub>,
    _connection: Task<()>,
    _index: Task<()>,
}

impl EventEmitter<AppStateEvent> for AppState {}

impl AppState {
    /// Creates the state and starts connecting. Needs the [`Backend`] global.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx).clone();
        let connection = cx.spawn({
            let backend = backend.clone();
            async move |this, cx| {
                backend::run_connection(
                    backend,
                    this,
                    |this: &mut AppState, status, client, cx| this.on_status(status, client, cx),
                    cx,
                )
                .await
            }
        });
        let index = cx.spawn(async move |this, cx| {
            backend::follow(
                backend,
                this,
                StreamName::index(),
                |this: &AppState| this.index.is_loaded().then(|| this.index.seq()),
                |this, event, cx| this.on_index(event, cx),
                cx,
            )
            .await
        });
        Self {
            connection: ConnectionStatus::Connecting { attempt: 0 },
            index: IndexModel::new(),
            agents: Vec::new(),
            agents_loaded: false,
            installs: HashMap::new(),
            usage: Vec::new(),
            load_generation: 0,
            fetching: HashSet::new(),
            terminals: cx.new(TerminalHub::new),
            _connection: connection,
            _index: index,
        }
    }

    /// The connection's status.
    pub fn connection(&self) -> &ConnectionStatus {
        &self.connection
    }

    /// Projects, sessions and open inbox items.
    pub fn index(&self) -> &IndexModel {
        &self.index
    }

    /// The attachments to terminals.
    pub fn terminals(&self) -> &Entity<TerminalHub> {
        &self.terminals
    }

    /// The agents `burrowd` offers, as last detected.
    pub fn agents(&self) -> &[AgentInfo] {
        &self.agents
    }

    /// Whether `agent/list` has answered on this connection.
    pub fn agents_loaded(&self) -> bool {
        self.agents_loaded
    }

    /// One agent, as last detected.
    pub fn agent(&self, id: &AgentId) -> Option<&AgentInfo> {
        self.agents.iter().find(|agent| &agent.id == id)
    }

    /// An agent's display name, or its id.
    pub fn agent_name(&self, id: &AgentId) -> String {
        self.agent(id)
            .map(|agent| agent.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    /// The install of `agent` this app started, while it runs or after it failed.
    pub fn install_status(&self, agent: &AgentId) -> Option<&InstallStatus> {
        self.installs.get(agent)
    }

    /// Each provider's usage and limit, as `usage/get` last answered.
    pub fn usage(&self) -> &[ProviderUsage] {
        &self.usage
    }

    fn on_status(
        &mut self,
        status: ConnectionStatus,
        client: Option<&Client>,
        cx: &mut Context<Self>,
    ) {
        if client.is_some() {
            self.refresh_agents(cx);
            self.refresh_usage(cx);
        } else if !status.is_connected() {
            self.agents_loaded = false;
        }
        self.connection = status;
        cx.notify();
    }

    fn on_index(&mut self, event: FollowEvent, cx: &mut Context<Self>) {
        match event {
            FollowEvent::Subscribed { head, reset } => {
                if reset || !self.index.is_loaded() {
                    // Live events follow `head`; the lists cover everything up to it.
                    self.index.set_seq(head);
                    self.reload_lists(cx);
                }
            }
            FollowEvent::Events(events) => {
                let mut applied = Applied::default();
                for event in &events {
                    let one = self.index.apply(event);
                    applied.changed |= one.changed;
                    applied.fetch.extend(one.fetch);
                }
                self.handle_applied(applied, cx);
            }
        }
    }

    /// Loads the lists again, buffering index events meanwhile.
    fn reload_lists(&mut self, cx: &mut Context<Self>) {
        self.load_generation += 1;
        let generation = self.load_generation;
        self.index.begin_load();
        let backend = Backend::global(cx).clone();
        let projects = backend.call::<ProjectList>(Empty {}, cx);
        // Archived sessions too: the sidebar can show them.
        let sessions = backend.call::<SessionList>(
            SessionListParams {
                include_archived: true,
                limit: Some(1000),
                ..Default::default()
            },
            cx,
        );
        let inbox = backend.call::<InboxList>(
            InboxListParams {
                status: Some(InboxItemStatus::Open),
                limit: Some(1000),
                ..Default::default()
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let lists =
                async { Ok::<_, CallError>((projects.await?, sessions.await?, inbox.await?)) };
            let result = lists.await;
            let retry = this
                .update(cx, |this, cx| {
                    if this.load_generation != generation {
                        return false;
                    }
                    match result {
                        Ok((projects, sessions, inbox)) => {
                            let applied = this.index.finish_load(projects, sessions, inbox);
                            this.handle_applied(applied, cx);
                            false
                        }
                        Err(error) => {
                            warn!(%error, "couldn't load the lists");
                            this.index.abandon_load();
                            // Disconnected: the index subscription loads again on reconnect.
                            !matches!(error, CallError::Disconnected)
                        }
                    }
                })
                .unwrap_or(false);
            if retry {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let _ = this.update(cx, |this, cx| {
                    if this.load_generation == generation {
                        this.reload_lists(cx);
                    }
                });
            }
        })
        .detach();
    }

    fn handle_applied(&mut self, applied: Applied, cx: &mut Context<Self>) {
        for fetch in applied.fetch {
            if self.fetching.insert(fetch.clone()) {
                self.fetch(fetch, cx);
            }
        }
        if applied.changed {
            cx.notify();
        }
    }

    /// Fills in an entry an index event left incomplete.
    fn fetch(&mut self, fetch: Fetch, cx: &mut Context<Self>) {
        let backend = Backend::global(cx).clone();
        enum Answer {
            Project(Result<Project, CallError>),
            Session(Result<Session, CallError>),
            Inbox(Result<InboxItem, CallError>),
        }
        let answer = match &fetch {
            Fetch::Project(id) => {
                let call = backend.call::<ProjectGet>(
                    ProjectIdParams {
                        project_id: id.clone(),
                    },
                    cx,
                );
                cx.background_spawn(async move { Answer::Project(call.await) })
            }
            Fetch::Session(id) => {
                let call = backend.call::<SessionGet>(
                    SessionIdParams {
                        session_id: id.clone(),
                    },
                    cx,
                );
                cx.background_spawn(async move { Answer::Session(call.await) })
            }
            Fetch::InboxItem(id) => {
                let call = backend.call::<InboxGet>(
                    InboxItemIdParams {
                        item_id: id.clone(),
                    },
                    cx,
                );
                cx.background_spawn(async move { Answer::Inbox(call.await) })
            }
        };
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            let _ = this.update(cx, |this, cx| {
                this.fetching.remove(&fetch);
                let changed = match answer {
                    Answer::Project(Ok(project)) => this.index.upsert_project(project),
                    Answer::Session(Ok(session)) => {
                        this.index.upsert_session(SessionSummary::from(&session))
                    }
                    Answer::Inbox(Ok(item)) => this.index.upsert_inbox_item(item),
                    Answer::Project(Err(error))
                    | Answer::Session(Err(error))
                    | Answer::Inbox(Err(error)) => {
                        warn!(?fetch, %error, "couldn't fetch");
                        false
                    }
                };
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Lists the agents again (`agent/list`).
    pub fn refresh_agents(&mut self, cx: &mut Context<Self>) {
        let call = Backend::global(cx).call::<AgentList>(Empty {}, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(agents) => {
                    this.agents = agents;
                    this.agents_loaded = true;
                    cx.notify();
                }
                Err(error) => warn!(%error, "couldn't list the agents"),
            });
        })
        .detach();
    }

    /// Detects the agents again (`agent/refresh`), for example after logging one in.
    pub fn redetect_agents(&mut self, cx: &mut Context<Self>) -> Task<Result<(), CallError>> {
        let call = Backend::global(cx).call::<AgentRefresh>(Empty {}, cx);
        cx.spawn(async move |this, cx| {
            let agents = call.await?;
            let _ = this.update(cx, |this, cx| {
                this.agents = agents;
                this.agents_loaded = true;
                cx.notify();
            });
            Ok(())
        })
    }

    /// Asks `burrowd` for every provider's usage again (`usage/get`).
    pub fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        let call = Backend::global(cx).call::<UsageGet>(Empty {}, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(usage) => {
                    if this.usage != usage {
                        this.usage = usage;
                        cx.notify();
                    }
                }
                Err(error) => warn!(%error, "couldn't read the usage"),
            });
        })
        .detach();
    }

    /// Installs what `agent` needs (`agent/install`), tracking it in [`Self::install_status`].
    /// On success the agent's entry is replaced with the answer.
    pub fn install_agent(&mut self, agent: AgentId, cx: &mut Context<Self>) {
        if matches!(
            self.installs.get(&agent),
            Some(InstallStatus::Running { .. })
        ) {
            return;
        }
        let call = Backend::global(cx).call::<AgentInstall>(
            AgentInstallParams {
                agent: agent.clone(),
            },
            cx,
        );
        self.installs.insert(
            agent.clone(),
            InstallStatus::Running {
                since: Instant::now(),
            },
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(info) => {
                        this.installs.remove(&agent);
                        match this.agents.iter_mut().find(|a| a.id == info.id) {
                            Some(cached) => *cached = info,
                            None => this.agents.push(info),
                        }
                    }
                    Err(error) => {
                        warn!(%agent, %error, "installing failed");
                        this.installs
                            .insert(agent, InstallStatus::Failed(describe_error(&error)));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- Calls views make ----------------------------------------------------------------------

    /// Starts a session (`session/create`). Answers once the agent is ready.
    pub fn create_session(
        &self,
        params: SessionCreateParams,
        cx: &App,
    ) -> Task<Result<Session, CallError>> {
        Backend::global(cx).call::<SessionCreate>(params, cx)
    }

    /// Sends a text prompt (`session/prompt`).
    pub fn prompt(
        &self,
        session_id: String,
        text: String,
        cx: &App,
    ) -> Task<Result<SessionPromptResult, CallError>> {
        Backend::global(cx).call::<SessionPrompt>(
            SessionPromptParams {
                session_id,
                content: vec![ContentBlock::text(text)],
            },
            cx,
        )
    }

    /// Stops the running turn (`session/cancel`).
    pub fn cancel(&self, session_id: String, cx: &App) -> Task<Result<Empty, CallError>> {
        Backend::global(cx).call::<SessionCancel>(SessionIdParams { session_id }, cx)
    }

    /// Switches the session's model (`session/setModel`); `modelChanged` follows.
    pub fn set_model(
        &self,
        session_id: String,
        model: String,
        cx: &App,
    ) -> Task<Result<Empty, CallError>> {
        Backend::global(cx).call::<SessionSetModel>(SessionSetModelParams { session_id, model }, cx)
    }

    /// Switches the agent's mode (`session/setAgentMode`); `agentModeChanged` follows.
    pub fn set_agent_mode(
        &self,
        session_id: String,
        agent_mode: String,
        cx: &App,
    ) -> Task<Result<Empty, CallError>> {
        Backend::global(cx).call::<SessionSetAgentMode>(
            SessionSetAgentModeParams {
                session_id,
                agent_mode,
            },
            cx,
        )
    }

    /// Renames a session (`session/rename`); `entityRenamed` follows on the index.
    pub fn rename(
        &self,
        session_id: String,
        title: String,
        cx: &App,
    ) -> Task<Result<Empty, CallError>> {
        Backend::global(cx).call::<SessionRename>(SessionRenameParams { session_id, title }, cx)
    }

    /// Ends the agent process (`session/stop`).
    pub fn stop(&self, session_id: String, cx: &App) -> Task<Result<Empty, CallError>> {
        Backend::global(cx).call::<SessionStop>(SessionIdParams { session_id }, cx)
    }

    /// Archives a session (`session/archive`), stopping it first if needed. On success, emits
    /// [`AppStateEvent::Archived`].
    pub fn archive(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<Empty, CallError>> {
        let call = Backend::global(cx).call::<SessionArchive>(
            SessionIdParams {
                session_id: session_id.clone(),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            if result.is_ok() {
                let _ = this.update(cx, |_, cx| cx.emit(AppStateEvent::Archived(session_id)));
            }
            result
        })
    }

    /// Answers an inbox item (`inbox/respond`). The item leaves the list at once; the index
    /// confirms it.
    pub fn respond(
        &mut self,
        item_id: String,
        action: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<InboxItem, CallError>> {
        let call = Backend::global(cx).call::<InboxRespond>(
            InboxRespondParams {
                item_id: item_id.clone(),
                action,
                payload: None,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let conflict = matches!(&result, Err(CallError::Rpc(error)) if error.code == burrow_protocol::code::CONFLICT);
            if result.is_ok() || conflict {
                let _ = this.update(cx, |this, cx| {
                    if this.index.remove_inbox_item(&item_id) {
                        cx.notify();
                    }
                });
            }
            result
        })
    }

    /// Adds a folder as a project (`project/add`).
    pub fn add_project(&self, path: String, cx: &App) -> Task<Result<Project, CallError>> {
        Backend::global(cx).call::<ProjectAdd>(ProjectAddParams { path }, cx)
    }
}

/// A short, human message for a failed call.
pub fn describe_error(error: &CallError) -> String {
    match error {
        CallError::Disconnected => "Not connected to burrowd".to_owned(),
        CallError::Rpc(error) => error.message.clone(),
    }
}

#[cfg(test)]
mod tests;
