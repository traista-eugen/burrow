//! Projects: folders Burrow works in. Kept minimal in M2: add, list, get, remove.
//!
//! Each project is an entity row (`project`, D-017) and appears on the `index` stream as
//! `entityCreated` and `entityRemoved`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use burrow_agents::launcher::{self, LaunchSpec};
use burrow_protocol::{
    CodeHost, EntityCreated, EntityKind, EntityRemoved, ErrorKind, ErrorObject, Project,
    ProjectRemote, ProjectSettings, StreamName, Timestamp,
};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::hub::{Commit, Hub, HubError};
use crate::ids;

/// The entity kind of project rows.
const KIND: &str = "project";
/// How long `git remote get-url` may take.
const GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Every project, in memory and in the entities table.
#[derive(Debug)]
pub struct Projects {
    hub: Hub,
    projects: Mutex<BTreeMap<String, Project>>,
}

impl Projects {
    /// Loads the projects from the entities table.
    pub async fn load(hub: Hub) -> Result<Self, HubError> {
        let mut projects = BTreeMap::new();
        for row in hub.entities(KIND).await? {
            match row.body::<Project>() {
                Ok(project) => {
                    projects.insert(project.id.clone(), project);
                }
                Err(error) => {
                    warn!(id = row.id, %error, "skipping a project row that doesn't decode")
                }
            }
        }
        Ok(Self {
            hub,
            projects: Mutex::new(projects),
        })
    }

    /// Every project, oldest first.
    pub async fn list(&self) -> Vec<Project> {
        self.projects.lock().await.values().cloned().collect()
    }

    /// One project.
    pub async fn get(&self, id: &str) -> Result<Project, ErrorObject> {
        self.projects
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| not_found(id))
    }

    /// Makes `path` a project, or returns the project it already is.
    pub async fn add(&self, path: &str) -> Result<Project, ErrorObject> {
        let path = folder(path)?;
        let display = display_path(&path);
        // Outside the lock: git can take a moment.
        let remote = git_origin(&path).await;

        let mut projects = self.projects.lock().await;
        if let Some(existing) = projects
            .values()
            .find(|project| project.path.eq_ignore_ascii_case(&display))
        {
            return Ok(existing.clone());
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| display.clone());
        let project = Project {
            id: ids::new_id("prj"),
            name,
            path: display,
            remote,
            settings: ProjectSettings::default(),
            created_at: Timestamp::now(),
        };
        let mut commit = Commit::new();
        commit
            .event(
                &StreamName::index(),
                EntityCreated {
                    entity: EntityKind::Project,
                    id: project.id.clone(),
                    title: Some(project.name.clone()),
                    project_id: None,
                    state: None,
                },
            )
            .put(KIND, &project.id, &project);
        self.hub.commit(commit).await.map_err(internal)?;
        info!(id = project.id, path = project.path, "project added");
        projects.insert(project.id.clone(), project.clone());
        Ok(project)
    }

    /// Forgets a project. Never touches its files.
    pub async fn remove(&self, id: &str) -> Result<(), ErrorObject> {
        let mut projects = self.projects.lock().await;
        if !projects.contains_key(id) {
            return Err(not_found(id));
        }
        let mut commit = Commit::new();
        commit
            .event(
                &StreamName::index(),
                EntityRemoved {
                    entity: EntityKind::Project,
                    id: id.to_owned(),
                },
            )
            .delete(KIND, id);
        self.hub.commit(commit).await.map_err(internal)?;
        projects.remove(id);
        info!(id, "project removed");
        Ok(())
    }
}

fn not_found(id: &str) -> ErrorObject {
    ErrorObject::new(ErrorKind::NotFound, format!("no project {id}"))
}

fn internal(error: HubError) -> ErrorObject {
    ErrorObject::internal(error.to_string())
}

/// `path` as an existing folder's canonical path, or invalid params.
pub fn folder(path: &str) -> Result<PathBuf, ErrorObject> {
    let given = Path::new(path);
    if !given.is_absolute() {
        return Err(ErrorObject::invalid_params(format!(
            "{path} isn't an absolute path"
        )));
    }
    let canonical = std::fs::canonicalize(given)
        .map_err(|error| ErrorObject::invalid_params(format!("{path}: {error}")))?;
    if !canonical.is_dir() {
        return Err(ErrorObject::invalid_params(format!(
            "{path} isn't a folder"
        )));
    }
    Ok(canonical)
}

/// A path for display and storage: without the `\\?\` prefix `canonicalize` adds on Windows,
/// when the path doesn't need it.
pub fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.len() < 260 && !rest.starts_with("UNC\\") => rest.to_owned(),
        Some(rest) if rest.starts_with("UNC\\") => format!(r"\\{}", &rest[4..]),
        _ => text,
    }
}

/// The folder's `origin` remote, if it is a git repository with one and git is installed.
async fn git_origin(path: &Path) -> Option<ProjectRemote> {
    let git = launcher::find_program("git")?;
    let spec = LaunchSpec::new(git, ["remote", "get-url", "origin"], "git remote").cwd(path);
    match launcher::run_capture(&spec, GIT_TIMEOUT).await {
        Ok(output) if output.exit.success() => {
            let url = output.stdout.trim();
            (!url.is_empty()).then(|| parse_remote(url))
        }
        Ok(output) => {
            debug!(path = %path.display(), stderr = output.stderr, "no origin remote");
            None
        }
        Err(error) => {
            debug!(path = %path.display(), %error, "git failed");
            None
        }
    }
}

/// Recognises GitHub URLs: `https://github.com/owner/repo(.git)`, `git@github.com:owner/repo`,
/// `ssh://git@github.com/owner/repo.git`.
pub fn parse_remote(url: &str) -> ProjectRemote {
    let mut remote = ProjectRemote {
        url: url.to_owned(),
        host: None,
        owner: None,
        repo: None,
    };
    let rest = if let Some(rest) = url.strip_prefix("git@github.com:") {
        Some(rest)
    } else {
        url.split_once("://")
            .map(|(_, rest)| rest)
            .and_then(|rest| {
                let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
                rest.strip_prefix("github.com/")
                    .or_else(|| rest.strip_prefix("www.github.com/"))
            })
    };
    if let Some(rest) = rest {
        let mut parts = rest.trim_end_matches('/').splitn(3, '/');
        if let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) {
            let repo = repo.strip_suffix(".git").unwrap_or(repo);
            if !owner.is_empty() && !repo.is_empty() {
                remote.host = Some(CodeHost::Github);
                remote.owner = Some(owner.to_owned());
                remote.repo = Some(repo.to_owned());
            }
        }
    }
    remote
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_remotes_in_every_form() {
        for url in [
            "https://github.com/traista-eugen/burrow.git",
            "https://github.com/traista-eugen/burrow",
            "https://token@github.com/traista-eugen/burrow/",
            "git@github.com:traista-eugen/burrow.git",
            "ssh://git@github.com/traista-eugen/burrow.git",
        ] {
            let remote = parse_remote(url);
            assert_eq!(remote.host, Some(CodeHost::Github), "{url}");
            assert_eq!(remote.owner.as_deref(), Some("traista-eugen"), "{url}");
            assert_eq!(remote.repo.as_deref(), Some("burrow"), "{url}");
            assert_eq!(remote.url, url);
        }
        for url in [
            "https://gitlab.com/team/repo.git",
            "https://github.com/only-owner",
            "D:\\repos\\bare.git",
        ] {
            let remote = parse_remote(url);
            assert_eq!(remote.host, None, "{url}");
            assert_eq!(remote.owner, None, "{url}");
        }
    }

    #[test]
    fn display_paths_drop_the_verbatim_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\D:\Repos\burrow")),
            r"D:\Repos\burrow"
        );
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\x")),
            r"\\server\share\x"
        );
        assert_eq!(display_path(Path::new(r"D:\plain")), r"D:\plain");
    }
}
