//! The process launcher: every agent process runs inside its own Job Object (D-007, D-018).
//!
//! [`spawn`] starts a program with piped stdin, stdout and stderr, and no console window. The
//! process is created suspended, put into a new Job Object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and only then resumed, so nothing it starts can escape
//! the job. Closing the job's handle kills the whole tree: npx, node, the agent, its MCP servers.
//! That happens when the [`AgentProcess`] and every [`ProcessHandle`] are dropped, on
//! [`ProcessHandle::kill`], and when `burrowd` itself dies, because Windows closes its handles.
//!
//! The launcher doesn't speak any protocol. It hands the child's stdin and stdout to whoever does
//! ([`AgentProcess::take_stdin`], [`AgentProcess::take_stdout`]), for example an ACP or MSP
//! client. Stderr is read here: each line goes to `tracing` (target `burrow_agents::stderr`) and
//! the last lines are kept for error messages ([`ProcessHandle::stderr_tail`]).
//!
//! Programs are found with [`find_program`], which knows Windows' shims: npm's `claude.cmd`, or
//! Cursor's `agent.cmd`. Batch files run through `cmd.exe`; the standard library quotes their
//! arguments for `cmd.exe` and refuses arguments it can't pass safely.

mod resolve;
#[cfg(windows)]
mod windows;

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::watch;
use tracing::{debug, warn};

pub use resolve::{find_program, find_program_in};

/// Lines of stderr kept for error messages.
const STDERR_TAIL: usize = 20;
/// How long [`ProcessHandle::stop`] waits for the job to empty after killing it.
const KILL_WAIT: Duration = Duration::from_secs(10);

/// What to start.
#[derive(Clone, Debug)]
pub struct LaunchSpec {
    /// The program: an absolute path, usually from [`find_program`]. `.cmd` and `.bat` files are
    /// run through `cmd.exe`.
    pub program: PathBuf,
    /// The arguments.
    pub args: Vec<OsString>,
    /// The working directory; `burrowd`'s own when absent.
    pub cwd: Option<PathBuf>,
    /// Environment variables to set, on top of `burrowd`'s environment.
    pub env: Vec<(OsString, OsString)>,
    /// Names the process in logs, for example `claude ses_01…`.
    pub label: String,
    /// Give the child a null stdin instead of a pipe, for commands that read nothing.
    pub null_stdin: bool,
}

impl LaunchSpec {
    /// Runs `program` with `args`, labelled `label`.
    pub fn new<I, S>(program: impl Into<PathBuf>, args: I, label: impl Into<String>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            cwd: None,
            env: Vec::new(),
            label: label.into(),
            null_stdin: false,
        }
    }

    /// Sets the working directory.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Sets an environment variable.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// A running process tree, with its stdio not yet handed over.
#[derive(Debug)]
pub struct AgentProcess {
    handle: ProcessHandle,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
}

impl AgentProcess {
    /// The main process's id.
    pub fn pid(&self) -> u32 {
        self.handle.pid
    }

    /// The child's stdin, for the protocol client. Dropping it closes the pipe, which tells most
    /// agents to exit.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.stdin.take()
    }

    /// The child's stdout, for the protocol client.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }

    /// A handle to stop the tree and watch for exit. The tree lives until this process value and
    /// every handle are dropped.
    pub fn handle(&self) -> ProcessHandle {
        self.handle.clone()
    }
}

/// Stops a process tree and watches its main process. Cheap to clone. When the last handle and
/// the [`AgentProcess`] are gone, the job closes and the tree dies.
#[derive(Clone, Debug)]
pub struct ProcessHandle {
    pid: u32,
    label: Arc<str>,
    #[cfg(windows)]
    job: Arc<windows::Job>,
    exit: watch::Receiver<Option<ExitInfo>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

/// How the main process ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitInfo {
    /// Its exit code, if it has one.
    pub code: Option<i32>,
    /// A readable description, for example `exit code 1`.
    pub description: String,
}

impl ExitInfo {
    fn from_status(status: io::Result<ExitStatus>) -> Self {
        match status {
            Ok(status) => Self {
                code: status.code(),
                description: match status.code() {
                    Some(code) => format!("exit code {code}"),
                    None => status.to_string(),
                },
            },
            Err(error) => Self {
                code: None,
                description: format!("waiting for it failed: {error}"),
            },
        }
    }

    /// Whether it exited with code 0.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

impl ProcessHandle {
    /// The main process's id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// How the main process ended, if it has.
    pub fn exit_info(&self) -> Option<ExitInfo> {
        self.exit.borrow().clone()
    }

    /// Waits for the main process to exit. Other processes in the tree may still run.
    pub async fn exited(&self) -> ExitInfo {
        let mut exit = self.exit.clone();
        match exit.wait_for(Option::is_some).await {
            Ok(info) => info.clone().expect("waited for Some"),
            // The waiter is gone without a status; report what we can.
            Err(_) => ExitInfo {
                code: None,
                description: "its exit status is unknown".into(),
            },
        }
    }

    /// The last lines the process wrote to stderr, oldest first.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// How many processes of the tree are still running.
    pub fn active_processes(&self) -> u32 {
        #[cfg(windows)]
        {
            self.job.active_processes().unwrap_or(0)
        }
        #[cfg(not(windows))]
        {
            0
        }
    }

    /// Kills every process in the tree now, without waiting.
    pub fn kill(&self) {
        #[cfg(windows)]
        if let Err(error) = self.job.terminate() {
            warn!(process = %self.label, %error, "terminating the job failed");
        }
    }

    /// Stops the tree: waits up to `grace` for the main process to exit by itself (close its
    /// stdin first to ask it to), then kills whatever is left and waits until every process of
    /// the tree is gone.
    pub async fn stop(&self, grace: Duration) {
        if !grace.is_zero() && self.exit_info().is_none() {
            let _ = tokio::time::timeout(grace, self.exited()).await;
        }
        self.kill();
        let gone = tokio::time::timeout(KILL_WAIT, async {
            while self.active_processes() > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if gone.is_err() {
            warn!(process = %self.label, "processes are still running after the job was killed");
        } else {
            debug!(process = %self.label, "stopped");
        }
    }
}

/// Starts `spec` in a new Job Object, suspended until it is in the job, with no console window.
///
/// Must be called on a tokio runtime: tasks read stderr and wait for the exit.
pub fn spawn(spec: &LaunchSpec) -> io::Result<AgentProcess> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .stdin(if spec.null_stdin {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }

    #[cfg(windows)]
    let (mut child, job) = windows::spawn_in_job(command)?;
    #[cfg(not(windows))]
    let mut child: tokio::process::Child = {
        let _ = &mut command;
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "starting agents is Windows-only for now",
        ));
    };

    let pid = child.id().unwrap_or_default();
    let label: Arc<str> = spec.label.as_str().into();
    debug!(process = %label, pid, program = %spec.program.display(), "started");

    let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL)));
    if let Some(pipe) = child.stderr.take() {
        tokio::spawn(read_stderr(pipe, label.clone(), stderr.clone()));
    }
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();

    let (exit_sender, exit) = watch::channel(None);
    let waiter_label = label.clone();
    tokio::spawn(async move {
        let info = ExitInfo::from_status(child.wait().await);
        debug!(process = %waiter_label, exit = %info.description, "main process exited");
        exit_sender.send_replace(Some(info));
    });

    Ok(AgentProcess {
        handle: ProcessHandle {
            pid,
            label,
            #[cfg(windows)]
            job: Arc::new(job),
            exit,
            stderr,
        },
        stdin,
        stdout,
    })
}

async fn read_stderr(pipe: ChildStderr, label: Arc<str>, tail: Arc<Mutex<VecDeque<String>>>) {
    let mut lines = BufReader::new(pipe).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                debug!(target: "burrow_agents::stderr", process = %label, "{line}");
                let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
                if tail.len() == STDERR_TAIL {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            Ok(None) => break,
            // Not UTF-8, or the pipe broke: stop reading; the process carries on.
            Err(error) => {
                debug!(process = %label, %error, "stopped reading stderr");
                break;
            }
        }
    }
}

/// Runs a short command to completion and captures its output, for detection. The command runs
/// in its own job like any agent process, so a timeout kills everything it started.
pub async fn run_capture(spec: &LaunchSpec, timeout: Duration) -> io::Result<Captured> {
    use tokio::io::AsyncReadExt;

    /// Output beyond this is dropped.
    const MAX_OUTPUT: u64 = 1024 * 1024;

    let mut spec = spec.clone();
    spec.null_stdin = true;
    let mut process = spawn(&spec)?;
    let handle = process.handle();
    let mut stdout = process.take_stdout().expect("stdout is piped");
    let read = async {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(MAX_OUTPUT)
            .read_to_end(&mut bytes)
            .await?;
        let exit = handle.exited().await;
        io::Result::Ok((bytes, exit))
    };
    let result = tokio::time::timeout(timeout, read).await;
    // Whatever happened, nothing of the command may outlive this call.
    handle.kill();
    match result {
        Ok(Ok((stdout, exit))) => Ok(Captured {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: handle.stderr_tail().join("\n"),
            exit,
        }),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{} didn't finish within {timeout:?}", spec.label),
        )),
    }
}

/// What a command printed.
#[derive(Clone, Debug)]
pub struct Captured {
    /// Everything on stdout, lossily decoded.
    pub stdout: String,
    /// The last lines of stderr.
    pub stderr: String,
    /// How it exited.
    pub exit: ExitInfo,
}

impl Captured {
    /// Stdout, then stderr: some CLIs print status to either.
    pub fn text(&self) -> String {
        match (self.stdout.trim().is_empty(), self.stderr.trim().is_empty()) {
            (false, false) => format!("{}\n{}", self.stdout, self.stderr),
            (false, true) => self.stdout.clone(),
            _ => self.stderr.clone(),
        }
    }
}

/// Every process that `pid` started, directly or not, that is still running. For tests and
/// diagnostics; it walks a snapshot of all processes by parent id.
pub fn descendants(pid: u32) -> Vec<u32> {
    #[cfg(windows)]
    {
        windows::descendants(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        Vec::new()
    }
}

/// Whether a process with this id is running.
pub fn is_running(pid: u32) -> bool {
    #[cfg(windows)]
    {
        windows::is_running(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        false
    }
}
