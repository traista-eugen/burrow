//! References to files in the artifact store.
//!
//! The artifact store itself (`artifact/*`) arrives later; M2 only needs the reference type, for
//! image content blocks and inbox attachments.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An immutable file in the artifact store, addressed by its SHA-256 hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRef {
    /// `sha256:` followed by the content's hash in lowercase hex.
    pub hash: String,
    /// A display name, for example `plan.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The media type, for example `image/png`.
    pub media_type: String,
    /// The size in bytes.
    pub size: u64,
}
