//! One marker type per MSP method this crate calls, the same pattern as `burrow-protocol`
//! (D-011): a call and its types agree at compile time, and the schema checks walk [`METHODS`].
//!
//! Any other method is reachable through [`crate::MspClient::call_raw`].

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::types::*;

/// An MSP method the client calls.
pub trait Method {
    /// The wire name, such as `turn/start`.
    const NAME: &'static str;
    /// Whether the request carries `params`. `usage/read` takes none.
    const HAS_PARAMS: bool = true;
    /// The params.
    type Params: Serialize;
    /// The result.
    type Result: DeserializeOwned;
}

/// Params for a method that takes none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NoParams;

macro_rules! methods {
    ($(
        $(#[doc = $doc:literal])*
        $marker:ident = $name:literal, $params:ty => $result:ty $(, has_params = $has:literal)?;
    )+) => {
        $(
            $(#[doc = $doc])*
            #[derive(Clone, Copy, Debug)]
            pub struct $marker;

            impl Method for $marker {
                const NAME: &'static str = $name;
                $(const HAS_PARAMS: bool = $has;)?
                type Params = $params;
                type Result = $result;
            }
        )+

        /// Every method marker's wire name, in declaration order.
        pub const METHODS: &[&str] = &[$($name),+];
    };
}

methods! {
    /// Opens the connection. [`crate::MspClient::connect`] calls it.
    Initialize = "initialize", InitializeParams => InitializeResult;
    /// Creates a session, loads it and subscribes this connection.
    SessionStart = "session/start", SessionStartParams => SessionStartResult;
    /// Selects a preconfigured approval mode.
    SessionSetApprovalMode = "session/setApprovalMode",
        SessionSetApprovalModeParams => SessionSetApprovalModeResult;
    /// Switches the session's model.
    SessionSetModel = "session/setModel", SessionSetModelParams => SessionSetModelResult;
    /// Lists the models `session/setModel` accepts.
    ModelList = "model/list", ModelListParams => ModelListResult;
    /// Submits input.
    TurnStart = "turn/start", TurnStartParams => TurnStartResult;
    /// Stops the running turn on the priority lane.
    TurnInterrupt = "turn/interrupt", TurnInterruptParams => TurnInterruptResult;
    /// Cancels a turn on the normal lane.
    TurnCancel = "turn/cancel", TurnCancelParams => TurnCancelResult;
    /// Decides an approval.
    ApprovalDecide = "approval/decide", ApprovalDecideParams => ApprovalDecideResult;
    /// Reads every pending approval and user-input prompt of a session.
    ApprovalListPending = "approval/listPending",
        ApprovalListPendingParams => ApprovalListPendingResult;
    /// Answers a user-input prompt.
    UserInputAnswer = "userInput/answer", UserInputAnswerParams => UserInputAnswerResult;
    /// Declines a user-input prompt.
    UserInputCancel = "userInput/cancel", UserInputCancelParams => UserInputCancelResult;
    /// Reads a page of stored output, such as an edit's patch.
    ItemReadOutput = "item/readOutput", ItemReadOutputParams => ItemReadOutputResult;
    /// Reads the last observed subscription usage.
    UsageRead = "usage/read", NoParams => UsageReadResult, has_params = false;
}

/// Wire names of the notifications and server requests this crate decodes.
pub mod names {
    /// Client to server: closes the handshake.
    pub const INITIALIZED: &str = "initialized";
    /// Server request: an approval to present.
    pub const APPROVAL_REQUEST: &str = "approval/request";
    /// Server request: a user-input prompt to present.
    pub const USER_INPUT_REQUEST: &str = "userInput/request";

    pub const SESSION_STARTED: &str = "session/started";
    pub const SESSION_STATUS_CHANGED: &str = "session/statusChanged";
    pub const SESSION_CLOSED: &str = "session/closed";
    pub const SESSION_MODEL_CHANGED: &str = "session/modelChanged";
    pub const SESSION_TOKEN_USAGE: &str = "session/tokenUsage";
    pub const SESSION_CONTEXT_USAGE: &str = "session/contextUsage";
    pub const TURN_STARTED: &str = "turn/started";
    pub const TURN_FOREGROUND_COMPLETED: &str = "turn/foregroundCompleted";
    pub const TURN_COMPLETED: &str = "turn/completed";
    pub const ITEM_STARTED: &str = "item/started";
    pub const ITEM_UPDATED: &str = "item/updated";
    pub const ITEM_COMPLETED: &str = "item/completed";
    pub const ITEM_DELTA: &str = "item/delta";
    pub const APPROVAL_REQUESTED: &str = "approval/requested";
    pub const APPROVAL_UPDATED: &str = "approval/updated";
    pub const APPROVAL_RESOLVED: &str = "approval/resolved";
    pub const USER_INPUT_REQUESTED: &str = "userInput/requested";
    pub const USER_INPUT_SETTLED: &str = "userInput/settled";
    pub const USAGE_CHANGED: &str = "usage/changed";
    pub const VIEW_GAP: &str = "view/gap";
}
