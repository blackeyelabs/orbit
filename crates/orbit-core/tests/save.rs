//! Saving notes through the Vault facade.

use std::fs;

use orbit_core::{Error, Vault, VaultPath};

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

#[test]
fn a_saved_note_reads_back_exactly_as_saved() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("Projects")).unwrap();
    fs::write(dir.path().join("Projects/roadmap.md"), "# Old\n").unwrap();
    let vault = Vault::open(dir.path()).unwrap();

    let text = "# Roadmap\n\n- [x] Open a vault\n- [ ] Edit 日本語 notes\n";
    vault.save_note(&p("Projects/roadmap.md"), text).unwrap();

    assert_eq!(vault.read_note(&p("Projects/roadmap.md")).unwrap(), text);
    assert_eq!(
        fs::read(dir.path().join("Projects/roadmap.md")).unwrap(),
        text.as_bytes()
    );
}

#[test]
fn front_matter_survives_a_body_edit_byte_for_byte() {
    // Comments, unusual key order, quoting, CRLF line endings and a BOM:
    // everything a YAML re-serializer would normalise.
    let front_matter = "\u{feff}---\r\n# keep this comment\r\nzeta: 1\r\ntags: [b,  a]\r\nalias: 'Orbit'\r\n---\r\n";
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("note.md");
    fs::write(&file, format!("{front_matter}Old body\r\n")).unwrap();
    let vault = Vault::open(dir.path()).unwrap();

    let text = vault.read_note(&p("note.md")).unwrap();
    let edited = text.replace("Old body", "New body");
    vault.save_note(&p("note.md"), &edited).unwrap();

    assert_eq!(
        fs::read(&file).unwrap(),
        format!("{front_matter}New body\r\n").as_bytes()
    );
}

#[cfg(unix)]
#[test]
fn a_save_that_fails_reports_it_and_leaves_the_old_note_intact() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Read only");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("note.md"), "old text").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    // A read-only folder: the save can't create its temporary file, as on a
    // full disk or a folder another app has locked.
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(folder.join("probe"), "").is_ok() {
        eprintln!("skipped: running with permission to ignore modes (root?)");
        return;
    }

    let result = vault.save_note(&p("Read only/note.md"), "new text");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(result.is_err(), "the failed save was reported as a success");
    assert_eq!(
        fs::read_to_string(folder.join("note.md")).unwrap(),
        "old text"
    );
    assert_eq!(
        fs::read_dir(&folder).unwrap().count(),
        1,
        "left files behind"
    );
}

#[cfg(unix)]
#[test]
fn saving_keeps_the_note_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("shared.md");
    fs::write(&file, "old").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    let vault = Vault::open(dir.path()).unwrap();

    vault.save_note(&p("shared.md"), "new").unwrap();

    let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644);
}

#[test]
fn a_note_that_is_not_utf8_is_refused_so_a_save_cannot_mangle_it() {
    let dir = tempfile::tempdir().unwrap();
    let latin1 = b"caf\xe9 au lait";
    fs::write(dir.path().join("latin1.md"), latin1).unwrap();
    let vault = Vault::open(dir.path()).unwrap();

    assert!(matches!(
        vault.read_note(&p("latin1.md")),
        Err(Error::NotUtf8(path)) if path == p("latin1.md")
    ));
    assert_eq!(fs::read(dir.path().join("latin1.md")).unwrap(), latin1);
}

#[test]
fn notes_cannot_be_saved_into_tool_folders() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".s/abc")).unwrap();
    fs::create_dir_all(dir.path().join("Projects/.Obsidian")).unwrap();
    let vault = Vault::open(dir.path()).unwrap();

    for path in [".s/abc/leak.md", "Projects/.Obsidian/x.md"] {
        assert!(vault.save_note(&p(path), "plaintext").is_err(), "{path}");
        assert!(!dir.path().join(path).exists(), "{path} was written");
    }
}

#[test]
fn a_save_that_fails_after_writing_its_temporary_file_cleans_up() {
    // The rename is the last step; a folder in the note's place makes it fail
    // after the new text is already on disk in a temporary file.
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("blocked.md/inside")).unwrap();
    let vault = Vault::open(dir.path()).unwrap();

    assert!(vault.save_note(&p("blocked.md"), "new text").is_err());

    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["blocked.md"], "a temporary file was left behind");
    assert!(dir.path().join("blocked.md/inside").is_dir());
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_is_not_replaced_by_a_save() {
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("gone.md", dir.path().join("link.md")).unwrap();
    let vault = Vault::open(dir.path()).unwrap();

    assert!(vault.save_note(&p("link.md"), "text").is_err());
    assert!(
        fs::symlink_metadata(dir.path().join("link.md"))
            .unwrap()
            .is_symlink()
    );
}
