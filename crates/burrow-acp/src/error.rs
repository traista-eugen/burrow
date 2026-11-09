use serde_json::Value;

use crate::limit::LimitInfo;
use crate::types::AuthMethod;

/// What can go wrong talking to an agent.
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    /// The agent wants the client to authenticate first (ACP error `-32000`).
    /// With the user logged in to the agent's own CLI this shouldn't happen;
    /// when it does, the user should log in again (for example `claude
    /// login`, `codex login` or `agent login`), or the caller can try
    /// [`crate::AcpClient::authenticate`] with one of `methods`.
    #[error("the agent needs you to log in: {message}")]
    AuthRequired {
        /// The agent's message.
        message: String,
        /// The methods the agent offered in `initialize`.
        methods: Vec<AuthMethod>,
    },
    /// The agent hit a usage or rate limit.
    #[error("{0}")]
    LimitReached(LimitInfo),
    /// The agent answered a request with an error.
    #[error("the agent returned error {code}: {message}")]
    Agent {
        /// The JSON-RPC error code.
        code: i32,
        /// The error message.
        message: String,
        /// The error's `data`, if any.
        data: Option<Value>,
    },
    /// The agent doesn't support what was asked.
    #[error("the agent doesn't support {0}")]
    Unsupported(String),
    /// The agent's answer didn't have the expected shape.
    #[error("unexpected answer to {method}: {message}")]
    Protocol {
        /// The method that was called.
        method: String,
        /// What was wrong.
        message: String,
    },
    /// The connection to the agent is closed, for example because its
    /// process exited.
    #[error("the connection to the agent is closed")]
    Closed,
    /// Starting the agent's process failed.
    #[error("couldn't start the agent: {0}")]
    Spawn(#[source] std::io::Error),
}

impl AcpError {
    /// The JSON-RPC error code, for errors the agent sent.
    pub fn code(&self) -> Option<i32> {
        match self {
            Self::AuthRequired { .. } => Some(-32000),
            Self::Agent { code, .. } => Some(*code),
            Self::LimitReached(info) => info.code,
            _ => None,
        }
    }
}

/// JSON-RPC's "method not found".
pub(crate) const METHOD_NOT_FOUND: i32 = -32601;
/// ACP's "authentication required".
pub(crate) const AUTH_REQUIRED: i32 = -32000;
