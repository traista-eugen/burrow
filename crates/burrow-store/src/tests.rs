//! Tests that need the raw SQLite connection. Tests of the public API live in
//! `tests/`.

use serde_json::json;

use super::*;

fn event(event_type: &str, n: u32) -> NewEvent {
    NewEvent::json(event_type, &json!({ "n": n })).unwrap()
}

/// Makes any insert of an event with type `boom` fail inside SQLite, after the
/// events before it in the same append were inserted.
fn install_failing_trigger(store: &Store) {
    store
        .conn
        .execute_batch(
            "CREATE TEMP TRIGGER boom BEFORE INSERT ON events WHEN NEW.type = 'boom'
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .unwrap();
}

#[test]
fn multi_event_append_is_all_or_nothing() {
    let mut store = Store::open_in_memory().unwrap();
    install_failing_trigger(&store);

    // Fails on the third of four events: the first two must not survive.
    let err = store
        .append(
            "index",
            &[
                event("a", 1),
                event("b", 2),
                event("boom", 3),
                event("c", 4),
            ],
        )
        .unwrap_err();
    assert!(matches!(err, Error::Sqlite(_)), "{err:?}");
    assert_eq!(store.head("index").unwrap(), 0);
    assert!(store.read("index", 0, usize::MAX).unwrap().is_empty());

    // The failed append leaves no hole: the next one starts at 1.
    store
        .append("index", &[event("a", 1), event("b", 2)])
        .unwrap();
    let failed = store.append("index", &[event("c", 3), event("boom", 4)]);
    assert!(failed.is_err());
    let appended = store.append("index", &[event("d", 5)]).unwrap();
    assert_eq!(appended.seqs, 3..=3);

    let types: Vec<_> = store
        .read("index", 0, usize::MAX)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .collect();
    assert_eq!(types, ["a", "b", "d"]);
}

#[test]
fn unique_constraint_rejects_a_duplicate_seq() {
    let mut store = Store::open_in_memory().unwrap();
    store.append("index", &[event("a", 1)]).unwrap();

    let duplicate = store.conn.execute(
        "INSERT INTO events (stream, seq, type, payload, at) VALUES ('index', 1, 'x', '{}', 0)",
        [],
    );
    let Err(rusqlite::Error::SqliteFailure(failure, _)) = duplicate else {
        panic!("expected a constraint violation, got {duplicate:?}");
    };
    assert_eq!(failure.code, rusqlite::ErrorCode::ConstraintViolation);

    // The same seq in another stream is fine.
    store
        .conn
        .execute(
            "INSERT INTO events (stream, seq, type, payload, at) VALUES ('run:r1', 1, 'x', '{}', 0)",
            [],
        )
        .unwrap();
}

#[test]
fn file_database_uses_wal_full_sync_and_foreign_keys() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("burrow.db")).unwrap();

    let pragma = |name: &str| -> String {
        store
            .conn
            .pragma_query_value(None, name, |row| row.get::<_, rusqlite::types::Value>(0))
            .map(|value| match value {
                rusqlite::types::Value::Integer(n) => n.to_string(),
                rusqlite::types::Value::Text(text) => text,
                other => format!("{other:?}"),
            })
            .unwrap()
    };
    assert_eq!(pragma("journal_mode"), "wal");
    assert_eq!(pragma("synchronous"), "2", "2 is FULL");
    assert_eq!(pragma("foreign_keys"), "1");
    assert_eq!(pragma("busy_timeout"), BUSY_TIMEOUT.as_millis().to_string());
}

#[test]
fn applying_migrations_again_changes_nothing() {
    let mut store = Store::open_in_memory().unwrap();
    store.append("index", &[event("a", 1)]).unwrap();

    migrations::apply(&mut store.conn).unwrap();
    migrations::apply(&mut store.conn).unwrap();

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(store.head("index").unwrap(), 1);
}

#[test]
fn refuses_a_database_from_a_newer_build() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("burrow.db");
    {
        let store = Store::open(&path).unwrap();
        store
            .conn
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
    }

    let err = Store::open(&path).unwrap_err();
    assert!(
        matches!(
            err,
            Error::SchemaTooNew { found, supported }
                if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
        ),
        "{err:?}"
    );
}

#[test]
fn schema_rejects_bad_rows() {
    let store = Store::open_in_memory().unwrap();
    for values in [
        "('', 1, 'x', '{}', 0)",          // empty stream
        "('index', 0, 'x', '{}', 0)",     // seq below 1
        "('index', 1, '', '{}', 0)",      // empty type
        "('index', 'one', 'x', '{}', 0)", // STRICT: seq must be an integer
        "('index', 1, 'x', NULL, 0)",     // payload required
    ] {
        let sql = format!("INSERT INTO events (stream, seq, type, payload, at) VALUES {values}");
        assert!(store.conn.execute(&sql, []).is_err(), "accepted {values}");
    }
}
