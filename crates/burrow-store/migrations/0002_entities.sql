-- Migration 2: current state, kept next to the event log (D-017).
--
-- One row per entity (a project, a session, an inbox item, …): its latest
-- state as a JSON document. Rows are written only in the same transaction as
-- the events that describe the change, so they never disagree with the log;
-- they spare `burrowd` from replaying every stream at startup. The event log
-- stays the source of truth for clients.
CREATE TABLE entities (
    -- What sort of entity, for example `session`.
    kind    TEXT    NOT NULL CHECK (kind <> ''),
    -- The entity's id, for example `ses_…`.
    id      TEXT    NOT NULL CHECK (id <> ''),
    -- JSON text, stored exactly as given.
    body    TEXT    NOT NULL,
    -- Wall-clock time of the last write, in Unix milliseconds.
    updated INTEGER NOT NULL,
    PRIMARY KEY (kind, id)
) STRICT, WITHOUT ROWID;
