//! Agents and usage: `agent/list`, `agent/refresh`, `agent/install` and `usage/get`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::time::Timestamp;

open_enum! {
    /// Which agent: the four Burrow drives, plus a scripted fake for tests.
    pub enum AgentId {
        /// Claude Code, through ACP.
        Claude = "claude",
        /// Codex, through ACP.
        Codex = "codex",
        /// The Cursor CLI (`agent`), through ACP.
        Cursor = "cursor",
        /// Muse Code, through MSP.
        Muse = "muse",
        /// A scripted fake agent for tests and trying things by hand. Listed only when `burrowd`
        /// runs with `--dev`.
        Fake = "fake",
    }
}

open_enum! {
    /// How Burrow talks to an agent.
    pub enum Integration {
        /// The Agent Client Protocol.
        Acp = "acp",
        /// Muse's own protocol, through `muse serve`.
        Msp = "msp",
        /// The agent's own TUI in a pseudo-terminal.
        Terminal = "terminal",
        /// The fake agent's own trivial protocol.
        Fake = "fake",
    }
}

open_enum! {
    /// Whether the agent's CLI is logged in.
    pub enum LoginState {
        /// Logged in; sessions can use the account.
        LoggedIn = "loggedIn",
        /// Not logged in. Log in with the agent's own CLI.
        LoggedOut = "loggedOut",
        /// Burrow couldn't tell.
        Unknown = "unknown",
    }
}

/// What `burrowd` found out about one agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    /// Which agent.
    pub id: AgentId,
    /// Its display name, for example `Claude Code`.
    pub name: String,
    /// How Burrow talks to it.
    pub integration: Integration,
    /// Whether its CLI was found.
    pub installed: bool,
    /// The CLI's version, as it reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The CLI's absolute path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether the CLI is logged in.
    pub login: AgentLogin,
    /// Models the agent offers, when they are cheap to find out. Empty when unknown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelInfo>,
    /// Whether `session/create` can start this agent now.
    pub available: bool,
    /// Why it isn't available, or what looks wrong. Human-readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// The ACP adapter Burrow installs and runs for this agent, when it needs one (Claude Code
    /// and Codex). `agent/install` installs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<AdapterInfo>,
    /// The agent mode Burrow starts sessions in when `session/create` doesn't name one, for
    /// example Claude Code's `default`. Absent when Burrow keeps the agent's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_agent_mode: Option<String>,
    /// When this was detected.
    pub detected_at: Timestamp,
}

/// An npm package that connects an agent to ACP, installed by Burrow at a pinned version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdapterInfo {
    /// The npm package, for example `@agentclientprotocol/claude-agent-acp`.
    pub package: String,
    /// The pinned version Burrow runs.
    pub version: String,
    /// Whether that version is installed.
    pub installed: bool,
    /// The folder it is, or will be, installed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `agent/install` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentInstallParams {
    /// The agent whose adapter to install.
    pub agent: AgentId,
}

/// An agent CLI's login state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentLogin {
    /// Logged in, logged out, or unknown.
    pub state: LoginState,
    /// The account, when the CLI reports it, for example an email address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// More detail, as the CLI reports it, for example the plan or "no subscription allowance".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A model an agent offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// The id to pass as `model`, for example `gpt-5.3-codex`.
    pub id: String,
    /// A display name, for example `Codex 5.3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Where a usage figure comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum UsageSource {
    /// The agent or provider reported it.
    Reported,
    /// Burrow estimated it.
    Estimated,
}

/// One provider's usage and limit, as last reported. Every figure is optional: providers report
/// different things.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    /// The provider, for example `claude`. By default the id of the agent that reported it.
    pub provider: String,
    /// The agent that reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
    /// How much of the allowance is used, in `unit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    /// The allowance, in `unit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    /// The unit of `used` and `limit`, for example `tokens`, `requests`, `usd` or `percent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Whether the provider's limit has been reached.
    #[serde(default, skip_serializing_if = "is_false")]
    pub limited: bool,
    /// When the limit resets, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
    /// The provider's own words, for example the message that came with a limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Reported or estimated.
    pub source: UsageSource,
    /// When this was last updated.
    pub updated_at: Timestamp,
}

fn is_false(value: &bool) -> bool {
    !*value
}
