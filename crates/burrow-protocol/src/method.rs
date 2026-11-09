//! Typed methods: one marker type per method ties its name to its params and result types, so the
//! server, the clients and the schema share one method table (D-011).
//!
//! ```
//! use burrow_protocol::{Method, Request, methods::EventsSubscribe, SubscribeParams, StreamName};
//!
//! let params = SubscribeParams { stream: StreamName::index(), after: Some(0) };
//! let request = Request::typed::<EventsSubscribe>(1, &params);
//! assert_eq!(request.method, "events/subscribe");
//! assert_eq!(request.params_as::<EventsSubscribe>().unwrap(), params);
//! ```

use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// A request method: its wire name, params and result.
pub trait Method {
    /// The wire name, for example `events/subscribe`.
    const NAME: &'static str;
    /// The `params` object.
    type Params: Serialize + DeserializeOwned + JsonSchema;
    /// The `result` value.
    type Result: Serialize + DeserializeOwned + JsonSchema;
}

/// A notification: its wire name and params. Notifications get no response.
pub trait NotificationMethod {
    /// The wire name, for example `events/event`.
    const NAME: &'static str;
    /// The `params` object.
    type Params: Serialize + DeserializeOwned + JsonSchema;
}

/// Receives every method and notification from [`visit_methods`].
pub trait MethodVisitor {
    /// Called once for each request method.
    fn method<M: Method>(&mut self);
    /// Called once for each notification.
    fn notification<N: NotificationMethod>(&mut self);
}

macro_rules! method_table {
    (
        methods { $( $(#[doc = $mdoc:literal])* $method:ident = $mname:literal ($params:ty) -> $result:ty; )* }
        notifications { $( $(#[doc = $ndoc:literal])* $notification:ident = $nname:literal ($nparams:ty); )* }
    ) => {
        /// Marker types for every request method.
        pub mod methods {
            #[allow(unused_imports)]
            use crate::*;

            $(
                $(#[doc = $mdoc])*
                #[derive(Clone, Copy, Debug)]
                pub enum $method {}

                impl crate::method::Method for $method {
                    const NAME: &'static str = $mname;
                    type Params = $params;
                    type Result = $result;
                }
            )*
        }

        /// Marker types for every notification.
        pub mod notifications {
            #[allow(unused_imports)]
            use crate::*;

            $(
                $(#[doc = $ndoc])*
                #[derive(Clone, Copy, Debug)]
                pub enum $notification {}

                impl crate::method::NotificationMethod for $notification {
                    const NAME: &'static str = $nname;
                    type Params = $nparams;
                }
            )*
        }

        /// Visits every method and notification in the protocol, in a fixed order.
        ///
        /// This is the one method table: the schema export walks it, and a server can use it to
        /// check its dispatch covers every method.
        pub fn visit_methods(visitor: &mut impl MethodVisitor) {
            $( visitor.method::<methods::$method>(); )*
            $( visitor.notification::<notifications::$notification>(); )*
        }
    };
}

method_table! {
    methods {
        /// `initialize`: the first request on every connection ([`InitializeParams`] →
        /// [`InitializeResult`]).
        Initialize = "initialize" (InitializeParams) -> InitializeResult;
        /// `$/ping`: keep-alive. Returns `{}`.
        Ping = "$/ping" (Empty) -> Empty;
        /// `events/subscribe`: replay a stream after a sequence number, then follow it live.
        EventsSubscribe = "events/subscribe" (SubscribeParams) -> SubscribeResult;
        /// `events/unsubscribe`: end a subscription. Returns `{}`.
        EventsUnsubscribe = "events/unsubscribe" (UnsubscribeParams) -> Empty;
        /// `debug/append`: development only. Appends an event to a stream; `burrowd` accepts it
        /// only when started with `--dev`.
        DebugAppend = "debug/append" (DebugAppendParams) -> DebugAppendResult;
        /// `project/list`: every project.
        ProjectList = "project/list" (Empty) -> Vec<Project>;
        /// `project/add`: make a folder a project, detecting its git `origin` remote.
        ProjectAdd = "project/add" (ProjectAddParams) -> Project;
        /// `project/get`: one project.
        ProjectGet = "project/get" (ProjectIdParams) -> Project;
        /// `project/remove`: forget a project. Never deletes files. Returns `{}`.
        ProjectRemove = "project/remove" (ProjectIdParams) -> Empty;
        /// `agent/list`: the agents `burrowd` knows, as last detected.
        AgentList = "agent/list" (Empty) -> Vec<AgentInfo>;
        /// `agent/refresh`: detect the agents again, then list them.
        AgentRefresh = "agent/refresh" (Empty) -> Vec<AgentInfo>;
        /// `agent/install`: install the pinned ACP adapter an agent needs, then detect it again.
        AgentInstall = "agent/install" (AgentInstallParams) -> AgentInfo;
        /// `usage/get`: each provider's usage and limit, as last reported.
        UsageGet = "usage/get" (Empty) -> Vec<ProviderUsage>;
        /// `session/list`: sessions, newest first.
        SessionList = "session/list" (SessionListParams) -> Vec<SessionSummary>;
        /// `session/create`: start an agent session. Answers once the agent is ready.
        SessionCreate = "session/create" (SessionCreateParams) -> Session;
        /// `session/get`: one session.
        SessionGet = "session/get" (SessionIdParams) -> Session;
        /// `session/prompt`: start a turn. Its output arrives as events.
        SessionPrompt = "session/prompt" (SessionPromptParams) -> SessionPromptResult;
        /// `session/cancel`: stop the running turn; the session stays open. Returns `{}`.
        SessionCancel = "session/cancel" (SessionIdParams) -> Empty;
        /// `session/setModel`: switch the session's model. Returns `{}`.
        SessionSetModel = "session/setModel" (SessionSetModelParams) -> Empty;
        /// `session/setAgentMode`: switch the agent's own mode. Returns `{}`.
        SessionSetAgentMode = "session/setAgentMode" (SessionSetAgentModeParams) -> Empty;
        /// `session/stop`: end the agent process and everything it started. Returns `{}`.
        SessionStop = "session/stop" (SessionIdParams) -> Empty;
        /// `session/archive`: stop the session if needed and hide it from default lists.
        SessionArchive = "session/archive" (SessionIdParams) -> Empty;
        /// `inbox/list`: inbox items, newest first.
        InboxList = "inbox/list" (InboxListParams) -> Vec<InboxItem>;
        /// `inbox/get`: one inbox item.
        InboxGet = "inbox/get" (InboxItemIdParams) -> InboxItem;
        /// `inbox/respond`: answer an inbox item.
        InboxRespond = "inbox/respond" (InboxRespondParams) -> InboxItem;
    }
    notifications {
        /// `$/cancel`: cancel one of the sender's own pending requests.
        Cancel = "$/cancel" (CancelParams);
        /// `events/event`: one event from a subscribed stream.
        EventsEvent = "events/event" (StreamEvent);
    }
}
