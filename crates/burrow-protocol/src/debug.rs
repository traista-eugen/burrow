//! `debug/append`: a development-only method.
//!
//! `burrowd` accepts it only when started with `--dev`; otherwise it answers `methodNotFound`.
//! Tests use it to append events without real sessions or runs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::{Event, StreamName};

/// `debug/append` params: append one event to a stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DebugAppendParams {
    /// The stream to append to. Any name is allowed, including ones `burrowd` doesn't use itself.
    pub stream: StreamName,
    /// The event to append. Unknown event types are stored as sent.
    pub event: Event,
}

/// `debug/append` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DebugAppendResult {
    /// The new event's sequence number in its stream.
    pub seq: u64,
}
