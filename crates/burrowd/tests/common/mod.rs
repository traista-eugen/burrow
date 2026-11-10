//! Helpers for tests that run the real `burrowd` binary.

#![allow(dead_code, reason = "each test file uses a different subset")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use burrow_client::{Client, ConnectError, ConnectOptions, Subscription};
use burrow_protocol::methods::DebugAppend;
use burrow_protocol::{
    ClientInfo, ClientKind, DebugAppendParams, EntityCreated, EntityKind, Event, StreamEvent,
    StreamName,
};
use tokio::runtime::Handle;

/// How long anything in these tests may take before it counts as hung.
pub const WAIT: Duration = Duration::from_secs(20);

/// A pipe name no other test, or test run, uses.
pub fn unique_pipe(test: &str) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!(
        "burrow-test-{}-{}-{test}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

pub fn burrowd() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_burrowd"));
    command
        .env("BURROW_LOG", "debug")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// A running `burrowd`, killed when dropped.
pub struct Daemon {
    child: Child,
    pub pipe: String,
    pub data_dir: PathBuf,
}

impl Daemon {
    /// Starts `burrowd` and waits until it accepts clients. With `dev`, it offers only the fake
    /// agent; without, no agents. Either way no real agent CLI is run.
    pub async fn start(data_dir: &Path, pipe: &str, dev: bool) -> Self {
        let args: &[&str] = if dev {
            &["--dev", "--agents=fake"]
        } else {
            &["--agents="]
        };
        Self::start_with(data_dir, pipe, args).await
    }

    /// Starts `burrowd` with `args` added, and waits until it accepts clients.
    pub async fn start_with(data_dir: &Path, pipe: &str, args: &[&str]) -> Self {
        let mut command = burrowd();
        command
            .arg("--pipe")
            .arg(pipe)
            .arg("--data-dir")
            .arg(data_dir)
            .args(args);
        let mut daemon = Self {
            child: command.spawn().expect("start burrowd"),
            pipe: pipe.to_owned(),
            data_dir: data_dir.to_owned(),
        };
        let deadline = Instant::now() + WAIT;
        loop {
            match Client::connect(options(pipe)).await {
                Ok(_) => return daemon,
                Err(ConnectError::NotRunning { .. } | ConnectError::Busy { .. }) => {}
                Err(error) => panic!("connecting to burrowd failed: {error}"),
            }
            if let Some(status) = daemon.child.try_wait().unwrap() {
                panic!("burrowd exited with {status}\n{}", daemon.logs());
            }
            assert!(
                Instant::now() < deadline,
                "burrowd didn't start\n{}",
                daemon.logs()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The `burrowd` process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Kills `burrowd` the hard way (TerminateProcess): no shutdown code runs.
    pub fn kill(&mut self) {
        self.child.kill().expect("kill burrowd");
        self.child.wait().expect("wait for burrowd");
    }

    /// Whether `burrowd` has exited, and how.
    pub fn exited(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().unwrap()
    }

    /// Everything `burrowd` has logged in this data dir, for failure messages.
    pub fn logs(&self) -> String {
        read_logs(&self.data_dir)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn read_logs(data_dir: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(data_dir.join("logs")) else {
        return "(no logs)".into();
    };
    let mut logs = String::new();
    for entry in entries.flatten() {
        logs.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
    }
    logs
}

/// Runs `burrowd` with `args` and waits for it to exit.
pub fn run_to_exit(args: &[&str]) -> Output {
    let mut command = burrowd();
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("start burrowd");
    let deadline = Instant::now() + WAIT;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("burrowd didn't exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

pub fn options(pipe: &str) -> ConnectOptions {
    ConnectOptions::new(ClientInfo {
        name: "burrowd-tests".into(),
        version: "0.0.0".into(),
        kind: ClientKind::Cli,
    })
    .pipe(pipe)
}

pub async fn connect(pipe: &str) -> Client {
    Client::connect(options(pipe)).await.expect("connect")
}

pub async fn connect_on(pipe: &str, runtime: Handle) -> Client {
    Client::connect(options(pipe).runtime(runtime))
        .await
        .expect("connect")
}

/// An `entityCreated` event for session `ses_<n>`, so each event is recognizable.
pub fn event(n: u64) -> Event {
    Event::EntityCreated(EntityCreated {
        entity: EntityKind::Session,
        id: format!("ses_{n}"),
        title: Some(format!("event {n}")),
        project_id: None,
        state: None,
        agent: None,
        mode: None,
    })
}

/// Which `event(n)` this is.
pub fn event_number(event: &StreamEvent) -> u64 {
    let Event::EntityCreated(created) = &event.event else {
        panic!("unexpected event {:?}", event.event);
    };
    created.id.strip_prefix("ses_").unwrap().parse().unwrap()
}

/// Appends `event(n)` to `stream` and returns its seq.
pub async fn append(client: &Client, stream: &StreamName, n: u64) -> u64 {
    client
        .call::<DebugAppend>(&DebugAppendParams {
            stream: stream.clone(),
            event: event(n),
        })
        .await
        .expect("debug/append")
        .seq
}

/// The next `count` events of a subscription.
pub async fn take(subscription: &mut Subscription, count: usize) -> Vec<StreamEvent> {
    let mut events = Vec::with_capacity(count);
    tokio::time::timeout(WAIT, async {
        while events.len() < count {
            let event = subscription.next().await.expect("the subscription ended");
            events.push(event);
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out after {} of {count} events", events.len()));
    events
}

/// Asserts no further event arrives for a moment.
pub async fn assert_quiet(subscription: &mut Subscription) {
    let extra = tokio::time::timeout(Duration::from_millis(300), subscription.next()).await;
    assert!(extra.is_err(), "unexpected extra event: {extra:?}");
}

/// The seqs of `events`.
pub fn seqs(events: &[StreamEvent]) -> Vec<u64> {
    events.iter().map(|event| event.seq).collect()
}
