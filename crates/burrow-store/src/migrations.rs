//! Numbered schema migrations, embedded in the binary.
//!
//! Migration `n` is `MIGRATIONS[n - 1]`. The database's `PRAGMA user_version`
//! records how many have been applied. To change the schema, add a new file
//! `migrations/NNNN_name.sql` and append it here; never edit a migration that
//! has shipped.

use rusqlite::{Connection, TransactionBehavior};

use crate::{Error, Result};

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_events.sql"),
    include_str!("../migrations/0002_entities.sql"),
];

/// The schema version this build creates and understands: the number of
/// migrations it contains.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

/// Reads the database's schema version (`PRAGMA user_version`).
pub(crate) fn version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Brings the database up to [`SCHEMA_VERSION`].
///
/// Pending migrations run in one `BEGIN IMMEDIATE` transaction, together with
/// the `user_version` bump, so a crash can't leave a half-migrated database and
/// two processes opening the same new file can't both migrate it.
pub(crate) fn apply(conn: &mut Connection) -> Result<()> {
    // Fast path: an up-to-date database needs no write lock.
    if version(conn)? == SCHEMA_VERSION {
        return Ok(());
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Check again under the lock: another connection may have migrated since.
    let current = version(&tx)?;
    if current > SCHEMA_VERSION {
        return Err(Error::SchemaTooNew {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    for next in current + 1..=SCHEMA_VERSION {
        tx.execute_batch(MIGRATIONS[next as usize - 1])?;
        tx.pragma_update(None, "user_version", next)?;
    }
    tx.commit()?;
    Ok(())
}
