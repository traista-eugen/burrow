use std::io;

use burrow_protocol::ErrorObject;

/// Why [`Client::connect`](crate::Client::connect) failed.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// Called outside a tokio runtime without [`ConnectOptions::runtime`](crate::ConnectOptions::runtime).
    #[error("no tokio runtime: connect from inside one, or pass one with ConnectOptions::runtime")]
    NoRuntime,
    /// Nothing serves the pipe: `burrowd` isn't running.
    #[error("burrowd isn't running: nothing serves {pipe}")]
    NotRunning {
        /// The pipe's full path.
        pipe: String,
    },
    /// Every instance of the pipe stayed busy for the whole busy timeout.
    #[error("burrowd is busy: every instance of {pipe} stayed in use")]
    Busy {
        /// The pipe's full path.
        pipe: String,
    },
    /// The pipe's server doesn't run as the current user, so it isn't `burrowd`: another account
    /// may have created a pipe with its name. Nothing was sent.
    #[error("refusing to talk to {pipe}: {reason}")]
    NotYours {
        /// The pipe's full path.
        pipe: String,
        /// What the check found.
        reason: String,
    },
    /// Opening the pipe failed for another reason.
    #[error("can't connect to burrowd: {0}")]
    Io(#[from] io::Error),
    /// `burrowd` answered `initialize` with an error, for example `versionMismatch`.
    #[error("burrowd refused the connection: {0}")]
    Rejected(ErrorObject),
    /// The connection closed before the handshake finished.
    #[error("the connection closed during the handshake")]
    Disconnected,
}

/// Why a call failed.
#[derive(Clone, Debug, thiserror::Error)]
pub enum CallError {
    /// `burrowd` answered with an error. A result that doesn't match the method's type is
    /// reported the same way, as an internal error (`-32603`).
    #[error("{0}")]
    Rpc(ErrorObject),
    /// The connection is closed, or closed before the answer arrived.
    #[error("disconnected from burrowd")]
    Disconnected,
}

impl CallError {
    /// The error `burrowd` answered with, if it answered.
    pub fn rpc(&self) -> Option<&ErrorObject> {
        match self {
            Self::Rpc(error) => Some(error),
            Self::Disconnected => None,
        }
    }
}
