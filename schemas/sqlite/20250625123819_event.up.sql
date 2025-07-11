CREATE TABLE IF NOT EXISTS event (
       id BLOB(16) PRIMARY KEY,
       stream_id BLOB(16) NOT NULL,
       version INTEGER NOT NULL CHECK (version > 0),
       event_type TEXT NOT NULL,
       payload BLOB NOT NULL,
       persisted_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
       UNIQUE (stream_id, version)
);


CREATE TABLE IF NOT EXISTS event_metadata (
       event_id BLOB(16) PRIMARY KEY,
       correlation_id TEXT NOT NULL,
       FOREIGN KEY (event_id) REFERENCES event(id)
);

CREATE INDEX IF NOT EXISTS metadata_correlation_id_idx ON event_metadata (correlation_id);
