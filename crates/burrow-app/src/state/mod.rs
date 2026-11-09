//! Shared state: what the app knows about `burrowd`, as GPUI entities that views observe.
//!
//! - [`AppState`] (one per app): the connection status, the lists from the `index` stream
//!   ([`index::IndexModel`]), and the agents from `agent/list`. It also owns the connection
//!   loop and the index subscription, and has the calls views make (`prompt`, `respond`, …).
//! - [`session::SessionStore`] (one per open session): the session's events and metadata.
//! - [`transcript::TranscriptModel`]: a session's events grouped into the transcript's rows; the
//!   transcript view keeps one per open session.
//!
//! The plain models ([`index::IndexModel`], [`session::SessionLog`],
//! [`transcript::TranscriptModel`]) hold the logic and have no GPUI in them; the entities wire
//! them to `burrowd` and to views.

pub mod index;
pub mod session;
pub mod transcript;

use std::collections::HashSet;
use std::time::Duration;

use burrow_client::{CallError, Client};
use burrow_protocol::methods::{
    AgentList, InboxGet, InboxList, InboxRespond, ProjectAdd, ProjectGet, ProjectList,
    SessionCancel, SessionCreate, SessionGet, SessionList, SessionPrompt,
};
use burrow_protocol::{
    AgentInfo, ContentBlock, Empty, InboxItem, InboxItemIdParams, InboxItemStatus, InboxListParams,
    InboxRespondParams, Project, ProjectAddParams, ProjectIdParams, Session, SessionCreateParams,
    SessionIdParams, SessionListParams, SessionPromptParams, SessionPromptResult, SessionSummary,
    StreamName,
};
use gpui::{App, AppContext as _, Context, Task};
use tracing::warn;

use self::index::{Applied, Fetch, IndexModel};
use crate::backend::{self, Backend, ConnectionStatus, FollowEvent};

/// What the app knows about `burrowd`. Views observe it and re-render when it notifies.
pub struct AppState {
    connection: ConnectionStatus,
    index: IndexModel,
    agents: Vec<AgentInfo>,
    agents_loaded: bool,
    /// Bumped by every list load, so the answer of a superseded load is ignored.
    load_generation: u64,
    /// Fetches in flight, so one event storm doesn't fetch the same thing twice.
    fetching: HashSet<Fetch>,
    _connection: Task<()>,
    _index: Task<()>,
}

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
            load_generation: 0,
            fetching: HashSet::new(),
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

    /// The agents `burrowd` offers, as last detected.
    pub fn agents(&self) -> &[AgentInfo] {
        &self.agents
    }

    /// Whether `agent/list` has answered on this connection.
    pub fn agents_loaded(&self) -> bool {
        self.agents_loaded
    }

    /// An agent's display name, or its id.
    pub fn agent_name(&self, id: &burrow_protocol::AgentId) -> String {
        self.agents
            .iter()
            .find(|agent| &agent.id == id)
            .map(|agent| agent.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    fn on_status(
        &mut self,
        status: ConnectionStatus,
        client: Option<&Client>,
        cx: &mut Context<Self>,
    ) {
        if client.is_some() {
            self.refresh_agents(cx);
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
        let sessions = backend.call::<SessionList>(
            SessionListParams {
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
