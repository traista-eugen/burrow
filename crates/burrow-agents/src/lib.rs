//! Agent adapters for `burrowd`: the interface every agent integration implements, the process
//! launcher that runs agents inside Job Objects, detection of the installed agents, and a
//! scripted fake agent (D-018).
//!
//! # Layout
//!
//! - [`adapter`]: [`AgentAdapter`] (detect, start) and [`AgentSession`] (prompt, cancel,
//!   set the model, stop), plus [`AgentEvent`], the normalized events a session emits.
//! - [`launcher`]: starts agent processes in Job Objects, with no console window, finds
//!   Windows shims on `PATH`, and hands the child's stdin and stdout to a protocol client.
//! - [`detect`]: version, login state and models of Claude Code, Codex, Cursor and Muse.
//! - [`fake`]: the scripted fake agent, a real child process, for tests and `burrowd --dev`.
//! - [`acp`]: the ACP adapter for Claude Code, Codex and Cursor, built on `burrow-acp`: launching,
//!   installing the pinned npm adapters, modes, the event mapping, the stuck-turn watchdog, and
//!   a scripted fake ACP agent (`burrow-fake-acp-agent`).
//! - [`msp`]: the MSP adapter, which runs Muse Code sessions on the `burrow-msp` client (D-026).
//! - [`registry`]: the set of adapters `burrowd` offers.
//!
//! The crate knows nothing about streams, inbox items or `burrowd`'s state: the session manager
//! in `burrowd` turns [`AgentEvent`]s into stored events.

pub mod acp;
pub mod adapter;
pub mod detect;
pub mod fake;
pub mod launcher;
pub mod msp;
pub mod registry;

pub use adapter::{
    AgentAdapter, AgentError, AgentEvent, AgentModes, AgentSession, PermissionOutcome,
    PermissionReply, PermissionRequest, SessionConfig, StartedSession,
};
pub use registry::{Registry, RegistryOptions};
