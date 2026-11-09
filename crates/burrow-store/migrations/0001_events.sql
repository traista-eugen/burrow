-- Migration 1: the event log (D-005).
--
-- One table holds every stream. A stream is a name such as `index`,
-- `session:<id>` or `run:<id>`. `seq` counts from 1 within each stream with no
-- gaps; the store assigns it inside a `BEGIN IMMEDIATE` transaction, and the
-- UNIQUE constraint is the backstop.
CREATE TABLE events (
    -- Global append order across all streams. Declared so that VACUUM can't
    -- renumber it. Internal: the public API orders by (stream, seq).
    id      INTEGER PRIMARY KEY,
    stream  TEXT    NOT NULL CHECK (stream <> ''),
    seq     INTEGER NOT NULL CHECK (seq >= 1),
    type    TEXT    NOT NULL CHECK (type <> ''),
    -- JSON text, stored exactly as given.
    payload TEXT    NOT NULL,
    -- Wall-clock time of the append, in Unix milliseconds.
    at      INTEGER NOT NULL,
    UNIQUE (stream, seq)
) STRICT;
