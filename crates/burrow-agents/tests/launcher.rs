//! The launcher against real processes: `cmd.exe`, `ping` and batch files.

#![cfg(windows)]

use std::time::{Duration, Instant};

use burrow_agents::launcher::{
    self, LaunchSpec, descendants, find_program, is_running, run_capture,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const WAIT: Duration = Duration::from_secs(20);

fn cmd() -> std::path::PathBuf {
    find_program("cmd").expect("cmd.exe is on PATH")
}

/// Waits until `pid` has a child, and returns it.
async fn first_child(pid: u32) -> u32 {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(&child) = descendants(pid).first() {
            return child;
        }
        assert!(Instant::now() < deadline, "process {pid} started no child");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `cmd /c ping`: a parent with a long-running child.
fn tree_spec(label: &str) -> LaunchSpec {
    LaunchSpec::new(cmd(), ["/c", "ping -n 60 127.0.0.1 > nul"], label)
}

#[tokio::test]
async fn stopping_kills_the_whole_tree() {
    let process = launcher::spawn(&tree_spec("tree")).unwrap();
    let handle = process.handle();
    let parent = process.pid();
    let child = first_child(parent).await;
    assert!(is_running(parent) && is_running(child));
    // cmd.exe and ping, plus cmd.exe's hidden conhost.exe once Windows has started it (D-018).
    let active = handle.active_processes();
    assert!((2..=3).contains(&active), "{active} processes in the job");

    handle.stop(Duration::ZERO).await;
    assert!(!is_running(parent), "cmd.exe survived");
    assert!(!is_running(child), "ping survived");
    assert_eq!(handle.active_processes(), 0);
    assert!(
        handle.exit_info().is_some() || tokio::time::timeout(WAIT, handle.exited()).await.is_ok()
    );
}

#[tokio::test]
async fn dropping_every_handle_kills_the_tree() {
    let process = launcher::spawn(&tree_spec("drop")).unwrap();
    let parent = process.pid();
    let child = first_child(parent).await;
    drop(process);
    let deadline = Instant::now() + WAIT;
    while is_running(parent) || is_running(child) {
        assert!(Instant::now() < deadline, "the tree outlived its job");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn batch_files_get_their_arguments_quoted() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("show args.cmd");
    std::fs::write(&script, "@echo off\r\necho [%1] [%2] [%3]\r\n").unwrap();
    let spec = LaunchSpec::new(&script, ["hello world", "plain", "50%"], "args");
    let output = run_capture(&spec, WAIT).await.unwrap();
    assert!(output.exit.success(), "{output:?}");
    // `%1` keeps the quotes cmd.exe needed; the percent sign arrives literally, unexpanded.
    assert_eq!(output.stdout.trim(), r#"["hello world"] [plain] ["50%"]"#);
}

#[tokio::test]
async fn stdio_is_handed_over_and_stderr_is_kept() {
    // `findstr` echoes stdin lines that match; `1>&2` sends a line to stderr first.
    let spec = LaunchSpec::new(cmd(), ["/c", "echo to stderr 1>&2 && findstr x"], "stdio");
    let mut process = launcher::spawn(&spec).unwrap();
    let handle = process.handle();
    let mut stdin = process.take_stdin().unwrap();
    let mut stdout = BufReader::new(process.take_stdout().unwrap()).lines();
    stdin.write_all(b"xyz\r\n").await.unwrap();
    drop(stdin);
    let line = tokio::time::timeout(WAIT, stdout.next_line())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(line.as_deref(), Some("xyz"));
    let exit = tokio::time::timeout(WAIT, handle.exited()).await.unwrap();
    assert!(exit.success(), "{exit:?}");
    let deadline = Instant::now() + WAIT;
    while handle.stderr_tail().is_empty() {
        assert!(Instant::now() < deadline, "stderr wasn't read");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let tail = handle.stderr_tail();
    assert_eq!(tail.len(), 1, "{tail:?}");
    assert_eq!(tail[0].trim(), "to stderr");
}

#[tokio::test]
async fn a_command_that_runs_too_long_is_killed() {
    let spec = tree_spec("slow");
    let started = Instant::now();
    let error = run_capture(&spec, Duration::from_millis(500))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(started.elapsed() < WAIT);
}

#[tokio::test]
async fn a_missing_program_fails_to_start() {
    let spec = LaunchSpec::new(
        r"C:\definitely\not\here.exe",
        Vec::<String>::new(),
        "missing",
    );
    assert!(launcher::spawn(&spec).is_err());
}
