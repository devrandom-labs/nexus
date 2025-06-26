const SQL_SCHEMA: &str = "../nexus-sql-schemas/migrations";

fn main() {
    println!("cargo:rerun-if-changed={}", SQL_SCHEMA);
    // TODO: go to "../nesus-sql-schemas/migrations"
    // TODO: get only the up migration schemas in time order
    // TODO: convert them into seq
    // TODO: create migration folder
}
