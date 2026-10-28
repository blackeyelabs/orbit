//! Very large notes open as source only, to keep typing fast; the preview
//! is one choice away.

use std::fs;

use orbit_core::{Vault, VaultPath, is_large_note};

#[test]
fn notes_of_several_megabytes_are_large_and_open_whole() {
    let dir = tempfile::tempdir().unwrap();
    let line = "- 2019-01-07 kickoff, notes below\n";
    let big = line.repeat(3 * 1024 * 1024 / line.len());
    fs::write(dir.path().join("Meeting log.md"), &big).unwrap();
    fs::write(dir.path().join("Roadmap.md"), line.repeat(3_000)).unwrap();
    let vault = Vault::open(dir.path()).unwrap();

    let (_, text) = vault
        .open_note(&VaultPath::new("Meeting log.md").unwrap())
        .unwrap();
    assert_eq!(text, big, "opened whole, nothing cut off");
    assert!(is_large_note(&text));
    let (_, text) = vault
        .open_note(&VaultPath::new("Roadmap.md").unwrap())
        .unwrap();
    assert!(!is_large_note(&text), "a long note isn't a large one");
    assert!(!is_large_note(&"x".repeat(1024 * 1024)), "1 MB isn't");
}
