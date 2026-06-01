use std::env;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct MigrationFile {
    id: i64,
    introduced_version: String,
    filename: String,
}

fn parse_introduced_version(contents: &str, default_version: &str) -> String {
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("-- introduced_version:") {
            let value = rest.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    default_version.to_string()
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let migrations_dir = manifest_dir.join("migrations");
    println!("cargo:rerun-if-changed={}", migrations_dir.display());

    let default_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let mut migrations: Vec<MigrationFile> = Vec::new();

    let entries = match fs::read_dir(&migrations_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&migrations_dir).expect("failed to create migrations dir");
            fs::read_dir(&migrations_dir).expect("failed to read migrations dir")
        }
        Err(error) => panic!("failed to read migrations dir: {error:?}"),
    };

    for entry in entries {
        let entry = entry.expect("failed to read migrations entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("sql") {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("migration file name")
            .to_string();
        let stem = Path::new(&filename)
            .file_stem()
            .and_then(|name| name.to_str())
            .expect("migration file stem");
        if !stem.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let id: i64 = stem.parse().expect("migration id");
        let contents = fs::read_to_string(&path).expect("failed to read migration file");
        let introduced_version = parse_introduced_version(&contents, &default_version);
        migrations.push(MigrationFile {
            id,
            introduced_version,
            filename,
        });
        println!("cargo:rerun-if-changed={}", path.display());
    }

    migrations.sort_by_key(|migration| migration.id);
    let mut last_id: Option<i64> = None;
    for migration in &migrations {
        if Some(migration.id) == last_id {
            panic!("duplicate migration id {}", migration.id);
        }
        last_id = Some(migration.id);
    }

    let mut output = String::new();
    writeln!(
        &mut output,
        "#[derive(Clone, Copy, Debug)]\npub struct Migration {{\n    pub id: i64,\n    pub introduced_version: &'static str,\n    pub sql: &'static str,\n}}\n"
    )
    .expect("write migration struct");
    writeln!(&mut output, "pub static MIGRATIONS: &[Migration] = &[").unwrap();
    for migration in &migrations {
        writeln!(
            &mut output,
            "    Migration {{ id: {}, introduced_version: {:?}, sql: include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/migrations/{}\")) }},",
            migration.id, migration.introduced_version, migration.filename
        )
        .unwrap();
    }
    writeln!(&mut output, "];").unwrap();

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("smol_workflow_migrations.rs"), output)
        .expect("write smol_workflow_migrations.rs");
}
