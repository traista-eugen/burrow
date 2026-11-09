//! The Burrow event store: append-only event streams in SQLite (D-005).
//!
//! Every state change in `burrowd` is recorded as an event in a **stream**,
//! named `index`, `session:<id>` or `run:<id>`. Each stream numbers its events
//! with a `seq` that starts at 1 and has no gaps, so a client that reconnects
//! can ask for "everything after N" and see each event exactly once, in order.
//!
//! The store deals in plain records: a stream name, an event type and a JSON
//! payload. It doesn't depend on `burrow-protocol`; mapping records to wire
//! types is `burrowd`'s job. So is notifying live subscribers.
//!
//! ```
//! use burrow_store::{NewEvent, Store};
//! use serde_json::json;
//!
//! let mut store = Store::open_in_memory()?;
//! let appended = store.append(
//!     "index",
//!     &[
//!         NewEvent::json("sessionCreated", &json!({ "id": "ses_1" }))?,
//!         NewEvent::json("sessionRenamed", &json!({ "id": "ses_1", "title": "Fix login" }))?,
//!     ],
//! )?;
//! assert_eq!(appended.seqs, 1..=2);
//! assert_eq!(store.head("index")?, 2);
//!
//! // Everything after seq 1, at most 100 events.
//! let events = store.read("index", 1, 100)?;
//! assert_eq!(events.len(), 1);
//! assert_eq!(events[0].event_type, "sessionRenamed");
//! assert_eq!(events[0].payload.get(), r#"{"id":"ses_1","title":"Fix login"}"#);
//! # Ok::<(), burrow_store::Error>(())
//! ```
//!
//! # Guarantees
//!
//! - **Gap-free, per stream.** [`Store::append`] reads the stream's head and
//!   inserts in one `BEGIN IMMEDIATE` transaction, which takes SQLite's write
//!   lock before reading. Appends from several threads or processes, each with
//!   its own `Store` on the same file, are serialized. A
//!   `UNIQUE (stream, seq)` constraint is the backstop.
//! - **All or nothing.** A multi-event append commits every event or none.
//! - **Durable.** The database runs in WAL mode with `synchronous = FULL`.
//!   Once `append` returns, its events survive the process being killed and,
//!   as long as the disk honours flushes, the machine losing power (D-012).
//! - **Migrated on open.** Schema changes are numbered SQL migrations embedded
//!   in the binary. They're tracked with `PRAGMA user_version` and applied by
//!   [`Store::open`]. A database from a newer build is refused rather than
//!   misread.
//!
//! Payloads are stored as the exact JSON text given ([`RawValue`]), so a
//! server can put them into a notification without parsing them again.
//! Timestamps are wall-clock Unix milliseconds and may go backwards if the
//! system clock does; order events by `seq`, never by time.
//!
//! # Using the store from async code
//!
//! The store is synchronous. An append can wait for another connection's write
//! lock and always waits for an fsync, which takes milliseconds. So don't call
//! it on a tokio worker thread. `Store` is [`Send`] but not [`Sync`], because
//! it owns one SQLite connection. Move it to a thread where blocking is fine:
//!
//! - **A dedicated store thread.** This is the pattern meant for `burrowd`.
//!   A `std::thread` owns the `Store` and serves commands from a
//!   `tokio::sync::mpsc` channel (`blocking_recv`), answering each on a
//!   `tokio::sync::oneshot`. With a single writer, appends never wait for the
//!   lock. The thread is also the natural place to hand new events to live
//!   subscribers, right after each commit and in seq order.
//! - **`spawn_blocking`.** Keep the store in an `Arc<std::sync::Mutex<Store>>`
//!   and lock it inside `tokio::task::spawn_blocking`. Never hold the guard
//!   across an `.await`.
//!
//! In WAL mode, readers don't wait for writers. A second `Store` opened on the
//! same file can serve replays with [`Store::read`] while the first one
//! appends.
//!
//! A subscriber that replays history and then switches to live events may see
//! an event in both. It can drop any event whose `seq` it has already
//! delivered. Because seqs are gap-free, a jump of more than one means an
//! event was missed.
//!
//! ```ignore
//! // Sketch of the dedicated-thread pattern.
//! enum Command {
//!     Append { stream: String, events: Vec<NewEvent>, reply: oneshot::Sender<Result<Appended>> },
//!     Read { stream: String, after: Seq, limit: usize, reply: oneshot::Sender<Result<Vec<Event>>> },
//! }
//!
//! let (commands, mut inbox) = tokio::sync::mpsc::channel::<Command>(256);
//! std::thread::spawn(move || {
//!     let mut store = Store::open(&db_path).expect("open the event store");
//!     while let Some(command) = inbox.blocking_recv() {
//!         match command {
//!             Command::Append { stream, events, reply } => {
//!                 let _ = reply.send(store.append(&stream, &events));
//!             }
//!             Command::Read { stream, after, limit, reply } => {
//!                 let _ = reply.send(store.read(&stream, after, limit));
//!             }
//!         }
//!     }
//! });
//! ```

mod error;
mod migrations;

#[cfg(test)]
mod tests;

use std::ops::RangeInclusive;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

pub use error::{Error, Result};
pub use migrations::SCHEMA_VERSION;

/// A position in a stream. The first event has seq 1; seq 0 means "before the
/// first event", so `read(stream, 0, ..)` starts at the beginning.
pub type Seq = u64;

/// How long a connection waits for another connection's write lock before
/// giving up with `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// An event to append. The store assigns its seq and timestamp.
#[derive(Debug, Clone)]
pub struct NewEvent {
    /// The event type, for example `sessionCreated`. Must not be empty.
    pub event_type: String,
    /// The payload, stored exactly as given.
    pub payload: Box<RawValue>,
}

impl NewEvent {
    /// An event with a payload that is already JSON.
    pub fn new(event_type: impl Into<String>, payload: Box<RawValue>) -> Self {
        Self {
            event_type: event_type.into(),
            payload,
        }
    }

    /// An event whose payload is `payload` serialized to JSON.
    ///
    /// Works for typed values and for [`serde_json::Value`].
    pub fn json<T: Serialize + ?Sized>(event_type: impl Into<String>, payload: &T) -> Result<Self> {
        Ok(Self::new(
            event_type,
            serde_json::value::to_raw_value(payload)?,
        ))
    }
}

/// An event read from a stream.
#[derive(Debug, Clone)]
pub struct Event {
    /// Position in the stream, from 1.
    pub seq: Seq,
    /// The event type.
    pub event_type: String,
    /// The payload, exactly as it was appended.
    pub payload: Box<RawValue>,
    /// When the event was appended, in Unix milliseconds (wall clock).
    pub at_ms: i64,
}

impl Event {
    /// Deserializes the payload into `T`.
    pub fn payload<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_str(self.payload.get())?)
    }
}

/// The result of an append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appended {
    /// The seqs given to the events, in the order they were passed. The last
    /// one is the stream's new head.
    pub seqs: RangeInclusive<Seq>,
    /// The timestamp recorded on every event of the append, in Unix
    /// milliseconds.
    pub at_ms: i64,
}

/// An SQLite event store. Owns one connection.
///
/// See the [crate docs](crate) for the guarantees and for how to use it from
/// async code.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

// `burrowd` moves the store onto a blocking thread, so it must stay `Send`.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Store>();
};

impl Store {
    /// Opens the database at `path`, creating it if it doesn't exist, and
    /// applies any pending migrations. The parent directory must exist.
    ///
    /// Several `Store`s, in one process or several, may open the same file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Connection::open(path)?, true)
    }

    /// Opens a private in-memory database, for tests.
    ///
    /// It isn't durable and can't be shared with another `Store`.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, false)
    }

    fn init(mut conn: Connection, file_backed: bool) -> Result<Self> {
        // First, so that every later statement waits for the lock too.
        conn.busy_timeout(BUSY_TIMEOUT)?;

        // The journal mode is stored in the file; the other settings are per
        // connection. An in-memory database reports "memory" and can't use WAL.
        let mode = enable_wal(&conn)?;
        if file_backed && !mode.eq_ignore_ascii_case("wal") {
            return Err(Error::NotWal(mode));
        }
        // FULL, not NORMAL: see D-012. In WAL mode NORMAL survives a killed
        // process but can lose the last commits on power loss, and clients
        // that already saw those seqs would then miss the events that reuse
        // them.
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        migrations::apply(&mut conn)?;
        Ok(Self { conn })
    }

    /// The schema version of the open database. After a successful open, this
    /// is always [`SCHEMA_VERSION`].
    pub fn schema_version(&self) -> Result<u32> {
        migrations::version(&self.conn)
    }

    /// Appends `events` to `stream`, atomically, and returns the seqs they got.
    ///
    /// The events get consecutive seqs after the stream's current head, in the
    /// order given, and all share one timestamp. Either every event is stored
    /// or, on error, none is.
    ///
    /// Fails with [`Error::EmptyAppend`] if `events` is empty, and with
    /// [`Error::EmptyStream`] or [`Error::EmptyEventType`] for empty names.
    pub fn append(&mut self, stream: &str, events: &[NewEvent]) -> Result<Appended> {
        if stream.is_empty() {
            return Err(Error::EmptyStream);
        }
        if events.is_empty() {
            return Err(Error::EmptyAppend);
        }
        if events.iter().any(|event| event.event_type.is_empty()) {
            return Err(Error::EmptyEventType);
        }

        // IMMEDIATE takes the write lock now, before the head is read, so no
        // other connection can append to the stream in between.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let head = head_of(&tx, stream)?;
        let at_ms = now_ms();
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO events (stream, seq, type, payload, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (seq, event) in (head + 1..).zip(events) {
                insert.execute(params![
                    stream,
                    sql_seq(seq),
                    event.event_type,
                    event.payload.get(),
                    at_ms,
                ])?;
            }
        }
        tx.commit()?;

        Ok(Appended {
            seqs: head + 1..=head + events.len() as Seq,
            at_ms,
        })
    }

    /// Reads up to `limit` events of `stream` with a seq greater than `after`,
    /// in seq order.
    ///
    /// `after = 0` reads from the beginning. An `after` at or beyond the head,
    /// or a stream that doesn't exist, gives an empty list. To read everything,
    /// pass `usize::MAX` as the limit.
    pub fn read(&self, stream: &str, after: Seq, limit: usize) -> Result<Vec<Event>> {
        let mut select = self.conn.prepare_cached(
            "SELECT seq, type, payload, at FROM events \
             WHERE stream = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = select.query_map(
            params![
                stream,
                sql_seq(after),
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;

        let mut events = Vec::new();
        for row in rows {
            let (seq, event_type, payload, at_ms) = row?;
            events.push(Event {
                // The schema guarantees seq >= 1.
                seq: seq.unsigned_abs(),
                event_type,
                payload: RawValue::from_string(payload)?,
                at_ms,
            });
        }
        Ok(events)
    }

    /// The seq of the last event in `stream`, or 0 if it has none.
    pub fn head(&self, stream: &str) -> Result<Seq> {
        head_of(&self.conn, stream)
    }
}

/// Switches the database to WAL mode and returns the resulting journal mode.
///
/// SQLite doesn't apply the busy timeout to this switch. When several
/// connections open a new file at once, all but one can get `SQLITE_BUSY`
/// immediately, so retry until the timeout instead. Once the file is in WAL
/// mode, the switch is a no-op that doesn't contend.
fn enable_wal(conn: &Connection) -> Result<String> {
    let deadline = Instant::now() + BUSY_TIMEOUT;
    loop {
        match conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0)) {
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == ErrorCode::DatabaseBusy && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(5));
            }
            result => return Ok(result?),
        }
    }
}

fn head_of(conn: &Connection, stream: &str) -> Result<Seq> {
    let head = conn
        .prepare_cached("SELECT seq FROM events WHERE stream = ?1 ORDER BY seq DESC LIMIT 1")?
        .query_row([stream], |row| row.get::<_, i64>(0))
        .optional()?;
    // The schema guarantees seq >= 1.
    Ok(head.map_or(0, i64::unsigned_abs))
}

/// SQLite integers are signed. Seqs never come near `i64::MAX`; if one did,
/// saturating makes the insert collide with the UNIQUE constraint and fail
/// instead of wrapping.
fn sql_seq(seq: Seq) -> i64 {
    i64::try_from(seq).unwrap_or(i64::MAX)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}
