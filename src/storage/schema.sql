PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY, day TEXT NOT NULL, version INTEGER NOT NULL,
    bytes INTEGER NOT NULL DEFAULT 2, state TEXT NOT NULL DEFAULT 'active',
    UNIQUE(day,version), CHECK (state IN ('active','sealing','sealed'))
);
CREATE UNIQUE INDEX IF NOT EXISTS one_active ON chunks(state) WHERE state='active';
CREATE TABLE IF NOT EXISTS events (
    rowid INTEGER PRIMARY KEY, eid TEXT NOT NULL UNIQUE, cid TEXT NOT NULL,
    chunk INTEGER NOT NULL REFERENCES chunks(id), payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS event_command ON events(cid,rowid);
CREATE INDEX IF NOT EXISTS event_file ON events(chunk,rowid);
CREATE TABLE IF NOT EXISTS health (name TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS retired (chunk INTEGER PRIMARY KEY REFERENCES chunks(id), cleaned INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS expired_events (eid TEXT PRIMARY KEY, cid TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS expired_command ON expired_events(cid);
PRAGMA user_version = 1;
