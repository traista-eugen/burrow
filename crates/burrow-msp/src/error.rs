//! Errors.
//!
//! Three kinds of failure stay apart:
//! - **A call failed** ([`MspError::Rpc`]): Muse answered a request with a JSON-RPC error. Its
//!   [`ErrorKind`] says why.
//! - **A turn failed**: not an error here at all. Muse reports mid-turn failures in
//!   `turn/completed`, never as a JSON-RPC error. [`crate::TurnError::class`] tells a usage
//!   limit and an unavailable provider apart from other failures.
//! - **The connection or process is gone** ([`MspError::Closed`], [`MspError::Exited`]).

use std::io;
use std::process::ExitStatus;

use crate::rpc::RpcError;

open_enum! {
    /// The `kind` of an MSP JSON-RPC error, from the schema's error table (codes in brackets).
    pub enum ErrorKind {
        /// [-32700] Muse couldn't parse a line.
        ParseError = "parseError",
        /// [-32600] The request was malformed.
        InvalidRequest = "invalidRequest",
        /// [-32600] A request came before `initialize`.
        NotInitialized = "notInitialized",
        /// [-32600] A second `initialize`.
        AlreadyInitialized = "alreadyInitialized",
        /// [-32601] Unknown method.
        MethodNotFound = "methodNotFound",
        /// [-32601, -32602] The method or member needs `experimentalApi`.
        ExperimentalRequired = "experimentalRequired",
        /// [-32602] Bad params.
        InvalidParams = "invalidParams",
        /// [-32603] Muse failed internally.
        Internal = "internal",
        /// [-32001] Muse is overloaded; retry later.
        Overloaded = "overloaded",
        /// [-32002] The input is too large.
        InputTooLarge = "inputTooLarge",
        /// [-32010] The method needs a capability this connection wasn't granted.
        CapabilityRequired = "capabilityRequired",
        /// [-32011] Something the request names doesn't exist.
        NotFound = "notFound",
        /// [-32013] Interrupted.
        Interrupted = "interrupted",
        /// [-32014] Cancelled.
        Cancelled = "cancelled",
        /// [-32020] No such session.
        SessionNotFound = "sessionNotFound",
        /// [-32021] Another connection drives the session.
        SessionInUse = "sessionInUse",
        /// [-32024] The session isn't loaded on this host.
        SessionNotLoaded = "sessionNotLoaded",
        /// [-32030] The command was refused; `data.reason` says why, for example `missing_run`
        /// for an interrupt with no running turn.
        CommandRejected = "commandRejected",
        /// [-32031] The command queue is full; retry later.
        Backpressured = "backpressured",
        /// [-32032] No such skill.
        SkillNotFound = "skillNotFound",
        /// [-32050] No such approval.
        ApprovalNotFound = "approvalNotFound",
        /// [-32051] The approval was already decided, perhaps by another client.
        ApprovalAlreadyResolved = "approvalAlreadyResolved",
        /// [-32052] The `choiceId` isn't one of the approval's `availableChoices`.
        ApprovalChoiceInvalid = "approvalChoiceInvalid",
        /// [-32053] The `requirementId` is stale: another stage is current now.
        ApprovalRequirementStale = "approvalRequirementStale",
        /// [-32054] The automated reviewer is unavailable.
        ApprovalReviewerUnavailable = "approvalReviewerUnavailable",
        /// [-32055] No such user-input prompt.
        UserInputNotFound = "userInputNotFound",
        /// [-32056] The prompt was already settled.
        UserInputAlreadySettled = "userInputAlreadySettled",
        /// [-32057] The answers don't fit the prompt.
        UserInputAnswerInvalid = "userInputAnswerInvalid",
    }
}

impl ErrorKind {
    /// The kind a bare code maps to, for an error without `data.kind`.
    pub fn from_code(code: i64) -> Self {
        let kind = match code {
            -32700 => "parseError",
            -32600 => "invalidRequest",
            -32601 => "methodNotFound",
            -32602 => "invalidParams",
            -32603 => "internal",
            -32001 => "overloaded",
            -32002 => "inputTooLarge",
            -32010 => "capabilityRequired",
            -32011 => "notFound",
            -32013 => "interrupted",
            -32014 => "cancelled",
            -32020 => "sessionNotFound",
            -32021 => "sessionInUse",
            -32022 => "sessionAmbiguous",
            -32023 => "forkBoundaryInvalid",
            -32024 => "sessionNotLoaded",
            -32025 => "sessionStreamMismatch",
            -32030 => "commandRejected",
            -32031 => "backpressured",
            -32032 => "skillNotFound",
            -32033 => "computerUseUnavailable",
            -32034 => "marketplaceNotFound",
            -32035 => "pluginNotFound",
            -32036 => "marketplaceAlreadyExists",
            -32037 => "pluginMutationsDisabled",
            -32038 => "uploadNotFound",
            -32040 => "viewTruncated",
            -32041 => "outputUnavailable",
            -32042 => "boundaryPruned",
            -32050 => "approvalNotFound",
            -32051 => "approvalAlreadyResolved",
            -32052 => "approvalChoiceInvalid",
            -32053 => "approvalRequirementStale",
            -32054 => "approvalReviewerUnavailable",
            -32055 => "userInputNotFound",
            -32056 => "userInputAlreadySettled",
            -32057 => "userInputAnswerInvalid",
            other => return Self::Other(format!("code {other}")),
        };
        Self::from(kind)
    }

    /// The error table's `retryable` for this kind.
    pub fn retryable_by_default(&self) -> bool {
        matches!(self, Self::Overloaded | Self::Backpressured)
    }
}

/// Everything that can go wrong talking to `muse serve`.
#[derive(Debug, thiserror::Error)]
pub enum MspError {
    /// Muse answered `method` with a JSON-RPC error.
    #[error("muse refused {method}: {error}")]
    Rpc {
        /// The method that was called.
        method: String,
        /// Muse's error, boxed to keep `MspError` small.
        error: Box<RpcError>,
    },
    /// The params for `method` don't encode as JSON.
    #[error("can't encode the params for {method}: {error}")]
    Encode {
        /// The method being called.
        method: String,
        /// Why encoding failed.
        error: serde_json::Error,
    },
    /// Muse's answer to `method` doesn't match this crate's types.
    #[error("can't decode muse's answer to {method}: {error}")]
    Decode {
        /// The method that was called.
        method: String,
        /// What didn't match.
        error: serde_json::Error,
    },
    /// The connection is closed: Muse closed its stdout, or [`crate::MspClient::shutdown`] ran.
    #[error("the connection to muse is closed")]
    Closed,
    /// The `muse` process exited before the handshake finished. `stderr` holds its last lines,
    /// which usually say why, for example "Local models are not available on this platform".
    #[error("muse exited ({status}) before the handshake: {stderr}")]
    Exited {
        /// How the process ended.
        status: ExitStatus,
        /// The last lines it wrote to stderr.
        stderr: String,
    },
    /// The handshake didn't finish in time.
    #[error("muse didn't answer `initialize` in time")]
    HandshakeTimeout,
    /// Starting or talking to the process failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl MspError {
    /// The JSON-RPC error, if Muse refused a call.
    pub fn rpc(&self) -> Option<&RpcError> {
        match self {
            Self::Rpc { error, .. } => Some(&**error),
            _ => None,
        }
    }

    /// The error kind, if Muse refused a call.
    pub fn kind(&self) -> Option<ErrorKind> {
        self.rpc().map(RpcError::kind)
    }
}

/// The result type of this crate.
pub type Result<T, E = MspError> = std::result::Result<T, E>;
