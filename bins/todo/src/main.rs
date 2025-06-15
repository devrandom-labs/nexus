use rusqlite::Result;

mod store;

fn main() -> Result<()> {
    let _store = store::Store::new()?;
    Ok(())
}
