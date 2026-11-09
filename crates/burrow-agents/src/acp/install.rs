//! Installing an npm ACP adapter at its pinned version, into Burrow's own folder (D-020,
//! D-024).
//!
//! npm installs into a staging folder next to the final one, `<version>.installing`, which is
//! renamed to `<version>` only once npm succeeded and the entry point exists. So an install that
//! fails, times out or is cancelled never leaves a folder that looks installed; the next install
//! starts the staging folder over.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use burrow_acp::launch::NpmAdapter;
use tracing::{info, warn};

use crate::adapter::AgentError;
use crate::launcher::{self, LaunchSpec};

/// How long an install may take. Measured: 5 to 9 seconds with a warm npm cache.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// How to run npm: `node npm-cli.js`, or the `npm.cmd` shim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Npm {
    /// `node.exe`, or `npm.cmd`.
    pub program: PathBuf,
    /// Arguments that come before npm's own: the path of `npm-cli.js` when running node.
    pub prefix_args: Vec<OsString>,
}

impl Npm {
    /// Finds npm: the `npm-cli.js` that ships next to `node.exe`, run by that node, so no
    /// `cmd.exe` sits in between; else `npm.cmd` on `PATH`.
    pub fn find() -> Option<Self> {
        if let Some(node) = find_node() {
            let cli = node.parent().map(|dir| {
                dir.join("node_modules")
                    .join("npm")
                    .join("bin")
                    .join("npm-cli.js")
            });
            if let Some(cli) = cli.filter(|cli| cli.is_file()) {
                return Some(Self {
                    program: node,
                    prefix_args: vec![cli.into_os_string()],
                });
            }
        }
        launcher::find_program_in("npm", &node_dirs()).map(|program| Self {
            program,
            prefix_args: Vec::new(),
        })
    }
}

/// Finds `node.exe`: on `PATH`, then in Node's default install folder.
pub fn find_node() -> Option<PathBuf> {
    launcher::find_program_in("node", &node_dirs())
}

fn node_dirs() -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|dir| PathBuf::from(dir).join("nodejs"))
        .collect()
}

/// Installs `adapter` under `agents_dir` with `npm`, unless it is installed already.
pub async fn install(adapter: &NpmAdapter, agents_dir: &Path, npm: &Npm) -> Result<(), AgentError> {
    let target = adapter.install_dir(agents_dir);
    if adapter.is_installed(agents_dir) {
        return Ok(());
    }
    let staging = staging_dir(&target);
    remove_dir(&staging).await?;
    let parent = target.parent().expect("an install folder has a parent");
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| failed(format!("can't create {}: {error}", parent.display())))?;

    let mut args = npm.prefix_args.clone();
    args.extend(adapter.install_args(&staging));
    let spec = LaunchSpec::new(
        &npm.program,
        args,
        format!("npm install {}@{}", adapter.package, adapter.version),
    )
    .cwd(parent);
    info!(package = adapter.package, version = adapter.version, dir = %target.display(), "installing an ACP adapter");
    let output = launcher::run_capture(&spec, INSTALL_TIMEOUT)
        .await
        .map_err(|error| failed(format!("npm didn't finish: {error}")))?;
    if !output.exit.success() {
        let detail = last_lines(&output.text(), 6);
        return Err(failed(format!(
            "npm install {}@{} failed ({}): {detail}",
            adapter.package, adapter.version, output.exit.description
        )));
    }
    if !adapter.entry_in(&staging).is_file() {
        return Err(failed(format!(
            "npm finished, but {} isn't there",
            adapter.entry_in(&staging).display()
        )));
    }

    // A folder without the entry point is a broken earlier install; replace it.
    remove_dir(&target).await?;
    tokio::fs::rename(&staging, &target)
        .await
        .map_err(|error| {
            failed(format!(
                "can't move {} to {}: {error}",
                staging.display(),
                target.display()
            ))
        })?;
    info!(
        package = adapter.package,
        version = adapter.version,
        "installed"
    );
    Ok(())
}

/// `<version>.installing`, next to `<version>`.
fn staging_dir(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_owned();
    name.push(".installing");
    target.with_file_name(name)
}

async fn remove_dir(dir: &Path) -> Result<(), AgentError> {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            warn!(dir = %dir.display(), %error, "can't remove an old install folder");
            Err(failed(format!("can't remove {}: {error}", dir.display())))
        }
    }
}

fn failed(message: String) -> AgentError {
    AgentError::Failed(message)
}

/// The last `count` non-empty lines of `text`, joined with ` | `.
fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines[lines.len().saturating_sub(count)..].join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADAPTER: NpmAdapter = NpmAdapter {
        package: "@agentclientprotocol/test-acp",
        version: "1.2.3",
        entry: "dist/index.js",
    };

    /// A stand-in for npm: a batch file that creates the entry point under `--prefix`, or fails
    /// when the package name says so.
    fn fake_npm(dir: &Path, works: bool) -> Npm {
        let script = dir.join("fake-npm.cmd");
        let body = if works {
            // %3 is the prefix: install --prefix <dir> ...
            "@echo off\r\nmkdir \"%~3\\node_modules\\@agentclientprotocol\\test-acp\\dist\"\r\n\
             echo // entry> \"%~3\\node_modules\\@agentclientprotocol\\test-acp\\dist\\index.js\"\r\n\
             echo added 1 package\r\n"
        } else {
            "@echo off\r\necho npm error code E404\r\necho npm error 404 Not Found 1>&2\r\nexit /b 1\r\n"
        };
        std::fs::write(&script, body).unwrap();
        Npm {
            program: script,
            prefix_args: Vec::new(),
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn installs_into_a_staging_folder_then_moves_it_into_place() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        // Leftovers of an interrupted install and of a broken one.
        let target = ADAPTER.install_dir(&agents);
        std::fs::create_dir_all(staging_dir(&target).join("junk")).unwrap();
        std::fs::create_dir_all(target.join("broken")).unwrap();

        install(&ADAPTER, &agents, &fake_npm(dir.path(), true))
            .await
            .unwrap();
        assert!(ADAPTER.is_installed(&agents));
        assert!(!staging_dir(&target).exists());
        assert!(!target.join("broken").exists());

        // Installed already: npm doesn't run again (this "npm" would fail).
        install(&ADAPTER, &agents, &fake_npm(dir.path(), false))
            .await
            .unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_failed_install_reports_npm_and_leaves_nothing_installed() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        let error = install(&ADAPTER, &agents, &fake_npm(dir.path(), false))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("exit code 1"), "{message}");
        assert!(message.contains("E404"), "{message}");
        assert!(!ADAPTER.is_installed(&agents));
    }

    #[test]
    fn staging_sits_next_to_the_version_folder() {
        assert_eq!(
            staging_dir(Path::new(r"C:\agents\codex-acp\2.6.0")),
            Path::new(r"C:\agents\codex-acp\2.6.0.installing")
        );
        assert_eq!(last_lines("a\n\nb\nc\n", 2), "b | c");
    }
}
