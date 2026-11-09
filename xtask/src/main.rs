//! Repository tasks, run with `cargo xtask <task>` (aliased in `.cargo/config.toml`).
//!
//! - `schema`: write the protocol's JSON Schema to `schema/burrow-protocol.schema.json`.
//! - `schema --check`: fail if that file is out of date, without writing it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

use burrow_protocol::schema::SCHEMA_PATH;

const USAGE: &str = "usage: cargo xtask schema [--check]

tasks:
  schema           write schema/burrow-protocol.schema.json from the burrow-protocol types
  schema --check   fail if schema/burrow-protocol.schema.json is out of date";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["schema"] => write_schema(),
        ["schema", "--check"] => check_schema(),
        ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn write_schema() -> Result<(), String> {
    let path = repo_root().join(SCHEMA_PATH);
    let schema = burrow_protocol::schema_json();
    if fs::read_to_string(&path).is_ok_and(|current| current == schema) {
        println!("{SCHEMA_PATH} is up to date");
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|error| format!("creating {}: {error}", dir.display()))?;
    }
    fs::write(&path, schema).map_err(|error| format!("writing {}: {error}", path.display()))?;
    println!("wrote {SCHEMA_PATH}");
    Ok(())
}

fn check_schema() -> Result<(), String> {
    let path = repo_root().join(SCHEMA_PATH);
    let current = fs::read_to_string(&path)
        .map_err(|error| format!("reading {}: {error}", path.display()))?;
    if current.replace("\r\n", "\n") == burrow_protocol::schema_json() {
        println!("{SCHEMA_PATH} is up to date");
        Ok(())
    } else {
        Err(format!(
            "{SCHEMA_PATH} is out of date; run `cargo xtask schema`"
        ))
    }
}

/// The workspace root: this crate lives in `<root>/xtask`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}
