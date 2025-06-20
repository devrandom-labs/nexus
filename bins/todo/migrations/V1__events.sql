CREATE TABLE IF NOT EXISTS events (
       sequence_number INTEGER PRIMARY KEY,
       id TEXT NOT NULL UNIQUE,
       stream_id TEXT NOT NULL,
       version INTEGER NOT NULL CHECK (version > 0),
       event_type TEXT NOT NULL,
       payload BLOB NOT NULL,
       persisted_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
       metadata BLOB,
       UNIQUE (stream_id, version)
);
