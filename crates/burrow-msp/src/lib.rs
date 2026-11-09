//! A client for MSP, the Muse Session Protocol that Muse Code serves with `muse serve`.
//!
//! MSP is JSON-RPC 2.0 over the process's stdio, one message per line. This crate speaks it
//! over any `AsyncRead`/`AsyncWrite` pair, and doesn't depend on Burrow's other crates: its
//! types are MSP's own. Burrow's Muse adapter maps them to Burrow's session events.
//!
//! ```no_run
//! # async fn demo() -> burrow_msp::Result<()> {
//! use burrow_msp::{ConnectOptions, Event, MuseCommand, SessionStartParams};
//!
//! let mut muse = MuseCommand::locate().expect("muse is installed").provider("echo").spawn()?;
//! let (client, mut events) = muse.connect(ConnectOptions::default()).await?;
//! let session = client.start_session(&SessionStartParams::new(r"C:\work\repo")).await?;
//! let turn = client.prompt(&session.session.session_id, "Hello").await?;
//! while let Some(event) = events.recv().await {
//!     match event {
//!         Event::ItemDelta { delta, .. } => print!("{}", delta.delta),
//!         Event::ApprovalRequested(request) => {
//!             let choice = request.allow_once().expect("an allowing choice").clone();
//!             client.decide_approval(&request, &choice).await?;
//!         }
//!         Event::TurnCompleted(done) if done.turn_id == turn.turn_id => break,
//!         _ => {}
//!     }
//! }
//! muse.shutdown(&client, std::time::Duration::from_secs(5)).await?;
//! # Ok(()) }
//! ```
//!
//! - **Types** ([`types`]) are a hand-written subset of Muse's exported schema, which is
//!   committed in `schema/` with the Muse version it came from. Tests check the types against
//!   it (D-022).
//! - **The client** ([`MspClient`]) does the handshake, typed calls and shutdown, and answers
//!   Muse's approval and user-input requests with the presentation receipt.
//! - **Events** ([`Events`]) are every notification, typed, in order.
//! - **Spawning** ([`MuseCommand`], [`MuseProcess`]) runs the versioned Muse binary directly,
//!   not through its launcher (D-023).

#[macro_use]
mod open_enum;

mod client;
mod error;
mod event;
mod framing;
pub mod methods;
mod rpc;
mod spawn;
pub mod types;

#[cfg(test)]
mod schema_check;

pub use client::{ConnectOptions, MspClient, SCHEMA_FINGERPRINT, SCHEMA_MUSE_VERSION};
pub use error::{ErrorKind, MspError, Result};
pub use event::{Event, Events};
pub use framing::MAX_LINE_BYTES;
pub use rpc::{RequestId, RpcError};
pub use spawn::{MuseCommand, MuseProcess, StderrTail, launcher_binary, locate_muse};
pub use types::*;
