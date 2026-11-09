//! Event streams: `events/subscribe`, `events/unsubscribe`, `events/event`, and the [`Event`]
//! payloads they carry.
//!
//! `burrowd` keeps an append-only log of events for each stream, numbered from 1 with no gaps
//! (D-005). Event `type` names are unique across all streams, so one [`Event`] enum decodes any
//! of them. Version 0 defines the `index` stream's events and, since M2, the session events
//! (their payloads live in [`crate::session`]). Run events arrive with M5, and until then decode
//! as [`Event::Unknown`].

use std::borrow::{Borrow, Cow};
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};

use crate::agent::AgentId;
use crate::inbox::InboxAction;
use crate::open_enum::doc_text;
use crate::session::{
    AgentMessageChunk, AgentModeChanged, AgentThoughtChunk, LimitReached, ModelChanged,
    PermissionRequested, PermissionResolved, Plan, StateChanged, TerminalOpened, ToolCall,
    ToolCallUpdate, TurnEnded, Usage, UserMessage,
};
use crate::time::Timestamp;

/// The name of an event stream: `index`, `session:<sessionId>` or `run:<runId>`.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct StreamName(String);

impl StreamName {
    /// The name of the `index` stream.
    pub const INDEX: &'static str = "index";

    /// A stream name, as given.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The `index` stream: lightweight summaries of every entity.
    pub fn index() -> Self {
        Self::new(Self::INDEX)
    }

    /// `session:<sessionId>`: everything in one agent session.
    pub fn session(session_id: &str) -> Self {
        Self(format!("session:{session_id}"))
    }

    /// `run:<runId>`: everything in one flow run.
    pub fn run(run_id: &str) -> Self {
        Self(format!("run:{run_id}"))
    }

    /// The name as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name as an owned string.
    pub fn into_string(self) -> String {
        self.0
    }

    /// Whether this is the `index` stream.
    pub fn is_index(&self) -> bool {
        self.0 == Self::INDEX
    }
}

impl fmt::Display for StreamName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for StreamName {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for StreamName {
    fn from(name: String) -> Self {
        Self(name)
    }
}

impl AsRef<str> for StreamName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for StreamName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// `events/subscribe` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeParams {
    /// The stream to follow.
    pub stream: StreamName,
    /// Replay events with a `seq` greater than this, then follow live. Leave it out to receive
    /// only new events; `0` replays the full history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
}

/// `events/subscribe` result. Replayed events follow as `events/event` notifications.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeResult {
    /// The stream subscribed to.
    pub stream: StreamName,
    /// The `seq` of the stream's latest event when the subscription started; `0` if it has none.
    pub head: u64,
    /// The requested history isn't available, for example after compaction, or `after` is
    /// beyond `head`. Nothing is replayed: reload state with `*/get`. Live events after `head`
    /// still follow.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reset: bool,
}

/// `events/unsubscribe` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeParams {
    /// The stream to stop following.
    pub stream: StreamName,
}

/// `events/event` params: one event from a subscribed stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    /// The stream the event belongs to.
    pub stream: StreamName,
    /// The event's position in its stream, from 1, with no gaps.
    pub seq: u64,
    /// When the event was appended.
    pub at: Timestamp,
    /// The event itself.
    pub event: Event,
}

open_enum! {
    /// The kind of entity an `index` event is about.
    pub enum EntityKind {
        /// A project (`prj_…`).
        Project = "project",
        /// An agent session (`ses_…`).
        Session = "session",
        /// A flow run (`run_…`).
        Run = "run",
    }
}

open_enum! {
    /// What an inbox item asks for.
    pub enum InboxItemKind {
        /// Approve or reject something, for example a plan.
        Approval = "approval",
        /// Answer a question.
        Question = "question",
        /// Pick one of several options, for example one of 4 images.
        Choice = "choice",
        /// Do something by hand, then confirm.
        ManualTask = "manualTask",
        /// An agent asks for permission to use a tool.
        Permission = "permission",
        /// Review a change.
        Review = "review",
        /// Something needs a human to step in, for example a review loop that doesn't converge.
        Escalation = "escalation",
    }
}

/// Where an inbox item comes from. Every member is optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxSource {
    /// The run that asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The step that asked, within `runId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_path: Option<String>,
    /// The session that asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The tool call a permission request is about, within `sessionId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Whether an inbox item still needs an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum InboxItemStatus {
    /// Waiting for an answer.
    Open,
    /// Answered.
    Resolved,
    /// Nobody answered in time.
    Expired,
}

/// `entityCreated`: a project, session or run now exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EntityCreated {
    /// What kind of entity.
    pub entity: EntityKind,
    /// The entity's id.
    pub id: String,
    /// Its display name: a project's name, or a session's or run's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The project it belongs to, for sessions and runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Its initial state, for entities that have one (a session or run state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// The agent, for sessions, so a list entry needs no `session/get`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
}

/// `entityRenamed`: an entity's display name changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EntityRenamed {
    /// What kind of entity.
    pub entity: EntityKind,
    /// The entity's id.
    pub id: String,
    /// The new display name.
    pub title: String,
}

/// `entityStateChanged`: an entity moved to a new state, or, for a session, its last activity
/// moved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EntityStateChanged {
    /// What kind of entity.
    pub entity: EntityKind,
    /// The entity's id.
    pub id: String,
    /// The state: one of the session states or run states in protocol.md. Kept as a string so
    /// new states don't break older clients. Usually a new state; the same state when only
    /// `lastActivityAt` moved.
    pub state: String,
    /// A session's `lastActivityAt`, so a list sorted by activity can re-sort without
    /// `session/get`. Always present on session events from `burrowd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<Timestamp>,
}

/// `entityRemoved`: the entity left the default lists. A project was removed, or a session or
/// run was archived.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EntityRemoved {
    /// What kind of entity.
    pub entity: EntityKind,
    /// The entity's id.
    pub id: String,
}

/// `inboxItemAdded`: something now waits on a human. Enough for a list entry or a phone
/// notification; `inbox/get` has the rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxItemAdded {
    /// The inbox item's id (`inb_…`).
    pub item_id: String,
    /// What it asks for.
    pub kind: InboxItemKind,
    /// A one-line summary.
    pub title: String,
    /// Where it comes from.
    #[serde(default)]
    pub source: InboxSource,
    /// The possible answers, as in the item, so a client can show its buttons without
    /// `inbox/get`. Absent from older `burrowd`s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<InboxAction>>,
}

/// `inboxItemResolved`: an inbox item no longer needs an answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxItemResolved {
    /// The inbox item's id.
    pub item_id: String,
    /// `resolved` or `expired`; never `open`.
    pub status: InboxItemStatus,
    /// The id of the action chosen. Absent when the item expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Who answered: a device id, or `local`. Absent when the item expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

/// An event whose `type` this version doesn't know, kept exactly as received.
///
/// Clients show it as an "unsupported event" and carry on; it is sent back unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownEvent(Map<String, Value>);

impl UnknownEvent {
    /// Wraps a JSON object. Returns `None` unless it has a string `type`.
    pub fn from_object(object: Map<String, Value>) -> Option<Self> {
        matches!(object.get("type"), Some(Value::String(_))).then_some(Self(object))
    }

    /// The event's `type`.
    pub fn type_name(&self) -> &str {
        self.0
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The whole event object, including `type`.
    pub fn as_object(&self) -> &Map<String, Value> {
        &self.0
    }

    /// The whole event object, including `type`.
    pub fn into_object(self) -> Map<String, Value> {
        self.0
    }
}

macro_rules! event_enum {
    (
        $(#[doc = $doc:literal])*
        pub enum $name:ident {
            $(
                $(#[doc = $vdoc:literal])*
                $variant:ident = $tag:literal,
            )+
        }
    ) => {
        $(#[doc = $doc])*
        #[derive(Clone, Debug, PartialEq)]
        pub enum $name {
            $(
                $(#[doc = $vdoc])*
                $variant($variant),
            )+
            /// An event type this version doesn't know, kept as received.
            Unknown(UnknownEvent),
        }

        impl $name {
            /// Every event `type` this version knows, in declaration order.
            pub const KNOWN_TYPES: &'static [&'static str] = &[$($tag),+];

            /// The event's `type`.
            pub fn type_name(&self) -> &str {
                match self {
                    $(Self::$variant(_) => $tag,)+
                    Self::Unknown(event) => event.type_name(),
                }
            }

            /// Whether this version knows the event's `type`.
            pub fn is_known(&self) -> bool {
                !matches!(self, Self::Unknown(_))
            }
        }

        $(
            impl From<$variant> for $name {
                fn from(event: $variant) -> Self {
                    Self::$variant(event)
                }
            }
        )+

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                #[derive(Serialize)]
                #[serde(tag = "type")]
                enum Tagged<'a> {
                    $(
                        #[serde(rename = $tag)]
                        $variant(&'a $variant),
                    )+
                }

                match self {
                    $(Self::$variant(event) => Tagged::$variant(event).serialize(serializer),)+
                    Self::Unknown(event) => event.0.serialize(serializer),
                }
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                use serde::de::Error;

                let object = Map::<String, Value>::deserialize(deserializer)?;
                let tag = match object.get("type") {
                    Some(Value::String(tag)) => tag.clone(),
                    Some(_) => return Err(D::Error::custom("event type must be a string")),
                    None => return Err(D::Error::missing_field("type")),
                };
                let value = Value::Object(object);
                match tag.as_str() {
                    $(
                        $tag => $variant::deserialize(value)
                            .map(Self::$variant)
                            .map_err(|error| D::Error::custom(format!("{}: {error}", $tag))),
                    )+
                    _ => match value {
                        Value::Object(object) => Ok(Self::Unknown(UnknownEvent(object))),
                        _ => unreachable!("value was built from an object"),
                    },
                }
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn schema_id() -> Cow<'static, str> {
                concat!(module_path!(), "::", stringify!($name)).into()
            }

            fn json_schema(generator: &mut SchemaGenerator) -> Schema {
                let mut one_of = Vec::new();
                $(
                    one_of.push(tagged_subschema::<$variant>(generator, $tag));
                )+
                one_of.push(generator.subschema_for::<UnknownEvent>());
                json_schema!({
                    "description": doc_text(&[$($doc),*]),
                    "type": "object",
                    "properties": { "type": { "type": "string" } },
                    "required": ["type"],
                    "oneOf": one_of,
                })
            }
        }

        impl JsonSchema for UnknownEvent {
            fn schema_name() -> Cow<'static, str> {
                "UnknownEvent".into()
            }

            fn schema_id() -> Cow<'static, str> {
                concat!(module_path!(), "::UnknownEvent").into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "description": "An event type added by a newer protocol revision. Show it as an unsupported event and keep going; never fail on it.",
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "not": { "enum": $name::KNOWN_TYPES } },
                    },
                    "required": ["type"],
                })
            }
        }
    };
}

event_enum! {
    /// One event in a stream, tagged by `type`.
    ///
    /// Event `type` names are unique across all streams. A receiver must accept a `type` it
    /// doesn't know, show it as an unsupported event and keep going. In Rust it decodes as
    /// `Event::Unknown`, which keeps the raw JSON.
    pub enum Event {
        /// `index`: a project, session or run now exists.
        EntityCreated = "entityCreated",
        /// `index`: an entity's display name changed.
        EntityRenamed = "entityRenamed",
        /// `index`: an entity moved to a new state.
        EntityStateChanged = "entityStateChanged",
        /// `index`: an entity left the default lists (removed or archived).
        EntityRemoved = "entityRemoved",
        /// `index`: something now waits on a human.
        InboxItemAdded = "inboxItemAdded",
        /// `index`: an inbox item was answered or expired.
        InboxItemResolved = "inboxItemResolved",
        /// `session:<id>`: the prompt that started a turn.
        UserMessage = "userMessage",
        /// `session:<id>`: a piece of the agent's answer.
        AgentMessageChunk = "agentMessageChunk",
        /// `session:<id>`: a piece of the agent's visible reasoning.
        AgentThoughtChunk = "agentThoughtChunk",
        /// `session:<id>`: the agent started, or asks to start, a tool call.
        ToolCall = "toolCall",
        /// `session:<id>`: a tool call changed.
        ToolCallUpdate = "toolCallUpdate",
        /// `session:<id>`: the agent's current plan.
        Plan = "plan",
        /// `session:<id>`: the agent asks for permission; an inbox item holds the question.
        PermissionRequested = "permissionRequested",
        /// `session:<id>`: a permission request was answered or expired.
        PermissionResolved = "permissionResolved",
        /// `session:<id>`: a turn is over.
        TurnEnded = "turnEnded",
        /// `session:<id>`: tokens and cost of a turn.
        Usage = "usage",
        /// `session:<id>`: a provider limit was hit.
        LimitReached = "limitReached",
        /// `session:<id>`: the session moved to a new state.
        StateChanged = "stateChanged",
        /// `session:<id>`: the session now uses another model.
        ModelChanged = "modelChanged",
        /// `session:<id>`: the agent now runs in another of its modes.
        AgentModeChanged = "agentModeChanged",
        /// `session:<id>`: a terminal belongs to the session (M4).
        TerminalOpened = "terminalOpened",
    }
}

/// The payload's own definition, with its `type` tag added, so each event definition in the
/// schema is self-contained.
fn tagged_subschema<T: JsonSchema>(generator: &mut SchemaGenerator, tag: &str) -> Schema {
    let reference = generator.subschema_for::<T>();
    if let Some(Value::Object(definition)) = generator
        .definitions_mut()
        .get_mut(T::schema_name().as_ref())
    {
        let properties = definition
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(properties) = properties {
            properties.insert("type".to_owned(), json!({ "const": tag }));
        }
        let required = definition
            .entry("required")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(required) = required
            && !required.iter().any(|field| field == "type")
        {
            required.insert(0, Value::from("type"));
        }
    }
    reference
}

fn is_false(value: &bool) -> bool {
    !*value
}
