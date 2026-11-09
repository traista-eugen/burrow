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
//! - [`acp`] and [`msp`]: slots for the ACP and MSP adapters, which M2's phase 2 builds on the
//!   `burrow-acp` and `burrow-msp` clients. Today they detect their agents and refuse to start.
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
    AgentAdapter, AgentError, AgentEvent, AgentSession, PermissionOutcome, PermissionReply,
    PermissionRequest, SessionConfig, StartedSession,
};
pub use registry::Registry;
