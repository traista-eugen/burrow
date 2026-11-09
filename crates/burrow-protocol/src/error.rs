//! JSON-RPC error objects, Burrow's error kinds and the error codes.
//!
//! Burrow's own errors carry a [`ErrorKind`] in `error.data.kind`. Clients branch on the kind,
//! never on the message.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::time::Timestamp;

/// JSON-RPC error codes: the standard ones, then Burrow's (`-32001` to `-32008`).
pub mod code {
    /// The line isn't valid JSON.
    pub const PARSE_ERROR: i64 = -32700;
    /// The JSON isn't a valid JSON-RPC message.
    pub const INVALID_REQUEST: i64 = -32600;
    /// The method doesn't exist, or isn't available on this connection.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// The params don't match the method.
    pub const INVALID_PARAMS: i64 = -32602;
    /// Something went wrong inside the receiver.
    pub const INTERNAL_ERROR: i64 = -32603;

    /// [`ErrorKind::NotFound`](crate::ErrorKind::NotFound).
    pub const NOT_FOUND: i64 = -32001;
    /// [`ErrorKind::InvalidState`](crate::ErrorKind::InvalidState).
    pub const INVALID_STATE: i64 = -32002;
    /// [`ErrorKind::Conflict`](crate::ErrorKind::Conflict).
    pub const CONFLICT: i64 = -32003;
    /// [`ErrorKind::Unauthorized`](crate::ErrorKind::Unauthorized).
    pub const UNAUTHORIZED: i64 = -32004;
    /// [`ErrorKind::LimitReached`](crate::ErrorKind::LimitReached).
    pub const LIMIT_REACHED: i64 = -32005;
    /// [`ErrorKind::VersionMismatch`](crate::ErrorKind::VersionMismatch).
    pub const VERSION_MISMATCH: i64 = -32006;
    /// [`ErrorKind::Cancelled`](crate::ErrorKind::Cancelled).
    pub const CANCELLED: i64 = -32007;
    /// [`ErrorKind::Unavailable`](crate::ErrorKind::Unavailable).
    pub const UNAVAILABLE: i64 = -32008;
}

open_enum! {
    /// Why a Burrow request failed. Sent in `error.data.kind`; clients branch on this.
    pub enum ErrorKind {
        /// The entity doesn't exist, or this device can't see it.
        NotFound = "notFound",
        /// Not allowed in the entity's current state, for example prompting a stopped session.
        InvalidState = "invalidState",
        /// Someone else got there first, for example the inbox item was already answered.
        Conflict = "conflict",
        /// Not authenticated, or the device lacks the permission.
        Unauthorized = "unauthorized",
        /// A provider limit was hit. `data.resetsAt` gives the reset time when known.
        LimitReached = "limitReached",
        /// Incompatible protocol versions. `data.supported` lists the supported ones.
        VersionMismatch = "versionMismatch",
        /// The request was cancelled.
        Cancelled = "cancelled",
        /// Something needed is missing, for example the agent isn't installed.
        Unavailable = "unavailable",
    }
}

impl ErrorKind {
    /// The JSON-RPC error code for this kind. A kind this version doesn't know has no code of its
    /// own; it is sent with [`code::INTERNAL_ERROR`].
    pub fn code(&self) -> i64 {
        match self {
            Self::NotFound => code::NOT_FOUND,
            Self::InvalidState => code::INVALID_STATE,
            Self::Conflict => code::CONFLICT,
            Self::Unauthorized => code::UNAUTHORIZED,
            Self::LimitReached => code::LIMIT_REACHED,
            Self::VersionMismatch => code::VERSION_MISMATCH,
            Self::Cancelled => code::CANCELLED,
            Self::Unavailable => code::UNAVAILABLE,
            Self::Other(_) => code::INTERNAL_ERROR,
        }
    }

    /// The kind that owns a Burrow error code (`-32001` to `-32008`).
    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            code::NOT_FOUND => Self::NotFound,
            code::INVALID_STATE => Self::InvalidState,
            code::CONFLICT => Self::Conflict,
            code::UNAUTHORIZED => Self::Unauthorized,
            code::LIMIT_REACHED => Self::LimitReached,
            code::VERSION_MISMATCH => Self::VersionMismatch,
            code::CANCELLED => Self::Cancelled,
            code::UNAVAILABLE => Self::Unavailable,
            _ => return None,
        })
    }
}

/// The `error` member of a failed response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorObject {
    /// A JSON-RPC error code: a standard one, or one of Burrow's, `-32001` to `-32008`.
    pub code: i64,
    /// A human-readable description. Clients show it but never branch on it.
    pub message: String,
    /// Burrow's structured details. Always present on Burrow's own errors; may be absent on
    /// standard JSON-RPC errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<ErrorData>,
}

/// `error.data`: the error kind plus details that only some kinds carry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorData {
    /// What went wrong. Absent on standard JSON-RPC errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ErrorKind>,
    /// `versionMismatch`: the protocol versions the server supports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported: Option<Vec<u32>>,
    /// `limitReached`: when the limit resets, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
}

impl ErrorObject {
    /// A Burrow error of the given kind: the kind's code, plus `data.kind`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            code: kind.code(),
            message: message.into(),
            data: Some(ErrorData {
                kind: Some(kind),
                ..ErrorData::default()
            }),
        }
    }

    /// A standard JSON-RPC error, without `data`.
    pub fn standard(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// `-32700`: the line isn't valid JSON.
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::standard(code::PARSE_ERROR, message)
    }

    /// `-32600`: the JSON isn't a valid JSON-RPC message.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::standard(code::INVALID_REQUEST, message)
    }

    /// `-32601`: no such method.
    pub fn method_not_found(method: &str) -> Self {
        Self::standard(
            code::METHOD_NOT_FOUND,
            format!("method not found: {method}"),
        )
    }

    /// `-32602`: the params don't match the method.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::standard(code::INVALID_PARAMS, message)
    }

    /// `-32603`: something went wrong inside the receiver.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::standard(code::INTERNAL_ERROR, message)
    }

    /// `versionMismatch`, listing the versions the server supports.
    pub fn version_mismatch(requested: u32, supported: Vec<u32>) -> Self {
        let mut error = Self::new(
            ErrorKind::VersionMismatch,
            format!("protocol version {requested} is not supported; supported: {supported:?}"),
        );
        if let Some(data) = &mut error.data {
            data.supported = Some(supported);
        }
        error
    }

    /// `limitReached`, with the reset time when known.
    pub fn limit_reached(message: impl Into<String>, resets_at: Option<Timestamp>) -> Self {
        let mut error = Self::new(ErrorKind::LimitReached, message);
        if let Some(data) = &mut error.data {
            data.resets_at = resets_at;
        }
        error
    }

    /// The error's kind: `data.kind`, or the kind that owns `code` when `data.kind` is absent.
    /// `None` for standard JSON-RPC errors.
    pub fn kind(&self) -> Option<ErrorKind> {
        self.data
            .as_ref()
            .and_then(|data| data.kind.clone())
            .or_else(|| ErrorKind::from_code(self.code))
    }
}

impl fmt::Display for ErrorObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind() {
            Some(kind) => write!(f, "{kind} ({}): {}", self.code, self.message),
            None => write!(f, "error {}: {}", self.code, self.message),
        }
    }
}

impl std::error::Error for ErrorObject {}
