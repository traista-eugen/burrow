//! A thin ACP (Agent Client Protocol) client.
//!
//! It drives one agent process over its stdio: Claude Code (through
//! `@agentclientprotocol/claude-agent-acp`), Codex (through
//! `@agentclientprotocol/codex-acp`) and Cursor (`agent acp`). It is built on
//! the `agent-client-protocol` SDK and depends on no other Burrow crate:
//! `burrowd`'s adapter maps its types to Burrow's session events.
//!
//! ```no_run
//! # async fn example() -> Result<(), burrow_acp::AcpError> {
//! use burrow_acp::{AcpClient, AcpEvent, ConnectOptions, SessionEvent, launch};
//!
//! let cwd = std::env::current_dir().unwrap();
//! let command = launch::recommended_command(
//!     launch::KnownAgent::Cursor,
//!     std::path::Path::new(r"C:\Users\me\AppData\Local\Burrow\agents"),
//! )
//! .expect("Cursor is installed");
//! let agent = launch::spawn(&command, &cwd)?;
//! let (client, mut events) =
//!     AcpClient::connect_tokio(agent.stdin, agent.stdout, ConnectOptions::default()).await?;
//! let session = client.new_session(&cwd).await?;
//!
//! let prompting = client.clone();
//! let session_id = session.session_id.clone();
//! tokio::spawn(async move { prompting.prompt(&session_id, ["Hi".into()]).await });
//! while let Some(event) = events.recv().await {
//!     match event {
//!         AcpEvent::Session { event: SessionEvent::AgentMessageChunk { content, .. }, .. } => {
//!             print!("{}", content.as_text().unwrap_or_default());
//!         }
//!         AcpEvent::Session { event: SessionEvent::TurnEnded(_), .. } => break,
//!         AcpEvent::Permission(request) => request.allow_once()?,
//!         _ => {}
//!     }
//! }
//! client.stop().await;
//! # Ok(())
//! # }
//! ```
//!
//! **Ordering.** Session events, permission requests and the end of each turn
//! arrive on one [`AcpEvents`] stream, in the order the agent sent them.
//! [`SessionEvent::TurnEnded`] always follows the turn's last update.
//!
//! **Client capabilities.** None (D-021): Burrow doesn't offer `fs/*` or
//! `terminal/*`, so agents use their own file and shell tools, and ask for
//! permission through `session/request_permission`.

mod client;
mod error;
pub mod launch;
pub mod limit;
mod types;

pub use client::{
    AcpClient, AcpEvent, AcpEvents, ConnectOptions, PermissionDecision, PermissionRequest,
    PromptResult,
};
pub use error::AcpError;
pub use limit::{LimitInfo, LimitSource, RateLimitStatus};
pub use types::{
    AgentCapabilities, AgentInfo, AuthMethod, Choice, Command, ConfigCategory, ConfigOption,
    ConfigValue, Content, Cost, Location, ModeState, ModelState, PermissionKind, PermissionOption,
    PlanEntry, PromptContent, SessionEvent, SessionInfo, StopReason, TokenUsage, ToolCall,
    ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolKind, TurnEnd,
};
