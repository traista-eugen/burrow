//! Tests of the public API.

use std::collections::BTreeMap;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use burrow_store::{Appended, Error, NewEvent, SCHEMA_VERSION, Seq, Store};
use serde::Deserialize;
use serde_json::json;

fn event(event_type: &str, n: u64) -> NewEvent {
    NewEvent::json(event_type, &json!({ "n": n })).unwrap()
}

fn seqs(store: &Store, stream: &str, after: Seq, limit: usize) -> Vec<Seq> {
    store
        .read(stream, after, limit)
        .unwrap()
        .iter()
        .map(|event| event.seq)
        .collect()
}

fn now_ms() -> i64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    i64::try_from(since.as_millis()).unwrap()
}

#[test]
fn seqs_start_at_one_and_count_per_stream() {
    let mut store = Store::open_in_memory().unwrap();
    assert_eq!(store.head("index").unwrap(), 0);

    let a = store
        .append("index", &[event("a", 1), event("b", 2)])
        .unwrap();
    let b = store.append("session:ses_1", &[event("c", 3)]).unwrap();
    let c = store
        .append("index", &[event("d", 4), event("e", 5), event("f", 6)])
        .unwrap();

    assert_eq!(a.seqs, 1..=2);
    assert_eq!(b.seqs, 1..=1);
    assert_eq!(c.seqs, 3..=5);
    assert_eq!(store.head("index").unwrap(), 5);
    assert_eq!(store.head("session:ses_1").unwrap(), 1);
    assert_eq!(store.head("run:unknown").unwrap(), 0);
    assert_eq!(seqs(&store, "index", 0, usize::MAX), [1, 2, 3, 4, 5]);
}

#[test]
fn read_respects_after_and_limit() {
    let mut store = Store::open_in_memory().unwrap();
    let events: Vec<_> = (1..=10).map(|n| event("tick", n)).collect();
    store.append("index", &events).unwrap();

    assert_eq!(
        seqs(&store, "index", 0, usize::MAX),
        (1..=10).collect::<Vec<_>>()
    );
    assert_eq!(seqs(&store, "index", 3, 4), [4, 5, 6, 7]);
    assert_eq!(seqs(&store, "index", 8, 100), [9, 10]);
    assert_eq!(seqs(&store, "index", 0, 1), [1]);
    assert!(seqs(&store, "index", 0, 0).is_empty());
    assert!(seqs(&store, "index", 10, 100).is_empty());
    assert!(seqs(&store, "index", 99, 100).is_empty());
    assert!(seqs(&store, "index", Seq::MAX, 100).is_empty());
    assert!(seqs(&store, "session:none", 0, 100).is_empty());

    // Paging with the last seen seq visits every event once, in order.
    let mut seen = Vec::new();
    let mut after = 0;
    loop {
        let page = store.read("index", after, 3).unwrap();
        let Some(last) = page.last() else { break };
        after = last.seq;
        seen.extend(
            page.iter()
                .map(|event| event.payload::<serde_json::Value>().unwrap()["n"].clone()),
        );
    }
    assert_eq!(seen, (1..=10).map(|n| json!(n)).collect::<Vec<_>>());
}

#[test]
fn events_keep_their_type_payload_and_time() {
    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Renamed {
        session_id: String,
        title: String,
    }

    let mut store = Store::open_in_memory().unwrap();
    // Key order, spacing and non-ASCII text are kept exactly.
    let raw = r#"{"title":"Réparer «login» 🦀",  "sessionId":"ses_1"}"#;
    let payload = serde_json::value::RawValue::from_string(raw.to_owned()).unwrap();

    let before = now_ms();
    let appended = store
        .append("index", &[NewEvent::new("sessionRenamed", payload)])
        .unwrap();
    let after = now_ms();
    assert!((before..=after).contains(&appended.at_ms));

    let [event] = &store.read("index", 0, 10).unwrap()[..] else {
        panic!("expected one event");
    };
    assert_eq!(event.seq, 1);
    assert_eq!(event.event_type, "sessionRenamed");
    assert_eq!(event.payload.get(), raw);
    assert_eq!(event.at_ms, appended.at_ms);
    assert_eq!(
        event.payload::<Renamed>().unwrap(),
        Renamed {
            session_id: "ses_1".into(),
            title: "Réparer «login» 🦀".into(),
        }
    );
    assert!(matches!(event.payload::<u32>(), Err(Error::Json(_))));
}

#[test]
fn invalid_appends_are_rejected_and_store_nothing() {
    let mut store = Store::open_in_memory().unwrap();

    assert!(matches!(
        store.append("index", &[]),
        Err(Error::EmptyAppend)
    ));
    assert!(matches!(
        store.append("", &[event("a", 1)]),
        Err(Error::EmptyStream)
    ));
    assert!(matches!(
        store.append("index", &[event("a", 1), event("", 2)]),
        Err(Error::EmptyEventType)
    ));

    assert_eq!(store.head("index").unwrap(), 0);
    assert_eq!(store.append("index", &[event("a", 1)]).unwrap().seqs, 1..=1);
}

#[test]
fn concurrent_appends_from_many_connections_stay_gap_free() {
    const THREADS: u64 = 8;
    const APPENDS: u64 = 25;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("burrow.db");
    // Create and migrate the database once, as burrowd would at startup.
    drop(Store::open(&path).unwrap());

    let start = Arc::new(Barrier::new(THREADS as usize));
    let workers: Vec<_> = (0..THREADS)
        .map(|thread| {
            let path = path.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                // Each thread has its own connection, as separate processes would.
                let mut store = Store::open(&path).unwrap();
                start.wait();
                let mut appended: Vec<(u64, Appended)> = Vec::new();
                for append in 0..APPENDS {
                    // 1 to 3 events per append, so batches interleave.
                    let count = append % 3 + 1;
                    let events: Vec<_> = (0..count)
                        .map(|i| {
                            NewEvent::json(
                                "tick",
                                &json!({ "thread": thread, "append": append, "i": i }),
                            )
                            .unwrap()
                        })
                        .collect();
                    appended.push((count, store.append("index", &events).unwrap()));
                    // A private stream per thread counts independently.
                    store
                        .append(&format!("run:{thread}"), &[event("own", append)])
                        .unwrap();
                }
                (thread, appended)
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();

    // The returned ranges tile 1..=total exactly: no gaps, no overlaps.
    let mut ranges: Vec<_> = results
        .iter()
        .flat_map(|(_, appended)| appended.iter())
        .map(|(count, appended)| {
            assert_eq!(appended.seqs.end() - appended.seqs.start() + 1, *count);
            appended.seqs.clone()
        })
        .collect();
    ranges.sort_by_key(|range| *range.start());
    let mut next = 1;
    for range in &ranges {
        assert_eq!(*range.start(), next, "gap or overlap before {range:?}");
        next = range.end() + 1;
    }
    let total = next - 1;

    let store = Store::open(&path).unwrap();
    assert_eq!(store.head("index").unwrap(), total);
    let events = store.read("index", 0, usize::MAX).unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=total).collect::<Vec<_>>()
    );

    // Each stored event sits at the seq its append returned, and each
    // append's events are contiguous and in the order they were passed.
    let returned: BTreeMap<(u64, u64), Seq> = results
        .iter()
        .flat_map(|(thread, appended)| {
            appended
                .iter()
                .enumerate()
                .map(move |(append, (_, appended))| {
                    ((*thread, append as u64), *appended.seqs.start())
                })
        })
        .collect();
    for event in &events {
        let payload: serde_json::Value = event.payload().unwrap();
        let key = (
            payload["thread"].as_u64().unwrap(),
            payload["append"].as_u64().unwrap(),
        );
        assert_eq!(event.seq, returned[&key] + payload["i"].as_u64().unwrap());
    }

    for thread in 0..THREADS {
        assert_eq!(
            seqs(&store, &format!("run:{thread}"), 0, usize::MAX),
            (1..=APPENDS).collect::<Vec<_>>()
        );
    }
}

#[test]
fn store_can_move_to_another_thread() {
    let mut store = Store::open_in_memory().unwrap();
    store.append("index", &[event("a", 1)]).unwrap();
    let store = thread::spawn(move || {
        store.append("index", &[event("b", 2)]).unwrap();
        store
    })
    .join()
    .unwrap();
    assert_eq!(store.head("index").unwrap(), 2);
}

#[test]
fn data_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("burrow.db");

    {
        let mut store = Store::open(&path).unwrap();
        store
            .append("index", &[event("a", 1), event("b", 2)])
            .unwrap();
        store.append("session:ses_1", &[event("c", 3)]).unwrap();
    }

    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.head("index").unwrap(), 2);
    assert_eq!(store.head("session:ses_1").unwrap(), 1);
    let types: Vec<_> = store
        .read("index", 0, usize::MAX)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .collect();
    assert_eq!(types, ["a", "b"]);

    // Appending carries on from the stored head.
    assert_eq!(store.append("index", &[event("d", 4)]).unwrap().seqs, 3..=3);
}

#[test]
fn migrations_run_once_and_reopening_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("burrow.db");

    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    store.append("index", &[event("a", 1)]).unwrap();
    drop(store);

    // Re-running migration 1 would fail (its CREATE TABLE has no IF NOT
    // EXISTS), so these opens prove it ran once.
    for reopen in 0..3 {
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(store.head("index").unwrap(), 1 + reopen);
        store.append("index", &[event("b", 2)]).unwrap();
    }
}

/// Switching a new file to WAL mode ignores SQLite's busy timeout, so racing
/// opens used to fail with "database is locked".
#[test]
fn several_stores_can_open_a_new_file_at_once() {
    const OPENERS: usize = 8;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("burrow.db");
    let start = Arc::new(Barrier::new(OPENERS));

    let openers: Vec<_> = (0..OPENERS)
        .map(|n| {
            let path = path.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                let mut store = Store::open(&path).unwrap();
                assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
                store.append("index", &[event("opened", n as u64)]).unwrap();
            })
        })
        .collect();
    for opener in openers {
        opener.join().unwrap();
    }

    let store = Store::open(&path).unwrap();
    assert_eq!(
        seqs(&store, "index", 0, usize::MAX),
        (1..=OPENERS as u64).collect::<Vec<_>>()
    );
}
