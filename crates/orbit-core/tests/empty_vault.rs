//! Whether a vault has notes, for its empty state: notes and canvases
//! count, attachments alone don't, and a locked protected folder means it
//! isn't empty, though what it holds isn't shown.

use std::fs;

use orbit_core::{Vault, VaultPath};
use orbit_crypto::KdfParams;

#[test]
fn a_vault_has_notes_when_a_note_canvas_or_locked_folder_is_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    assert!(!vault.has_notes(), "nothing at all");

    fs::create_dir_all(dir.path().join("Pictures")).unwrap();
    fs::write(dir.path().join("Pictures/photo.png"), "PNG").unwrap();
    assert!(
        !vault.has_notes(),
        "a folder and an attachment aren't notes"
    );

    fs::write(dir.path().join("Board.canvas"), "{}").unwrap();
    assert!(vault.has_notes(), "a canvas is");
    fs::remove_file(dir.path().join("Board.canvas")).unwrap();

    fs::write(dir.path().join("Pictures/Diary.md"), "# Diary").unwrap();
    assert!(vault.has_notes());
    vault.set_kdf_params(KdfParams::INSECURE_TEST);
    let keys = vault.prepare_protection("pass").unwrap();
    vault
        .protect_folder(&VaultPath::new("Pictures").unwrap(), keys)
        .unwrap();
    vault.lock_all();
    assert!(vault.has_notes(), "its notes are only locked away");
}
