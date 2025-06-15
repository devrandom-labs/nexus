use rusqlite::{Connection, Error};

pub struct Store {
    pub connection: Connection,
}

impl Store {
    pub fn new() -> Result<Self, Error> {
        let connection = Connection::open_in_memory()?;

        Ok(Store { connection })
    }
}
