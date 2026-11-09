//! Starting `muse serve`: finding the binary, building the command, and owning the process.
//!
//! Burrow's core starts agents inside Job Objects (D-007), so it builds the command with
//! [`MuseCommand::std_command`], spawns it its own way, and wraps the child with
//! [`MuseProcess::from_child`]. [`MuseCommand::spawn`] is the plain version, for tests and
//! tools.
//!
//! **The launcher.** On Windows `muse` is `muse.cmd`, which runs a PowerShell launcher, which
//! checks for updates (sometimes starting a background updater) and then runs
//! `muse-bin-<version>.exe`. [`locate_muse`] skips all that and returns the versioned binary the
//! launcher would run, named in `.muse-version` next to it (D-023). The binary then is the
//! process Burrow talks to and kills, with nothing in between.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, ChildStderr};
use tokio::task::JoinHandle;
use tracing::debug;

use crate::client::{ConnectOptions, MspClient};
use crate::error::{MspError, Result};
use crate::event::Events;

/// Lines of stderr kept for error reports.
const STDERR_LINES: usize = 64;

/// `CREATE_NO_WINDOW`: no console window flashes up when a windowless service starts Muse.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Finds the Muse binary on `PATH`: `muse.exe` (`muse` elsewhere), or the versioned binary
/// behind a `muse.cmd` launcher. `None` if Muse isn't installed.
pub fn locate_muse() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| muse_in(&dir))
}

fn muse_in(dir: &Path) -> Option<PathBuf> {
    let program = dir.join(format!("muse{}", std::env::consts::EXE_SUFFIX));
    if program.is_file() {
        // A Unix launcher is a script named `muse`; Windows' `muse.exe` is the real thing.
        if cfg!(not(windows))
            && let Some(binary) = launcher_binary(&program)
        {
            return Some(binary);
        }
        return Some(program);
    }
    if cfg!(windows) {
        for name in ["muse.cmd", "muse.bat"] {
            let launcher = dir.join(name);
            if launcher.is_file() {
                return Some(launcher_binary(&launcher).unwrap_or(launcher));
            }
        }
    }
    None
}

/// The binary Muse's launcher at `launcher` would run: `muse-bin-<version>` (plus `.exe` on
/// Windows) in the same folder, where `.muse-version` names the version. `None` when that
/// layout isn't there.
pub fn launcher_binary(launcher: &Path) -> Option<PathBuf> {
    let dir = launcher.parent()?;
    let version = std::fs::read_to_string(dir.join(".muse-version")).ok()?;
    let version = version.trim();
    let safe = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
    if version.is_empty() || !version.chars().all(safe) {
        return None;
    }
    let binary = dir.join(format!(
        "muse-bin-{version}{}",
        std::env::consts::EXE_SUFFIX
    ));
    binary.is_file().then_some(binary)
}

/// A `muse serve` command line.
#[derive(Clone, Debug)]
pub struct MuseCommand {
    program: PathBuf,
    args: Vec<OsString>,
    envs: Vec<(OsString, Option<OsString>)>,
    current_dir: Option<PathBuf>,
}

impl MuseCommand {
    /// `<program> serve`.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: vec!["serve".into()],
            envs: Vec::new(),
            current_dir: None,
        }
    }

    /// `muse serve`, with the binary [`locate_muse`] finds. `None` if Muse isn't installed.
    pub fn locate() -> Option<Self> {
        locate_muse().map(Self::new)
    }

    /// `--provider <provider>`: `echo` (free, no tokens), `meta` (the default) or `local`
    /// (macOS only).
    pub fn provider(self, provider: impl AsRef<OsStr>) -> Self {
        self.arg("--provider").arg(provider)
    }

    /// `--model <model>`: the host's default model, such as `muse-spark-1.3`.
    pub fn model(self, model: impl AsRef<OsStr>) -> Self {
        self.arg("--model").arg(model)
    }

    /// `--no-session-log`: memory-only sessions, nothing written under Muse's home.
    pub fn no_session_log(self) -> Self {
        self.arg("--no-session-log")
    }

    /// `--disable-request-user-input`: the model can't ask the user questions.
    pub fn disable_request_user_input(self) -> Self {
        self.arg("--disable-request-user-input")
    }

    /// `--disable-sandbox`: no shell sandboxing. On Windows the shell tool fails without this
    /// until `muse sandbox setup` has run.
    pub fn disable_sandbox(self) -> Self {
        self.arg("--disable-sandbox")
    }

    /// `-c <key>=<value>`: one setting for this host. Values are JSON.
    pub fn config(self, key: &str, value: &str) -> Self {
        self.arg("-c").arg(format!("{key}={value}"))
    }

    /// Any other argument.
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    /// Sets an environment variable for the process.
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.envs
            .push((key.as_ref().to_owned(), Some(value.as_ref().to_owned())));
        self
    }

    /// Removes an environment variable for the process.
    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Self {
        self.envs.push((key.as_ref().to_owned(), None));
        self
    }

    /// The process's working directory. Sessions name their own workspace, so this only
    /// matters for relative paths.
    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }

    /// The program.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The arguments, starting with `serve`.
    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    /// The command, with stdin, stdout and stderr piped and, on Windows, no console window.
    pub fn std_command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &self.envs {
            match value {
                Some(value) => command.env(key, value),
                None => command.env_remove(key),
            };
        }
        if let Some(dir) = &self.current_dir {
            command.current_dir(dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
    }

    /// Spawns the process, outside any Job Object. The process is killed if the
    /// [`MuseProcess`] is dropped while it runs.
    pub fn spawn(&self) -> io::Result<MuseProcess> {
        let mut command = tokio::process::Command::from(self.std_command());
        command.kill_on_drop(true);
        Ok(MuseProcess::from_child(command.spawn()?))
    }
}

/// The last lines a process wrote to stderr. Muse logs there, and it must be read, or Muse
/// blocks once the pipe fills.
#[derive(Clone, Debug, Default)]
pub struct StderrTail {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl StderrTail {
    /// Reads `stderr` to the end on the current runtime, keeping the last lines and logging
    /// each at debug level.
    pub fn drain<R: AsyncRead + Unpin + Send + 'static>(stderr: R) -> (Self, JoinHandle<()>) {
        let tail = Self::default();
        let lines = tail.lines.clone();
        let task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                debug!(target: "burrow_msp::stderr", "{line}");
                let mut lines = lines.lock().unwrap_or_else(PoisonError::into_inner);
                if lines.len() == STDERR_LINES {
                    lines.pop_front();
                }
                lines.push_back(line);
            }
        });
        (tail, task)
    }

    /// The kept lines, joined with newlines.
    pub fn text(&self) -> String {
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        lines
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A running `muse serve`.
#[derive(Debug)]
pub struct MuseProcess {
    child: Child,
    stderr: StderrTail,
    stderr_task: Option<JoinHandle<()>>,
}

impl MuseProcess {
    /// Wraps a child spawned with piped stdio, for example from [`MuseCommand::std_command`]
    /// inside a Job Object. Starts draining its stderr. Needs a tokio runtime.
    pub fn from_child(mut child: Child) -> Self {
        let (stderr, stderr_task) = match child.stderr.take() {
            Some(stderr) => {
                let (tail, task) = StderrTail::drain::<ChildStderr>(stderr);
                (tail, Some(task))
            }
            None => (StderrTail::default(), None),
        };
        Self {
            child,
            stderr,
            stderr_task,
        }
    }

    /// Connects to the process's stdio and does the handshake. If Muse exits first, which it
    /// does when it can't build its provider (`--provider local` on Windows, for one), the
    /// error is [`MspError::Exited`] with Muse's own explanation from stderr.
    pub async fn connect(&mut self, options: ConnectOptions) -> Result<(MspClient, Events)> {
        let taken = || io::Error::other("muse's stdio was already taken");
        let stdin = self.child.stdin.take().ok_or_else(taken)?;
        let stdout = self.child.stdout.take().ok_or_else(taken)?;
        match MspClient::connect(stdout, stdin, options).await {
            Ok(connected) => Ok(connected),
            Err(error @ (MspError::Closed | MspError::Io(_))) => {
                match tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await {
                    Ok(Ok(status)) => {
                        self.finish_stderr().await;
                        Err(MspError::Exited {
                            status,
                            stderr: self.stderr.text(),
                        })
                    }
                    _ => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// The process id, while it runs.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// The last lines Muse wrote to stderr.
    pub fn stderr(&self) -> &StderrTail {
        &self.stderr
    }

    /// The child, for anything else.
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Waits for the process to exit.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.child.wait().await?;
        self.finish_stderr().await;
        Ok(status)
    }

    /// Kills the process and waits for it.
    pub async fn kill(&mut self) -> io::Result<()> {
        self.child.kill().await
    }

    /// Shuts down cleanly: closes Muse's stdin through `client`, waits up to `grace` for the
    /// process to exit (it exits 0), and kills it if it doesn't.
    pub async fn shutdown(mut self, client: &MspClient, grace: Duration) -> io::Result<ExitStatus> {
        let graceful = tokio::time::timeout(grace, async {
            client.shutdown().await;
            self.child.wait().await
        })
        .await;
        match graceful {
            Ok(status) => {
                self.finish_stderr().await;
                status
            }
            Err(_) => {
                debug!("muse didn't exit in time; killing it");
                self.child.kill().await?;
                self.wait().await
            }
        }
    }

    /// Lets the stderr reader catch the last lines after an exit.
    async fn finish_stderr(&mut self) {
        if let Some(task) = self.stderr_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launcher_resolves_to_its_versioned_binary() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = dir.path().join("muse.cmd");
        std::fs::write(&launcher, "@echo off\n").unwrap();
        assert_eq!(launcher_binary(&launcher), None, "no .muse-version yet");

        std::fs::write(dir.path().join(".muse-version"), "1.4.14-R7309.1\n").unwrap();
        assert_eq!(launcher_binary(&launcher), None, "no binary yet");

        let binary = dir.path().join(format!(
            "muse-bin-1.4.14-R7309.1{}",
            std::env::consts::EXE_SUFFIX
        ));
        std::fs::write(&binary, b"").unwrap();
        assert_eq!(launcher_binary(&launcher), Some(binary));
    }

    #[test]
    fn a_version_with_path_characters_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = dir.path().join("muse.cmd");
        std::fs::write(&launcher, "").unwrap();
        std::fs::write(dir.path().join(".muse-version"), "..\\evil").unwrap();
        assert_eq!(launcher_binary(&launcher), None);
    }

    #[test]
    fn the_command_line_is_serve_plus_options() {
        let command = MuseCommand::new("muse")
            .provider("echo")
            .model("muse-spark-1.3")
            .no_session_log()
            .config("settings.reasoning_effort", "\"low\"");
        let args: Vec<_> = command.args().iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "serve",
                "--provider",
                "echo",
                "--model",
                "muse-spark-1.3",
                "--no-session-log",
                "-c",
                "settings.reasoning_effort=\"low\"",
            ]
        );
    }
}
