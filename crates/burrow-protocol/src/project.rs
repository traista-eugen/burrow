//! Projects: `project/list`, `project/add`, `project/get` and `project/remove`.
//!
//! A project is a local folder Burrow works in, usually a git repository, plus its settings.
//! Projects appear on the `index` stream as `entityCreated` and `entityRemoved`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::time::Timestamp;

/// A local folder Burrow works in, usually a git repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// The project's id (`prj_…`).
    pub id: String,
    /// The display name: the folder's name.
    pub name: String,
    /// The folder's absolute path.
    pub path: String,
    /// The git `origin` remote, when the folder is a git repository that has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<ProjectRemote>,
    /// The project's settings.
    #[serde(default)]
    pub settings: ProjectSettings,
    /// When the project was added.
    pub created_at: Timestamp,
}

/// A project's git `origin` remote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRemote {
    /// The remote's URL, as git reports it.
    pub url: String,
    /// The code host, when Burrow recognises it. Absent for other hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<CodeHost>,
    /// The repository's owner on the code host, for example `traista-eugen`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The repository's name on the code host, for example `burrow`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

open_enum! {
    /// A code host Burrow knows how to work with.
    pub enum CodeHost {
        /// github.com.
        Github = "github",
    }
}

/// A project's settings. Empty for now: role overrides, the check command, `autoMerge` and
/// default flows arrive with the features that use them. Unknown members are ignored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ProjectSettings {}

/// `project/add` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAddParams {
    /// The folder, as an absolute path. Adding a folder that is already a project returns that
    /// project.
    pub path: String,
}

/// Params naming one project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIdParams {
    /// The project's id.
    pub project_id: String,
}
