//! Deleting sends notes and folders to the freedesktop trash.
//!
//! The test points `XDG_DATA_HOME` at a temporary folder on the same disk as
//! its vault, so the home trash there is the one used and the real one is
//! never touched. It's the only test in this binary, so no other test thread
//! reads the environment while it changes.

use std::fs;

use orbit_core::{Vault, VaultPath};

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

#[test]
fn deleting_sends_notes_and_folders_to_the_system_trash() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary, so nothing else reads the
    // environment concurrently.
    unsafe { std::env::set_var("XDG_DATA_HOME", home.path()) };
    let trashed = |name: &str| home.path().join("Trash/files").join(name);

    let dir = tempfile::tempdir_in(home.path()).unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("Inbox")).unwrap();
    fs::create_dir_all(root.join("Old project")).unwrap();
    fs::write(root.join("Inbox/doomed note.md"), "goodbye").unwrap();
    fs::write(root.join("Old project/plan.md"), "the plan").unwrap();
    fs::write(root.join("keep.md"), "kept").unwrap();
    let vault = Vault::open(root).unwrap();

    // A note goes to the system trash, not to an Orbit trash folder.
    vault.delete(&p("Inbox/doomed note.md")).unwrap();
    assert!(!root.join("Inbox/doomed note.md").exists());
    assert_eq!(
        fs::read_to_string(trashed("doomed note.md")).unwrap(),
        "goodbye"
    );
    assert!(!root.join(".trash").exists());

    // A folder goes with everything in it.
    vault.delete(&p("Old project")).unwrap();
    assert!(!root.join("Old project").exists());
    assert_eq!(
        fs::read_to_string(trashed("Old project/plan.md")).unwrap(),
        "the plan"
    );

    // The vault itself and missing paths can't be deleted.
    assert!(vault.delete(&VaultPath::root()).is_err());
    assert!(vault.delete(&p("missing.md")).is_err());
    assert!(root.join("keep.md").exists());
}
