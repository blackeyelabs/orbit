//! Fixture vaults shaped like Obsidian's, with real Obsidian's answers about
//! them beside each one.

use std::{fs, path::Path};

use orbit_core::Vault;
use serde_json::Value;

pub fn fixtures() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A copy of the fixture vault `name`, so opening it writes nothing into
/// the repo.
pub fn fixture_vault(name: &str) -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixtures().join(name), dir.path());
    let vault = Vault::open(dir.path()).unwrap();
    (dir, vault)
}

/// What Obsidian answered about the fixture vault `name`.
pub fn obsidian(name: &str) -> Value {
    let file = fixtures().join(format!("{name}.obsidian.json"));
    serde_json::from_slice(&fs::read(file).unwrap()).unwrap()
}
