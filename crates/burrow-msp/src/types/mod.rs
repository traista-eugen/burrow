//! MSP wire types: the subset of Muse's schema a coding session needs, written by hand (D-022).
//!
//! - Names follow the schema's `$defs`, so `TurnStartParams` here is `#/$defs/TurnStartParams`
//!   in `schema/msp.schema.json`. The schema checks in `src/schema_check.rs` rely on that.
//! - Fields are camelCase on the wire. Optional fields are left out when `None`.
//! - Members this crate doesn't model are ignored when decoding, so a newer Muse that adds
//!   members still decodes. Enums the schema marks open are `open_enum!`s.
//! - Every command (a method that changes state) takes a client-minted UUIDv7 `commandId`, which
//!   makes retries idempotent. The `new` constructors mint one with [`new_command_id`].

pub mod approval;
pub mod handshake;
pub mod item;
pub mod model;
pub mod session;
pub mod turn;
pub mod usage;
pub mod user_input;

pub use approval::*;
pub use handshake::*;
pub use item::*;
pub use model::*;
pub use session::*;
pub use turn::*;
pub use usage::*;
pub use user_input::*;

/// A fresh UUIDv7 for a `commandId`. Muse rejects anything that isn't a UUID.
pub fn new_command_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// The ack `status` of an admitted command: `accepted`, or `noop` for a few commands.
pub type CommandStatus = String;

open_enum! {
    /// How hard the model thinks, per turn or as a session default.
    pub enum ReasoningEffort {
        /// No reasoning.
        None = "none",
        /// Minimal.
        Minimal = "minimal",
        /// Low.
        Low = "low",
        /// Medium.
        Medium = "medium",
        /// High: Muse's default.
        High = "high",
        /// Extra high.
        Xhigh = "xhigh",
        /// Maximum.
        Max = "max",
        /// Beyond maximum, where a model offers it.
        Ultra = "ultra",
    }
}
