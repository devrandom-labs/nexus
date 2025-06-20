CREATE TABLE IF NOT EXISTS event (
       id TEXT PRIMARY,
       stream_id TEXT NOT NULL,
       version INTEGER NOT NULL CHECK (version > 0),
       event_type TEXT NOT NULL,
       payload BLOB NOT NULL,
       persisted_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
       UNIQUE (stream_id, version)
);


CREATE TABLE IF NOT EXISTS event_metadata (
       event_id TEXT PRIMARY,
       correlation_id TEXT NOT NULL,
       FOREIGN KEY (event_id) REFERENCES event(id)
);

CREATE INDEX IF NOT EXISTS metadata_correlation_id_idx ON event_metadata (correlation_id);
