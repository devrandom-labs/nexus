use rusqlite::{Connection, Error};
use std::sync::{Arc, Mutex};

// going to be started in one place..
pub struct Store {
    #[allow(dead_code)]
    pub connection: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn new() -> Result<Self, Error> {
        let connection = Connection::open_in_memory()?;
        connection.execute(
            "CREATE TABLE IF NOT EXISTS nexus_events (
                 id TEXT PRIMARY KEY,
                 stream_id TEXT NOT NULL,
                 version INTEGER NOT NULL,
                 event_type TEXT NOT NULL,
                 payload BLOB NOT NULL,
                 UNIQUE (stream_id, version)
            )",
            (),
        )?;
        Ok(Store {
            connection: Arc::new(Mutex::new(connection)),
        })
    }
}
