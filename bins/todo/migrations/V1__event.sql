CREATE TABLE IF NOT EXISTS event (
       sequence_number INTEGER PRIMARY KEY,
       id TEXT NOT NULL UNIQUE,
       stream_id TEXT NOT NULL,
       version INTEGER NOT NULL CHECK (version > 0),
       event_type TEXT NOT NULL,
       payload BLOB NOT NULL,
       persisted_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
       UNIQUE (stream_id, version)
);


CREATE TABLE IF NOT EXISTS event_metadata (
       sequence_number INTEGER PRIMARY KEY,
       correlation_id TEXT NOT NULL,
       FOREIGN KEY (sequence_number) REFERENCES event(sequence_number)
);

CREATE INDEX IF NOT EXISTS metadata_correlation_id_idx ON event_metadata (correlation_id);
