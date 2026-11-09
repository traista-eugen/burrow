//! How to start each ACP agent on Windows, and a helper that spawns one.
//!
//! Burrow's core spawns agents itself, inside a Job Object (D-007); it only
//! needs the [`AgentCommand`]. [`spawn`] is for tests and tools.
//!
//! The launch strategy (D-020):
//! - **Cursor** ships its own `node.exe`. Its `agent.cmd` shim starts
//!   PowerShell, which picks the newest folder under `versions\` and runs
//!   `node.exe index.js`. Burrow skips both shells and runs that `node.exe`
//!   directly: [`cursor_command`].
//! - **Claude Code and Codex** go through npm adapters, installed once into a
//!   Burrow-owned folder at a pinned version ([`NpmAdapter::install_command`])
//!   and run as `node <entry point>` ([`NpmAdapter::command`]). `npx -y`
//!   ([`NpmAdapter::npx_command`]) works too, but is slower to start and
//!   needs the network the first time.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::error::AcpError;

/// The agents this crate knows how to launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnownAgent {
    /// Claude Code, through `@agentclientprotocol/claude-agent-acp`.
    ClaudeCode,
    /// Codex, through `@agentclientprotocol/codex-acp`.
    Codex,
    /// The Cursor CLI's built-in ACP server, `agent acp`.
    Cursor,
}

/// A command line that starts an agent speaking ACP on its stdio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    /// The program to run.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
    /// Extra environment variables.
    pub env: Vec<(OsString, OsString)>,
}

impl AgentCommand {
    /// A command that runs `program` with no arguments.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Adds an argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Sets an environment variable.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// A tokio command for it: stdio piped, killed when dropped, and on
    /// Windows without a console window.
    pub fn to_tokio(&self, cwd: &Path) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
    }
}

impl std::fmt::Display for AgentCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.program.display())?;
        for arg in &self.args {
            write!(f, " {}", arg.to_string_lossy())?;
        }
        Ok(())
    }
}

/// An ACP adapter published on npm, pinned to the version Burrow is tested
/// with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NpmAdapter {
    /// The npm package.
    pub package: &'static str,
    /// The pinned version.
    pub version: &'static str,
    /// The package's `bin` script, relative to the package folder.
    pub entry: &'static str,
}

/// Claude Code's ACP adapter. It bundles the Claude Agent SDK and its own
/// Claude Code, and uses the login of the installed `claude` CLI.
pub const CLAUDE_AGENT_ACP: NpmAdapter = NpmAdapter {
    package: "@agentclientprotocol/claude-agent-acp",
    version: "0.101.0",
    entry: "dist/index.js",
};

/// Codex's ACP adapter. It bundles its own Codex (0.166.0 for adapter 2.6.0),
/// runs `codex app-server`, and shares the installed CLI's `~/.codex` login
/// and config.
pub const CODEX_ACP: NpmAdapter = NpmAdapter {
    package: "@agentclientprotocol/codex-acp",
    version: "2.6.0",
    entry: "dist/index.js",
};

impl NpmAdapter {
    /// The adapter for an agent, if it uses one.
    pub fn for_agent(agent: KnownAgent) -> Option<NpmAdapter> {
        match agent {
            KnownAgent::ClaudeCode => Some(CLAUDE_AGENT_ACP),
            KnownAgent::Codex => Some(CODEX_ACP),
            KnownAgent::Cursor => None,
        }
    }

    /// The package name without its scope, for example `codex-acp`.
    pub fn short_name(&self) -> &'static str {
        self.package.rsplit('/').next().unwrap_or(self.package)
    }

    /// Where this version is installed under `agents_dir`:
    /// `<agents_dir>\<short name>\<version>`.
    pub fn install_dir(&self, agents_dir: &Path) -> PathBuf {
        agents_dir.join(self.short_name()).join(self.version)
    }

    /// The script `node` runs.
    pub fn entry_point(&self, agents_dir: &Path) -> PathBuf {
        let mut path = self.install_dir(agents_dir).join("node_modules");
        for part in self.package.split('/') {
            path.push(part);
        }
        for part in self.entry.split('/') {
            path.push(part);
        }
        path
    }

    /// Whether this version is installed under `agents_dir`.
    pub fn is_installed(&self, agents_dir: &Path) -> bool {
        self.entry_point(agents_dir).is_file()
    }

    /// The command that installs this version under `agents_dir`. `npm` is
    /// `npm.cmd` on Windows. Takes a few seconds and a few hundred MB.
    pub fn install_command(&self, agents_dir: &Path, npm: &Path) -> AgentCommand {
        AgentCommand::new(npm)
            .arg("install")
            .arg("--prefix")
            .arg(self.install_dir(agents_dir))
            .arg("--no-fund")
            .arg("--no-audit")
            .arg("--omit=dev")
            .arg(format!("{}@{}", self.package, self.version))
    }

    /// The recommended way to start the adapter: `node <entry point>`, from
    /// the Burrow-local install. `None` if it isn't installed.
    pub fn command(&self, agents_dir: &Path, node: &Path) -> Option<AgentCommand> {
        let entry = self.entry_point(agents_dir);
        entry
            .is_file()
            .then(|| AgentCommand::new(node).arg(entry.into_os_string()))
    }

    /// Starts the adapter through `npx -y <package>@<version>`. Needs no
    /// install step, but each start runs npm first (seconds) and the first
    /// start downloads the package.
    pub fn npx_command(&self, npx: &Path) -> AgentCommand {
        AgentCommand::new(npx)
            .arg("-y")
            .arg(format!("{}@{}", self.package, self.version))
    }
}

/// The Cursor CLI's install folder: `%LOCALAPPDATA%\cursor-agent`.
pub fn cursor_install_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("cursor-agent"))
}

/// `agent acp`, run the way Cursor's `agent.ps1` would run it, without the
/// `cmd` and PowerShell shims: `versions\<newest>\node.exe
/// versions\<newest>\index.js acp`. `None` if no version is installed under
/// `install_dir`.
pub fn cursor_command(install_dir: &Path) -> Option<AgentCommand> {
    let version = newest_cursor_version(&install_dir.join("versions"))?;
    let dir = install_dir.join("versions").join(&version);
    let node = dir.join(if cfg!(windows) { "node.exe" } else { "node" });
    let index = dir.join("index.js");
    if !node.is_file() || !index.is_file() {
        return None;
    }
    let mut command = AgentCommand::new(node)
        .arg(index.into_os_string())
        .arg("acp")
        // What the shim sets; Cursor uses it in messages about itself.
        .env("CURSOR_INVOKED_AS", "agent.cmd");
    // The shim turns on Node's compile cache, which speeds up start.
    if std::env::var_os("NODE_COMPILE_CACHE").is_none()
        && let Some(local) = std::env::var_os("LOCALAPPDATA")
    {
        command = command.env(
            "NODE_COMPILE_CACHE",
            PathBuf::from(local).join("cursor-compile-cache"),
        );
    }
    Some(command)
}

/// The newest version folder name under Cursor's `versions` folder. Names
/// look like `2026.11.05-86047df` or `2026.11.05-14-03-59-86047df`.
pub fn newest_cursor_version(versions_dir: &Path) -> Option<String> {
    std::fs::read_dir(versions_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| cursor_version_key(&name).map(|key| (key, name)))
        .max()
        .map(|(_, name)| name)
}

/// Sort key for a Cursor version folder name: date, then build time.
fn cursor_version_key(name: &str) -> Option<[u32; 6]> {
    let parts: Vec<&str> = name.split('-').collect();
    let (date, rest) = parts.split_first()?;
    let (commit, time) = rest.split_last()?;
    if commit.is_empty() || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let date: Vec<u32> = date
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let [year, month, day] = date[..] else {
        return None;
    };
    let time: Vec<u32> = time
        .iter()
        .map(|part| part.parse())
        .collect::<Result<_, _>>()
        .ok()?;
    let [hour, minute, second] = match time[..] {
        [] => [0, 0, 0],
        [h, m, s] => [h, m, s],
        _ => return None,
    };
    Some([year, month, day, hour, minute, second])
}

/// Finds a program on `PATH`, trying `PATHEXT` extensions on Windows.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions: Vec<OsString> = if cfg!(windows) && Path::new(name).extension().is_none() {
        std::env::var_os("PATHEXT")
            .unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"))
            .to_string_lossy()
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(OsString::from)
            .collect()
    } else {
        vec![OsString::new()]
    };
    std::env::split_paths(&path).find_map(|dir| {
        extensions.iter().find_map(|ext| {
            let mut file = OsString::from(name);
            file.push(ext);
            let candidate = dir.join(&file);
            candidate.is_file().then_some(candidate)
        })
    })
}

/// The recommended command for `agent`, or `None` if what it needs isn't
/// installed. `agents_dir` is where npm adapters are installed (Burrow uses
/// `%LOCALAPPDATA%\Burrow\agents`).
pub fn recommended_command(agent: KnownAgent, agents_dir: &Path) -> Option<AgentCommand> {
    match agent {
        KnownAgent::Cursor => cursor_command(&cursor_install_dir()?),
        KnownAgent::ClaudeCode | KnownAgent::Codex => {
            let adapter = NpmAdapter::for_agent(agent)?;
            adapter.command(agents_dir, &find_on_path("node")?)
        }
    }
}

/// A spawned agent process. The caller owns it; dropping it kills the
/// process (not its children: Burrow's core uses a Job Object for that).
#[derive(Debug)]
pub struct SpawnedAgent {
    /// The process.
    pub child: tokio::process::Child,
    /// Its stdin: the client writes here.
    pub stdin: tokio::process::ChildStdin,
    /// Its stdout: the client reads here.
    pub stdout: tokio::process::ChildStdout,
    /// Its stderr, for logs.
    pub stderr: tokio::process::ChildStderr,
}

/// Spawns `command` in `cwd` with piped stdio. For tests and tools: it uses
/// no Job Object.
pub fn spawn(command: &AgentCommand, cwd: &Path) -> Result<SpawnedAgent, AcpError> {
    let mut child = command.to_tokio(cwd).spawn().map_err(AcpError::Spawn)?;
    let missing = || AcpError::Spawn(std::io::Error::other("stdio wasn't piped"));
    let stdin = child.stdin.take().ok_or_else(missing)?;
    let stdout = child.stdout.take().ok_or_else(missing)?;
    let stderr = child.stderr.take().ok_or_else(missing)?;
    Ok(SpawnedAgent {
        child,
        stdin,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_versions_sort_by_date_then_build_time() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "2026.9.30-aaaa111",
            "2026.11.05-86047df",
            "2026.11.05-14-03-59-0123abc",
            "2026.10.12-bbbb222",
            "not-a-version",
            "2027.01.01-zzzz", // not hex
        ] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        std::fs::write(dir.path().join("2030.1.1-ffff"), "a file, not a folder").unwrap();
        assert_eq!(
            newest_cursor_version(dir.path()).as_deref(),
            Some("2026.11.05-14-03-59-0123abc")
        );
    }

    #[test]
    fn cursor_command_runs_the_bundled_node_directly() {
        let dir = tempfile::tempdir().unwrap();
        let version = dir.path().join("versions").join("2026.11.05-86047df");
        std::fs::create_dir_all(&version).unwrap();
        let node = if cfg!(windows) { "node.exe" } else { "node" };
        std::fs::write(version.join(node), "").unwrap();
        std::fs::write(version.join("index.js"), "").unwrap();
        let command = cursor_command(dir.path()).expect("a command");
        assert_eq!(command.program, version.join(node));
        assert_eq!(
            command.args,
            [
                version.join("index.js").into_os_string(),
                OsString::from("acp")
            ]
        );
        assert!(cursor_command(&dir.path().join("missing")).is_none());
    }

    #[test]
    fn npm_adapters_install_under_a_versioned_folder() {
        let agents = Path::new(r"C:\Burrow\agents");
        assert_eq!(
            CODEX_ACP.install_dir(agents),
            agents.join("codex-acp").join("2.6.0")
        );
        assert_eq!(
            CLAUDE_AGENT_ACP.entry_point(agents),
            agents
                .join("claude-agent-acp")
                .join("0.101.0")
                .join("node_modules")
                .join("@agentclientprotocol")
                .join("claude-agent-acp")
                .join("dist")
                .join("index.js")
        );
        let install = CODEX_ACP.install_command(agents, Path::new("npm.cmd"));
        assert_eq!(
            install.args.last().unwrap(),
            "@agentclientprotocol/codex-acp@2.6.0"
        );
        assert_eq!(install.program, Path::new("npm.cmd"));
    }
}
