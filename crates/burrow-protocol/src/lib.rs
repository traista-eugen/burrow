//! Wire protocol shared by `burrowd` and every client (desktop app, phone, CLI).
//!
//! Intentionally empty for now: the message set will be designed once the
//! remaining product requirements are in.

/// Bumped on any breaking change to the wire format.
pub const PROTOCOL_VERSION: u32 = 0;
