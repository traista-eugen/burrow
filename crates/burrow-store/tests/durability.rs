//! Events must survive the process being killed without warning.
//!
//! The test runs this test binary again as a child process. The child appends
//! events, reports that `append` returned, and then hangs with the database
//! still open. The parent kills it (`TerminateProcess` on Windows), reopens the
//! database and checks that every reported event is there.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use burrow_store::{NewEvent, Store};
use serde_json::json;

const TEST_NAME: &str = "events_survive_the_process_being_killed";
const CHILD_DB_ENV: &str = "BURROW_STORE_TEST_CHILD_DB";
const READY: &str = "burrow-store test child: appends returned";

const INDEX_APPENDS: u64 = 20;
const RUN_EVENTS: u64 = 3;

#[test]
fn events_survive_the_process_being_killed() {
    if let Some(path) = std::env::var_os(CHILD_DB_ENV) {
        child(Path::new(&path));
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("burrow.db");

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([TEST_NAME, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_DB_ENV, &path)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    // Wait for the child's report, with a timeout in case it hangs.
    let stdout = child.stdout.take().unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    thread::spawn(move || {
        // libtest prints "test <name> ... " without a newline before running
        // the test, so the marker shares a line with it.
        for line in BufReader::new(stdout).lines() {
            if line.is_ok_and(|line| line.contains(READY)) {
                let _ = ready_tx.send(());
            }
        }
    });
    let ready = ready_rx.recv_timeout(Duration::from_secs(60));

    child.kill().unwrap();
    child.wait().unwrap();
    ready.expect("the child process didn't report its appends");

    // The child never closed its connection, so its commits are still in the
    // WAL, not yet checkpointed into the main file. Reopening must recover them.
    let wal = dir.path().join("burrow.db-wal");
    assert!(
        wal.metadata().is_ok_and(|meta| meta.len() > 0),
        "expected a non-empty WAL file left behind by the killed process"
    );

    let store = Store::open(&path).unwrap();
    let index = store.read("index", 0, usize::MAX).unwrap();
    assert_eq!(index.len() as u64, INDEX_APPENDS);
    for (n, event) in (0..).zip(&index) {
        assert_eq!(event.seq, n + 1);
        assert_eq!(
            event.payload::<serde_json::Value>().unwrap(),
            json!({ "n": n })
        );
    }
    assert_eq!(store.head("run:r1").unwrap(), RUN_EVENTS);
}

fn child(path: &Path) -> ! {
    let mut store = Store::open(path).unwrap();
    for n in 0..INDEX_APPENDS {
        store
            .append(
                "index",
                &[NewEvent::json("tick", &json!({ "n": n })).unwrap()],
            )
            .unwrap();
    }
    let run: Vec<_> = (0..RUN_EVENTS)
        .map(|n| NewEvent::json("step", &json!({ "n": n })).unwrap())
        .collect();
    store.append("run:r1", &run).unwrap();

    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{READY}").unwrap();
    stdout.flush().unwrap();
    drop(stdout);

    // Keep the store open until the parent kills us.
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
