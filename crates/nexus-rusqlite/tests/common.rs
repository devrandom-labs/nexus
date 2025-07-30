use nexus_rusqlite::Store;
use refinery::embed_migrations;
use rusqlite::Connection;

embed_migrations!("migrations");

pub struct TestContext {
    pub store: Store,
}

impl TestContext {
    pub fn new() -> Self {
        let mut conn = Connection::open_in_memory().expect("could not open connection");
        migrations::runner()
            .run(&mut conn)
            .expect("migrations could not be applied.");

        TestContext {
            store: Store::new(conn).expect("Store could not be initialized"),
        }
    }
}
