use rusqlite::{Connection, Result};

// event record
#[derive(Debug)]
struct Person {
    #[allow(dead_code)]
    id: i32,
    name: String,
    data: Option<Vec<u8>>,
}

fn main() -> Result<()> {
    // setup
    let conn = Connection::open_in_memory()?;

    // one time bootstrap, since its in memory, we need this everytime.
    conn.execute(
        "CREATE TABLE person (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            data BLOB
        )",
        (),
    )?;

    let me = Person {
        id: 0,
        name: "Steven".to_string(),
        data: None,
    };

    // part of EventStore append
    conn.execute(
        "INSERT INTO person(name, data) VALUES (?1, ?2)",
        (&me.name, &me.data),
    )?;

    // part of EventStore fetch
    let mut stmt = conn.prepare("SELECT id, name, data FROM person")?;

    let person_iter = stmt.query_map([], |row| {
        Ok(Person {
            id: row.get(0)?,
            name: row.get(1)?,
            data: row.get(2)?,
        })
    })?;

    for person in person_iter {
        println!("Found person {:?}", person?);
    }

    Ok(())
}
