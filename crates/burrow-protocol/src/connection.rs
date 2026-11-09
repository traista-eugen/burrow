//! Connection methods: `initialize`, `$/ping` and `$/cancel`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::jsonrpc::RequestId;

/// An empty object, `{}`: the params of `$/ping` and the result of methods that return nothing.
/// Any members are ignored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Empty {}

/// `initialize` params: the client introduces itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// The protocol version the client speaks. Version 0 must match the server's exactly.
    pub protocol_version: u32,
    /// Who the client is.
    pub client: ClientInfo,
    /// What the client can do.
    #[serde(default)]
    pub capabilities: ClientCapabilities,
}

/// The client's name, version and kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    /// For example `burrow-desktop` or `burrowctl`.
    pub name: String,
    /// The client's own version, for example `0.1.0`.
    pub version: String,
    /// What sort of client this is.
    pub kind: ClientKind,
}

/// What sort of client is connecting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ClientKind {
    /// The desktop app, `burrow.exe`.
    Desktop,
    /// `burrowctl` or another command-line tool.
    Cli,
    /// The phone app.
    Mobile,
    /// A custom step process, in RPC mode (the step protocol).
    Step,
}

/// What the client can do. Every capability defaults to off when absent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    /// The client can draw terminals (`terminal/attach`).
    #[serde(default)]
    pub terminal: bool,
    /// Media types the client can display, for example `image/png`.
    #[serde(default)]
    pub media_types: Vec<String>,
}

/// `initialize` result: the server introduces itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// The protocol version the server speaks.
    pub protocol_version: u32,
    /// Who the server is.
    pub server: ServerInfo,
    /// What the server can do.
    #[serde(default)]
    pub capabilities: ServerCapabilities,
    /// This connection's id (`con_…`). Remote clients sign it during authentication.
    pub connection_id: String,
}

/// The server's name and version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// `burrowd`.
    pub name: String,
    /// The server's own version, for example `0.1.0`.
    pub version: String,
}

/// What the server can do. Every capability defaults to off when absent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    /// The flow engine is available.
    #[serde(default)]
    pub flows: bool,
    /// Remote connections are enabled.
    #[serde(default)]
    pub remote: bool,
}

/// `$/cancel` params: which of the sender's own pending requests to cancel. The receiver should
/// stop and answer that request with the error `cancelled`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CancelParams {
    /// The id of the request to cancel.
    pub id: RequestId,
}
