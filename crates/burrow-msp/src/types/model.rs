//! Models: the catalog (`model/list`) and the picker (`session/setModel`).
//!
//! Also see [`crate::SessionStartParams::model_id`] for the starting model, and
//! [`crate::MuseCommand::model`] for the host's default. The `echo` provider has no models:
//! `session/setModel` on an echo session is refused (`commandRejected`, reason
//! `invalid_target`), though `model/list` still lists the `meta` catalog.

use serde::{Deserialize, Serialize};

use super::{CommandStatus, ReasoningEffort, new_command_id};

/// `model/list` params. A query: no `commandId`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ModelListParams {
    /// Mark this session's model `isActive`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// A model's list price, as decimal strings per million tokens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    /// Cached input.
    pub cached: String,
    /// The currency, such as `USD`.
    #[serde(default)]
    pub currency: Option<String>,
    /// Input.
    pub input: String,
    /// Output.
    pub output: String,
}

/// The reasoning efforts a model offers, or `"unknown"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum ModelVariants {
    /// The efforts, in order.
    Known(Vec<ReasoningEffort>),
    /// The host doesn't know; observed as `"unknown"` on echo hosts.
    Unknown(String),
}

/// One selectable model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogEntry {
    /// Context window in tokens, when declared.
    #[serde(default)]
    pub context_limit: Option<u64>,
    /// List price, when declared.
    #[serde(default)]
    pub cost: Option<ModelCost>,
    /// The model's default effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<ReasoningEffort>,
    /// A description, when declared.
    #[serde(default)]
    pub description: Option<String>,
    /// The label to show.
    pub display_label: String,
    /// The session's current model (only with `sessionId`). Possibly false for every row.
    pub is_active: bool,
    /// The catalog's default. Possibly false for every row.
    pub is_default: bool,
    /// The id to select.
    pub model_id: String,
    /// Output limit in tokens, when declared.
    #[serde(default)]
    pub output_limit: Option<u64>,
    /// The provider profile, when declared.
    #[serde(default)]
    pub profile_id: Option<String>,
    /// The provider.
    pub provider_id: String,
    /// Release date, when declared.
    #[serde(default)]
    pub release_date: Option<String>,
    /// The selectable efforts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<ModelVariants>,
}

/// `model/list` result. `models` may be empty, for example while the provider's catalog is
/// still loading (`source: bundledCatalog`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ModelListResult {
    /// The models, newest first.
    pub models: Vec<ModelCatalogEntry>,
    /// The catalog's profile.
    #[serde(default)]
    pub profile_id: Option<String>,
    /// The catalog's provider.
    pub provider_id: String,
    /// Where the catalog came from: `providerCatalog`, `bundledCatalog`, `fakeCatalog`, and so
    /// on.
    pub source: String,
}

/// A model to switch to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    /// A label to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_label: Option<String>,
    /// The catalog id.
    pub model_id: String,
    /// The provider profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// The provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
}

/// `session/setModel` params. Takes effect at the next model call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionSetModelParams {
    /// Idempotency handle (UUIDv7).
    pub command_id: String,
    /// The model.
    pub model: ModelSelection,
    /// The session.
    pub session_id: String,
}

impl SessionSetModelParams {
    /// Switches `session_id` to `model_id`, with a fresh `commandId`.
    pub fn new(session_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            command_id: new_command_id(),
            model: ModelSelection {
                display_label: None,
                model_id: model_id.into(),
                profile_id: None,
                provider_id: None,
            },
            session_id: session_id.into(),
        }
    }
}

/// `session/setModel` result: admitted. `session/modelChanged` follows when it takes effect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SessionSetModelResult {
    /// Echo of the `commandId`.
    pub command_id: String,
    /// Admission status.
    pub status: CommandStatus,
}
