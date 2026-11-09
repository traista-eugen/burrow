//! Wire protocol shared by `burrowd` and every client: the desktop app, `burrowctl`, the phone,
//! and custom step processes.
//!
//! These types are the source of truth for the protocol (D-004). `docs/protocol.md` explains the
//! intent; the JSON Schema built by [`schema()`] and committed as
//! `schema/burrow-protocol.schema.json` is the exact contract for other languages.
//!
//! # Layout
//!
//! - [`jsonrpc`]: the JSON-RPC 2.0 envelope ([`Request`], [`Response`], [`Notification`]) and
//!   [`Message::parse`], which classifies one incoming line.
//! - [`error`]: [`ErrorObject`], Burrow's [`ErrorKind`]s and the JSON-RPC error codes.
//! - [`method`]: the [`Method`] and [`NotificationMethod`] traits that tie a method name to its
//!   params and result types, and [`visit_methods`], the one table of every method.
//! - [`connection`]: `initialize`, `$/ping` and `$/cancel`.
//! - [`events`]: streams, `events/subscribe`, `events/unsubscribe`, `events/event` and the
//!   [`Event`] payloads.
//! - [`project`]: `project/*` and [`Project`].
//! - [`agent`]: `agent/list`, `agent/refresh`, `usage/get`, [`AgentInfo`] and [`AgentId`].
//! - [`session`]: `session/*`, [`Session`], [`ContentBlock`] and the session events.
//! - [`inbox`]: `inbox/*` and [`InboxItem`].
//! - [`artifact`]: references to files in the artifact store, [`ArtifactRef`].
//! - [`debug`]: `debug/append`, a development-only method.
//! - [`time`]: [`Timestamp`], which always serializes as `2026-11-06T14:03:22.418Z`.
//!
//! # Example
//!
//! ```
//! use burrow_protocol::{Message, Method, methods::Ping, Response, Empty};
//!
//! let line = r#"{"jsonrpc":"2.0","id":7,"method":"$/ping"}"#;
//! let Message::Request(request) = Message::parse(line).unwrap() else { panic!() };
//! assert_eq!(request.method, Ping::NAME);
//! let _params: Empty = request.params_as::<Ping>().unwrap();
//!
//! let reply = Response::ok::<Ping>(request.id, &Empty {});
//! assert_eq!(reply.to_line(), r#"{"jsonrpc":"2.0","id":7,"result":{}}"#);
//! ```

#[macro_use]
mod open_enum;

pub mod agent;
pub mod artifact;
pub mod connection;
pub mod debug;
pub mod error;
pub mod events;
pub mod inbox;
pub mod jsonrpc;
pub mod method;
pub mod project;
pub mod schema;
pub mod session;
pub mod time;

pub use agent::{
    AgentId, AgentInfo, AgentLogin, Integration, LoginState, ModelInfo, ProviderUsage, UsageSource,
};
pub use artifact::ArtifactRef;
pub use connection::{
    CancelParams, ClientCapabilities, ClientInfo, ClientKind, Empty, InitializeParams,
    InitializeResult, ServerCapabilities, ServerInfo,
};
pub use debug::{DebugAppendParams, DebugAppendResult};
pub use error::{ErrorData, ErrorKind, ErrorObject, code};
pub use events::{
    EntityCreated, EntityKind, EntityRemoved, EntityRenamed, EntityStateChanged, Event,
    InboxItemAdded, InboxItemKind, InboxItemResolved, InboxItemStatus, InboxSource, StreamEvent,
    StreamName, SubscribeParams, SubscribeResult, UnknownEvent, UnsubscribeParams,
};
pub use inbox::{
    InboxAction, InboxActionStyle, InboxItem, InboxItemIdParams, InboxListParams, InboxResolution,
    InboxRespondParams,
};
pub use jsonrpc::{
    JsonRpcVersion, Message, MessageError, Notification, Request, RequestId, Response,
    ResponseOutcome,
};
pub use method::{
    Method, MethodVisitor, NotificationMethod, methods, notifications, visit_methods,
};
pub use project::{
    CodeHost, Project, ProjectAddParams, ProjectIdParams, ProjectRemote, ProjectSettings,
};
pub use schema::{schema, schema_json};
pub use session::{
    AgentMessageChunk, AgentThoughtChunk, ContentBlock, LimitInfo, LimitReached, ModelChanged,
    PermissionOption, PermissionOptionKind, PermissionRequested, PermissionResolved, Plan,
    PlanEntry, PlanEntryPriority, PlanEntryStatus, Session, SessionCreateParams, SessionIdParams,
    SessionListParams, SessionMode, SessionPromptParams, SessionPromptResult,
    SessionSetModelParams, SessionState, SessionSummary, SessionUsage, StateChanged,
    TerminalOpened, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate,
    ToolKind, TurnEndReason, TurnEnded, Usage, UserMessage,
};
pub use time::Timestamp;

/// The protocol version this crate speaks.
///
/// Version 0 is unstable: both sides of a connection must use exactly the same version. Within a
/// version, changes are additive only (see `docs/protocol.md`).
pub const PROTOCOL_VERSION: u32 = 0;
