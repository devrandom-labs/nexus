use std::{
    env,
    fs::{self, DirEntry},
    io::{ErrorKind, Result},
    path::{Path, PathBuf},
};

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
struct MigrationFile {
    pub timestamp: u64,
    pub from: PathBuf,
    pub file_name: String,
}

fn main() -> Result<()> {
    let migration_path = get_migration_path();
    println!(
        "cargo:warning=Searching for migrations in: {}",
        migration_path.display()
    );
    println!("cargo:rerun-if-changed={}", migration_path.display());

    if !migration_path.exists() {
        println!("Migration source directory does not exist. Skipping.");
        return Ok(());
    }

    let mut migration_files = fs::read_dir(migration_path)?
        .filter_map(|r| r.ok())
        .inspect(|entry| println!("Found entry: {:?}", entry.file_name())) // DEBUG: See all entries
        .filter(is_sql)
        .inspect(|entry| println!("After is_sql filter: {:?}", entry.file_name())) // DEBUG: See what passes is_sql
        .filter(only_up_files)
        .inspect(|entry| println!("After only_up filter: {:?}", entry.file_name())) // DEBUG: See what passes only_up
        .filter_map(convert_to_tuple)
        .collect::<Vec<_>>();

    println!(
        "Found {} valid migration files to process.",
        migration_files.len()
    );

    migration_files.sort();

    if !migration_files.is_empty() {
        let local_migration_dir = Path::new("migrations");
        println!(
            "Creating migrations directory at: {}",
            local_migration_dir.display()
        );

        remove_dir_if_exists(local_migration_dir)?;
        fs::create_dir(local_migration_dir)?;

        for (idx, migration) in migration_files.iter().enumerate() {
            let new_file_name = format!("V{}__{}.sql", idx + 1, migration.file_name);
            let dest_path = local_migration_dir.join(new_file_name);
            println!("Copying to: {}", dest_path.display());
            fs::copy(&migration.from, &dest_path)?;
        }
    }

    Ok(())
}

// cross Operating system

fn get_migration_path() -> PathBuf {
    // if let Ok(schemas_dir) = env::var("SCHEMAS_DIR") {
    //     return PathBuf::from(schemas_dir).join("sqlite");
    // }
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let project_root = manifest_dir.parent().unwrap().parent().unwrap();
    project_root.join("schemas").join("sqlite")
}

fn is_sql(entry: &DirEntry) -> bool {
    entry.path().extension().is_some_and(|ext| ext == "sql")
}

fn only_up_files(entry: &DirEntry) -> bool {
    entry.path().to_string_lossy().contains(".up.sql")
}

fn convert_to_tuple(entry: DirEntry) -> Option<MigrationFile> {
    let file_name = entry.file_name();
    let file_name = file_name.to_string_lossy();
    let (prefix, rest) = file_name.split_once("_")?;
    let timestamp = prefix.parse::<u64>().ok()?;
    let file_name = rest.strip_suffix(".up.sql")?.to_string();
    Some(MigrationFile {
        timestamp,
        from: entry.path(),
        file_name,
    })
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
