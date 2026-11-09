/// Errors returned by the event store.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// SQLite failed, for example because the disk is full or another
    /// connection held the write lock for longer than the busy timeout.
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// A payload couldn't be serialized to JSON, or a stored payload couldn't
    /// be parsed or deserialized into the requested type.
    #[error("event payload JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// The database was written by a newer build of Burrow, with migrations
    /// this build doesn't know about.
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    SchemaTooNew {
        /// The database's `user_version`.
        found: u32,
        /// [`SCHEMA_VERSION`](crate::SCHEMA_VERSION) of this build.
        supported: u32,
    },

    /// SQLite wouldn't switch a file database to WAL mode. This happens on
    /// file systems without shared memory support, such as network drives.
    #[error("SQLite refused WAL mode; the journal mode is {0:?}")]
    NotWal(String),

    /// [`Store::append`](crate::Store::append) was called with no events.
    #[error("an append needs at least one event")]
    EmptyAppend,

    /// A stream name was empty.
    #[error("stream name is empty")]
    EmptyStream,

    /// An event type was empty.
    #[error("event type is empty")]
    EmptyEventType,
}

/// `Result` with the store's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
