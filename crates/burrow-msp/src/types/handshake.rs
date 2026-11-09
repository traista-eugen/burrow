//! `initialize`: the first request on a connection, followed by the `initialized` notification.

use serde::{Deserialize, Serialize};

/// Who the client is. Muse records it on sessions the client starts (`createdByClient`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    /// Machine identifier: lowercase letters, digits and `_` only.
    pub name: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Client version.
    pub version: String,
}

/// What the client asks for. Every member has a default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    /// Opt into the experimental surface. This crate only speaks the stable one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental_api: Option<bool>,
    /// Notification methods to stop receiving. Protected ones can't be opted out of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opt_out_notification_methods: Vec<String>,
    /// Capabilities to request, such as `userShell`. Unknown names are ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requested_capabilities: Vec<String>,
    /// Whether the client can show `userInput/request` prompts. Absent means yes; `false` makes
    /// Muse withhold them from this connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_input_dialogs: Option<bool>,
}

/// `initialize` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// What the client asks for; absent means all defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ClientCapabilities>,
    /// Who the client is.
    pub client_info: ClientInfo,
}

/// The server's name and version: `muse` and, for example, `1.4.14`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// Server name.
    pub name: String,
    /// Server version.
    pub version: String,
}

/// The wire envelope version and the stable surface's fingerprint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    /// `sha256:<hex>` of the binary's stable-surface schema. A mismatch with the schema this
    /// crate was written against is a warning, not an error.
    pub fingerprint: String,
    /// The envelope schema version: `1`.
    pub version: u32,
}

open_enum! {
    /// Whether Muse can enforce its sandbox on this machine.
    pub enum SandboxAvailability {
        /// It can.
        Available = "available",
        /// It can't. On Windows the shell tool then fails until `muse sandbox setup` has run.
        Unavailable = "unavailable",
    }
}

open_enum! {
    /// Whether the host keeps its sessions on disk.
    pub enum SessionDurability {
        /// Sessions are logged and can be resumed.
        Durable = "durable",
        /// Memory-only sessions (`muse serve --no-session-log`).
        Ephemeral = "ephemeral",
    }
}

/// The `initialize` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Whether the experimental surface is on.
    pub experimental_api: bool,
    /// The capabilities granted, fixed for the connection's lifetime.
    pub granted_capabilities: Vec<String>,
    /// Muse's home directory.
    pub muse_home: String,
    /// `windows` or `unix`.
    pub platform_family: String,
    /// `windows`, `macos` or `linux`.
    pub platform_os: String,
    /// Whether the sandbox can be enforced. Absent means unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_availability: Option<SandboxAvailability>,
    /// The envelope version and fingerprint.
    pub schema: SchemaInfo,
    /// Server name and version.
    pub server_info: ServerInfo,
    /// Whether sessions are kept. Absent means durable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_durability: Option<SessionDurability>,
    /// The user agent Muse presents to model providers.
    pub user_agent: String,
}
