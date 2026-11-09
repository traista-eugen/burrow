//! The lists the app shows (projects, sessions, open inbox items), as a plain model with no GPUI
//! in it, so it can be tested on its own.
//!
//! The lists are loaded with `project/list`, `session/list` and `inbox/list`, then kept current
//! from the `index` stream. Index events are summaries: a new session arrives without its agent,
//! a new inbox item without its actions. [`IndexModel::apply`] says what to fetch to fill them in
//! ([`Fetch`]), and the caller hands the answers back with [`IndexModel::upsert_session`] and
//! friends.
//!
//! # Loading without losing events
//!
//! The caller subscribes to `index` for live events first, then asks for the lists. Events that
//! arrive before the lists are buffered ([`IndexModel::begin_load`]) and applied on top of them
//! ([`IndexModel::finish_load`]). Every index event sets a value rather than changing one, so
//! applying an event the lists already include does no harm, and the model ends up where
//! `burrowd` is.

use std::collections::HashMap;

use burrow_protocol::{
    AgentId, EntityKind, Event, InboxItem, InboxItemKind, InboxItemStatus, InboxSource, Project,
    SessionState, SessionSummary, StreamEvent, Timestamp,
};

/// A project in the sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectEntry {
    /// `prj_…`.
    pub id: String,
    /// The folder's name.
    pub name: String,
    /// The folder. `None` until `project/get` answers for a project that arrived on the index.
    pub path: Option<String>,
}

impl From<Project> for ProjectEntry {
    fn from(project: Project) -> Self {
        Self {
            id: project.id,
            name: project.name,
            path: Some(project.path),
        }
    }
}

/// A session in the sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionEntry {
    /// `ses_…`.
    pub id: String,
    /// The project it belongs to, if any.
    pub project_id: Option<String>,
    /// The agent. `None` until `session/get` answers for a session that arrived on the index.
    pub agent: Option<AgentId>,
    /// The model, if known.
    pub model: Option<String>,
    /// The display name, once it has one.
    pub title: Option<String>,
    /// The current state.
    pub state: SessionState,
    /// The sort key: the last time something happened. The time of the last index event about
    /// the session, or the list's `updatedAt`.
    pub activity: Option<Timestamp>,
}

impl SessionEntry {
    fn from_summary(summary: SessionSummary) -> Self {
        let activity = Some(summary_activity(&summary));
        Self {
            id: summary.id,
            project_id: summary.project_id,
            agent: Some(summary.agent),
            model: summary.model,
            title: summary.title,
            state: summary.state,
            activity,
        }
    }

    /// The title, or a stand-in while the session has none.
    pub fn display_title(&self) -> String {
        match &self.title {
            Some(title) if !title.trim().is_empty() => title.clone(),
            _ => "New session".to_owned(),
        }
    }
}

/// When a listed session last saw activity.
///
/// `burrowd`'s `SessionSummary.lastActivityAt`: when a turn last started or ended. `updatedAt`
/// stands in only for sessions stored before the field existed.
fn summary_activity(summary: &SessionSummary) -> Timestamp {
    summary.last_activity_at.unwrap_or(summary.updated_at)
}

/// An open inbox item.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxEntry {
    /// `inb_…`.
    pub id: String,
    /// What it asks for.
    pub kind: InboxItemKind,
    /// A one-line summary.
    pub title: String,
    /// Where it comes from.
    pub source: InboxSource,
    /// The whole item, with its actions. `None` until `inbox/get` answers for an item that
    /// arrived on the index.
    pub item: Option<InboxItem>,
    /// When it arrived, for ordering: newest first.
    pub created: Timestamp,
}

impl From<InboxItem> for InboxEntry {
    fn from(item: InboxItem) -> Self {
        Self {
            id: item.id.clone(),
            kind: item.kind.clone(),
            title: item.title.clone(),
            source: item.source.clone(),
            created: item.created_at,
            item: Some(item),
        }
    }
}

/// Something an index event left incomplete, for the caller to fetch.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Fetch {
    /// `project/get`, then [`IndexModel::upsert_project`].
    Project(String),
    /// `session/get`, then [`IndexModel::upsert_session`].
    Session(String),
    /// `inbox/get`, then [`IndexModel::upsert_inbox_item`].
    InboxItem(String),
}

/// What applying events did.
#[derive(Debug, Default, PartialEq)]
pub struct Applied {
    /// Whether anything visible changed.
    pub changed: bool,
    /// What to fetch to complete the entries.
    pub fetch: Vec<Fetch>,
}

impl Applied {
    fn merge(&mut self, other: Applied) {
        self.changed |= other.changed;
        self.fetch.extend(other.fetch);
    }
}

/// The lists the app shows, from `*/list` plus the `index` stream.
#[derive(Debug, Default)]
pub struct IndexModel {
    projects: Vec<ProjectEntry>,
    sessions: HashMap<String, SessionEntry>,
    inbox: HashMap<String, InboxEntry>,
    /// The seq of the last index event applied or buffered.
    seq: u64,
    /// Whether the lists have been loaded at least once.
    loaded: bool,
    /// Events that arrived while the lists were loading.
    buffered: Option<Vec<StreamEvent>>,
}

impl IndexModel {
    /// An empty model, before anything is loaded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the lists have been loaded at least once.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Whether lists are being loaded: events are buffered until [`Self::finish_load`].
    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        self.buffered.is_some()
    }

    /// The seq of the last index event seen. After a reconnect, subscribe again after it.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Starts loading the lists: index events are buffered until [`Self::finish_load`].
    pub fn begin_load(&mut self) {
        self.buffered = Some(Vec::new());
    }

    /// Replaces every list with what `*/list` returned, then applies the events that arrived
    /// meanwhile. `inbox` should be the open items.
    pub fn finish_load(
        &mut self,
        projects: Vec<Project>,
        sessions: Vec<SessionSummary>,
        inbox: Vec<InboxItem>,
    ) -> Applied {
        self.projects = projects.into_iter().map(ProjectEntry::from).collect();
        self.sessions = sessions
            .into_iter()
            .filter(|summary| !summary.archived)
            .map(|summary| (summary.id.clone(), SessionEntry::from_summary(summary)))
            .collect();
        self.inbox = inbox
            .into_iter()
            .filter(|item| item.status == InboxItemStatus::Open)
            .map(|item| (item.id.clone(), InboxEntry::from(item)))
            .collect();
        self.loaded = true;
        let mut applied = Applied {
            changed: true,
            fetch: Vec::new(),
        };
        for event in self.buffered.take().unwrap_or_default() {
            applied.merge(self.apply_now(&event));
        }
        applied
    }

    /// Gives up a load that won't finish, for example because the connection closed. Buffered
    /// events are dropped; the next connection loads again.
    pub fn abandon_load(&mut self) {
        self.buffered = None;
    }

    /// Notes the seq a fresh live-only subscription starts after, so a later resume doesn't
    /// replay what the lists already include.
    pub fn set_seq(&mut self, seq: u64) {
        self.seq = seq;
    }

    /// Applies one index event, or buffers it while the lists load.
    pub fn apply(&mut self, event: &StreamEvent) -> Applied {
        self.seq = self.seq.max(event.seq);
        if let Some(buffered) = &mut self.buffered {
            buffered.push(event.clone());
            return Applied::default();
        }
        self.apply_now(event)
    }

    fn apply_now(&mut self, event: &StreamEvent) -> Applied {
        let at = event.at;
        let mut applied = Applied::default();
        match &event.event {
            Event::EntityCreated(created) => match created.entity {
                EntityKind::Project => {
                    if !self.projects.iter().any(|p| p.id == created.id) {
                        self.projects.push(ProjectEntry {
                            id: created.id.clone(),
                            name: created.title.clone().unwrap_or_else(|| created.id.clone()),
                            path: None,
                        });
                        applied.fetch.push(Fetch::Project(created.id.clone()));
                    }
                    applied.changed = true;
                }
                EntityKind::Session => {
                    let entry =
                        self.sessions
                            .entry(created.id.clone())
                            .or_insert_with(|| SessionEntry {
                                id: created.id.clone(),
                                project_id: created.project_id.clone(),
                                agent: None,
                                model: None,
                                title: None,
                                state: SessionState::Starting,
                                activity: None,
                            });
                    if created.title.is_some() {
                        entry.title = created.title.clone();
                    }
                    if let Some(state) = &created.state {
                        entry.state = SessionState::from(state.as_str());
                    }
                    entry.activity = Some(at);
                    if entry.agent.is_none() {
                        applied.fetch.push(Fetch::Session(created.id.clone()));
                    }
                    applied.changed = true;
                }
                _ => {}
            },
            Event::EntityRenamed(renamed) => match renamed.entity {
                EntityKind::Project => {
                    if let Some(project) = self.projects.iter_mut().find(|p| p.id == renamed.id) {
                        project.name = renamed.title.clone();
                        applied.changed = true;
                    }
                }
                EntityKind::Session => {
                    if let Some(session) = self.sessions.get_mut(&renamed.id) {
                        session.title = Some(renamed.title.clone());
                        applied.changed = true;
                    }
                }
                _ => {}
            },
            Event::EntityStateChanged(changed) => {
                if changed.entity == EntityKind::Session
                    && let Some(session) = self.sessions.get_mut(&changed.id)
                {
                    session.state = SessionState::from(changed.state.as_str());
                    // `burrowd` sends the session's activity time; the event's own time stands
                    // in for an older `burrowd`.
                    session.activity = Some(changed.last_activity_at.unwrap_or(at));
                    applied.changed = true;
                }
            }
            Event::EntityRemoved(removed) => match removed.entity {
                EntityKind::Project => {
                    let before = self.projects.len();
                    self.projects.retain(|p| p.id != removed.id);
                    applied.changed = self.projects.len() != before;
                }
                EntityKind::Session => {
                    applied.changed = self.sessions.remove(&removed.id).is_some();
                }
                _ => {}
            },
            Event::InboxItemAdded(added) => {
                if !self.inbox.contains_key(&added.item_id) {
                    self.inbox.insert(
                        added.item_id.clone(),
                        InboxEntry {
                            id: added.item_id.clone(),
                            kind: added.kind.clone(),
                            title: added.title.clone(),
                            source: added.source.clone(),
                            item: None,
                            created: at,
                        },
                    );
                    applied.fetch.push(Fetch::InboxItem(added.item_id.clone()));
                    applied.changed = true;
                }
            }
            Event::InboxItemResolved(resolved) => {
                applied.changed = self.inbox.remove(&resolved.item_id).is_some();
            }
            // Not an index event, or one this version doesn't know: nothing to show.
            _ => {}
        }
        applied
    }

    /// Fills in a project from `project/get`. Ignored if the project was removed meanwhile.
    pub fn upsert_project(&mut self, project: Project) -> bool {
        match self.projects.iter_mut().find(|p| p.id == project.id) {
            Some(entry) => {
                *entry = ProjectEntry::from(project);
                true
            }
            None => false,
        }
    }

    /// Fills in a session from `session/get` (as a summary). The state and activity the index
    /// gave are kept: they can be newer than the answer. Ignored if the session left the list
    /// meanwhile.
    pub fn upsert_session(&mut self, summary: SessionSummary) -> bool {
        let Some(entry) = self.sessions.get_mut(&summary.id) else {
            return false;
        };
        if entry.activity.is_none() {
            entry.activity = Some(summary_activity(&summary));
        }
        entry.agent = Some(summary.agent);
        entry.model = summary.model;
        entry.project_id = summary.project_id;
        if summary.title.is_some() {
            entry.title = summary.title;
        }
        true
    }

    /// Fills in an inbox item from `inbox/get`, or drops it if it is no longer open.
    pub fn upsert_inbox_item(&mut self, item: InboxItem) -> bool {
        if item.status != InboxItemStatus::Open {
            return self.inbox.remove(&item.id).is_some();
        }
        match self.inbox.get_mut(&item.id) {
            Some(entry) => {
                *entry = InboxEntry::from(item);
                true
            }
            None => false,
        }
    }

    /// Forgets an inbox item, for example after answering it here.
    pub fn remove_inbox_item(&mut self, item_id: &str) -> bool {
        self.inbox.remove(item_id).is_some()
    }

    /// Projects, oldest first.
    pub fn projects(&self) -> &[ProjectEntry] {
        &self.projects
    }

    /// One project.
    pub fn project(&self, project_id: &str) -> Option<&ProjectEntry> {
        self.projects.iter().find(|p| p.id == project_id)
    }

    /// One session.
    pub fn session(&self, session_id: &str) -> Option<&SessionEntry> {
        self.sessions.get(session_id)
    }

    /// The sessions of a project, most recent activity first. `None` gives the sessions that
    /// have no project, or whose project isn't listed.
    pub fn sessions_of(&self, project_id: Option<&str>) -> Vec<&SessionEntry> {
        let mut sessions: Vec<&SessionEntry> = self
            .sessions
            .values()
            .filter(|session| match project_id {
                Some(id) => session.project_id.as_deref() == Some(id),
                None => session
                    .project_id
                    .as_deref()
                    .is_none_or(|id| self.project(id).is_none()),
            })
            .collect();
        sort_by_activity(&mut sessions);
        sessions
    }

    /// Every listed session, most recent activity first.
    pub fn sessions(&self) -> Vec<&SessionEntry> {
        let mut sessions: Vec<&SessionEntry> = self.sessions.values().collect();
        sort_by_activity(&mut sessions);
        sessions
    }

    /// Open inbox items, newest first.
    pub fn inbox(&self) -> Vec<&InboxEntry> {
        let mut items: Vec<&InboxEntry> = self.inbox.values().collect();
        items.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
        items
    }

    /// How many inbox items are open.
    pub fn inbox_count(&self) -> usize {
        self.inbox.len()
    }

    /// The open inbox items of one session.
    #[cfg(test)]
    pub fn inbox_of_session(&self, session_id: &str) -> Vec<&InboxEntry> {
        self.inbox()
            .into_iter()
            .filter(|entry| entry.source.session_id.as_deref() == Some(session_id))
            .collect()
    }
}

fn sort_by_activity(sessions: &mut [&SessionEntry]) {
    // Newest first; ids are time-ordered, so they break ties the same way.
    sessions.sort_by(|a, b| b.activity.cmp(&a.activity).then_with(|| b.id.cmp(&a.id)));
}

#[cfg(test)]
mod tests {
    use burrow_protocol::{
        EntityCreated, EntityRemoved, EntityRenamed, EntityStateChanged, InboxAction,
        InboxItemAdded, InboxItemResolved, ProjectSettings, StreamName,
    };

    use super::*;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(1_800_000_000_000 + ms).unwrap()
    }

    fn event(seq: u64, event: impl Into<Event>) -> StreamEvent {
        StreamEvent {
            stream: StreamName::index(),
            seq,
            at: at(seq as i64 * 1000),
            event: event.into(),
        }
    }

    fn project(id: &str, name: &str) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            path: format!("D:\\{name}"),
            remote: None,
            settings: ProjectSettings::default(),
            created_at: at(0),
        }
    }

    fn summary(
        id: &str,
        project: Option<&str>,
        state: SessionState,
        updated: i64,
    ) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            project_id: project.map(Into::into),
            agent: AgentId::Fake,
            model: Some("fake".into()),
            title: Some(format!("title of {id}")),
            state,
            archived: false,
            created_at: at(0),
            updated_at: at(updated),
            last_activity_at: Some(at(updated)),
        }
    }

    fn inbox_item(id: &str, session: &str, status: InboxItemStatus) -> InboxItem {
        InboxItem {
            id: id.into(),
            kind: InboxItemKind::Permission,
            title: "Edit README.md".into(),
            body: None,
            source: InboxSource {
                session_id: Some(session.into()),
                tool_call_id: Some("call_1".into()),
                ..Default::default()
            },
            actions: vec![InboxAction {
                id: "allow".into(),
                label: "Allow".into(),
                style: None,
            }],
            form: None,
            attachments: Vec::new(),
            status,
            resolution: None,
            created_at: at(5),
            expires_at: None,
            closed_at: None,
        }
    }

    fn session_created(id: &str, project: Option<&str>) -> EntityCreated {
        EntityCreated {
            entity: EntityKind::Session,
            id: id.into(),
            title: None,
            project_id: project.map(Into::into),
            state: Some("starting".into()),
        }
    }

    fn state_changed(id: &str, state: &str) -> EntityStateChanged {
        EntityStateChanged {
            entity: EntityKind::Session,
            id: id.into(),
            state: state.into(),
            last_activity_at: None,
        }
    }

    fn loaded() -> IndexModel {
        let mut model = IndexModel::new();
        model.begin_load();
        model.finish_load(
            vec![project("prj_a", "alpha"), project("prj_b", "beta")],
            vec![
                summary("ses_1", Some("prj_a"), SessionState::Idle, 1),
                summary("ses_2", Some("prj_a"), SessionState::Stopped, 2),
                summary("ses_3", None, SessionState::Idle, 3),
            ],
            vec![inbox_item("inb_1", "ses_1", InboxItemStatus::Open)],
        );
        model
    }

    #[test]
    fn lists_load_into_groups_sorted_by_activity() {
        let model = loaded();
        assert!(model.is_loaded());
        let alpha: Vec<_> = model
            .sessions_of(Some("prj_a"))
            .iter()
            .map(|s| s.id.clone())
            .collect();
        assert_eq!(alpha, ["ses_2", "ses_1"]);
        let loose: Vec<_> = model
            .sessions_of(None)
            .iter()
            .map(|s| s.id.clone())
            .collect();
        assert_eq!(loose, ["ses_3"]);
        assert!(model.sessions_of(Some("prj_b")).is_empty());
        assert_eq!(model.inbox_count(), 1);
        assert_eq!(model.inbox_of_session("ses_1").len(), 1);
    }

    #[test]
    fn a_new_session_asks_for_its_details_and_keeps_newer_state() {
        let mut model = loaded();
        let applied = model.apply(&event(10, session_created("ses_9", Some("prj_b"))));
        assert!(applied.changed);
        assert_eq!(applied.fetch, [Fetch::Session("ses_9".into())]);
        assert_eq!(
            model.session("ses_9").unwrap().state,
            SessionState::Starting
        );
        assert_eq!(model.session("ses_9").unwrap().agent, None);

        model.apply(&event(11, state_changed("ses_9", "idle")));
        model.apply(&event(12, state_changed("ses_9", "working")));
        // session/get answered with what it saw before the last change.
        let mut answer = summary("ses_9", Some("prj_b"), SessionState::Idle, 11);
        answer.title = None;
        assert!(model.upsert_session(answer));
        let session = model.session("ses_9").unwrap();
        assert_eq!(session.agent, Some(AgentId::Fake));
        assert_eq!(session.state, SessionState::Working);
        assert_eq!(session.display_title(), "New session");
        // The newest activity sorts first.
        assert_eq!(model.sessions()[0].id, "ses_9");
        assert_eq!(model.seq(), 12);
    }

    #[test]
    fn renames_and_removals() {
        let mut model = loaded();
        model.apply(&event(
            20,
            EntityRenamed {
                entity: EntityKind::Session,
                id: "ses_1".into(),
                title: "Fix the typo".into(),
            },
        ));
        assert_eq!(
            model.session("ses_1").unwrap().display_title(),
            "Fix the typo"
        );

        model.apply(&event(
            21,
            EntityRemoved {
                entity: EntityKind::Session,
                id: "ses_1".into(),
            },
        ));
        assert!(model.session("ses_1").is_none());

        // Removing a project moves its sessions to "no project".
        model.apply(&event(
            22,
            EntityRemoved {
                entity: EntityKind::Project,
                id: "prj_a".into(),
            },
        ));
        assert_eq!(model.projects().len(), 1);
        let loose: Vec<_> = model
            .sessions_of(None)
            .iter()
            .map(|s| s.id.clone())
            .collect();
        assert_eq!(loose, ["ses_3", "ses_2"]);
    }

    #[test]
    fn inbox_items_come_and_go() {
        let mut model = loaded();
        let applied = model.apply(&event(
            30,
            InboxItemAdded {
                item_id: "inb_2".into(),
                kind: InboxItemKind::Permission,
                title: "Run echo hi".into(),
                source: InboxSource {
                    session_id: Some("ses_3".into()),
                    ..Default::default()
                },
            },
        ));
        assert_eq!(applied.fetch, [Fetch::InboxItem("inb_2".into())]);
        assert_eq!(model.inbox()[0].id, "inb_2");
        assert!(model.inbox()[0].item.is_none());
        assert!(model.upsert_inbox_item(inbox_item("inb_2", "ses_3", InboxItemStatus::Open)));
        assert!(model.inbox_of_session("ses_3")[0].item.is_some());

        model.apply(&event(
            31,
            InboxItemResolved {
                item_id: "inb_2".into(),
                status: InboxItemStatus::Resolved,
                action: Some("allow".into()),
                by: Some("local".into()),
            },
        ));
        assert_eq!(model.inbox_count(), 1);
        // An answer that comes back after the item closed doesn't bring it back.
        assert!(!model.upsert_inbox_item(inbox_item("inb_2", "ses_3", InboxItemStatus::Open)));
        assert!(model.upsert_inbox_item(inbox_item("inb_1", "ses_1", InboxItemStatus::Expired)));
        assert_eq!(model.inbox_count(), 0);
    }

    #[test]
    fn events_during_a_load_are_applied_on_top_of_the_lists() {
        let mut model = IndexModel::new();
        model.begin_load();
        // Live events arrive while the lists are on their way; one of them the lists already
        // include (ses_1 created and idle), the newest one they don't (ses_1 working).
        assert_eq!(
            model.apply(&event(5, session_created("ses_1", Some("prj_a")))),
            Applied::default()
        );
        model.apply(&event(6, state_changed("ses_1", "idle")));
        model.apply(&event(7, state_changed("ses_1", "working")));
        assert!(model.sessions().is_empty());

        let applied = model.finish_load(
            vec![project("prj_a", "alpha")],
            vec![summary("ses_1", Some("prj_a"), SessionState::Idle, 6)],
            Vec::new(),
        );
        assert!(applied.changed);
        // ses_1 came complete from the list, so nothing to fetch.
        assert!(applied.fetch.is_empty());
        assert_eq!(model.session("ses_1").unwrap().state, SessionState::Working);
        assert_eq!(model.seq(), 7);
        assert!(!model.is_loading());
    }

    #[test]
    fn a_reload_replaces_everything() {
        let mut model = loaded();
        model.apply(&event(40, session_created("ses_x", None)));
        // After a `reset` subscription, the caller reloads: entries burrowd no longer has go.
        model.begin_load();
        model.finish_load(vec![project("prj_a", "alpha")], Vec::new(), Vec::new());
        assert!(model.sessions().is_empty());
        assert_eq!(model.inbox_count(), 0);
        assert_eq!(model.projects().len(), 1);
    }

    #[test]
    fn unknown_events_and_kinds_are_ignored() {
        let mut model = loaded();
        let unknown: Event = serde_json_event(r#"{"type":"somethingNew","x":1}"#);
        assert!(!model.apply(&event(50, unknown)).changed);
        let run = EntityCreated {
            entity: EntityKind::Run,
            id: "run_1".into(),
            title: None,
            project_id: None,
            state: Some("queued".into()),
        };
        assert!(!model.apply(&event(51, run)).changed);
        assert_eq!(model.seq(), 51);
    }

    fn serde_json_event(json: &str) -> Event {
        // `Event` decodes a type it doesn't know into `Event::Unknown`.
        serde_json::from_str(json).unwrap()
    }
}
