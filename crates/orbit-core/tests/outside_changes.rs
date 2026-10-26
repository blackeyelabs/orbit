//! Other apps changing the vault while Orbit has a note open: each case is
//! driven by changing files behind the Vault's back.

use std::fs;

use orbit_core::{DiskCheck, Error, Trouble, Vault, VaultPath};

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

fn vault_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    (dir, vault)
}

#[test]
fn a_note_without_edits_reloads_silently_when_another_app_changes_it() {
    let (dir, vault) = vault_with(&[("note.md", "mine")]);
    let (mut note, text) = vault.open_note(&p("note.md")).unwrap();
    assert_eq!(text, "mine");
    assert_eq!(note.check_disk(&vault, false), DiskCheck::Unchanged);

    fs::write(dir.path().join("note.md"), "theirs").unwrap();

    assert_eq!(
        note.check_disk(&vault, false),
        DiskCheck::Reload("theirs".into())
    );
    assert_eq!(note.trouble(), None);
    // What was reloaded is now the known version.
    assert_eq!(note.check_disk(&vault, false), DiskCheck::Unchanged);
}

#[test]
fn orbits_own_saves_are_not_changes_by_another_app() {
    let (_dir, vault) = vault_with(&[("note.md", "mine")]);
    let (mut note, _) = vault.open_note(&p("note.md")).unwrap();
    note.save(&vault, "edited").unwrap();
    assert_eq!(note.check_disk(&vault, true), DiskCheck::Unchanged);
    note.save(&vault, "edited again").unwrap();
    assert_eq!(
        fs::read_to_string(_dir.path().join("note.md")).unwrap(),
        "edited again"
    );
}

#[test]
fn with_unsaved_edits_another_apps_change_waits_for_load_theirs_or_keep_mine() {
    let (dir, vault) = vault_with(&[("note.md", "original")]);
    let file = dir.path().join("note.md");

    // Load theirs.
    let (mut note, _) = vault.open_note(&p("note.md")).unwrap();
    fs::write(&file, "theirs").unwrap();
    assert_eq!(note.check_disk(&vault, true), DiskCheck::Trouble);
    assert!(matches!(note.trouble(), Some(Trouble::Changed(t)) if t == "theirs"));
    // Saving waits for the choice, so theirs isn't overwritten.
    assert!(matches!(
        note.save(&vault, "mine"),
        Err(Error::ChangedOnDisk(_))
    ));
    assert_eq!(fs::read_to_string(&file).unwrap(), "theirs");
    assert_eq!(note.load_theirs(), Some("theirs".to_string()));
    assert_eq!(note.trouble(), None);
    note.save(&vault, "mine on theirs").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "mine on theirs");

    // Keep mine.
    fs::write(&file, "theirs again").unwrap();
    assert_eq!(note.check_disk(&vault, true), DiskCheck::Trouble);
    note.keep_mine(&vault, "mine wins").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "mine wins");
    assert_eq!(note.trouble(), None);
}

#[test]
fn keep_mine_never_overwrites_a_newer_change_than_the_one_shown() {
    let (dir, vault) = vault_with(&[("note.md", "original")]);
    let file = dir.path().join("note.md");
    let (mut note, _) = vault.open_note(&p("note.md")).unwrap();
    fs::write(&file, "theirs").unwrap();
    note.check_disk(&vault, true);

    fs::write(&file, "theirs, newer").unwrap();

    assert!(note.keep_mine(&vault, "mine").is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "theirs, newer");
    assert!(matches!(note.trouble(), Some(Trouble::Changed(t)) if t == "theirs, newer"));
}

#[test]
fn a_saving_race_with_another_app_is_caught_by_a_save_not_lost() {
    // Another app writes between Orbit's last check and its save.
    let (dir, vault) = vault_with(&[("note.md", "original")]);
    let (mut note, _) = vault.open_note(&p("note.md")).unwrap();
    fs::write(dir.path().join("note.md"), "theirs").unwrap();

    assert!(matches!(
        note.save(&vault, "mine"),
        Err(Error::ChangedOnDisk(_))
    ));
    assert_eq!(
        fs::read_to_string(dir.path().join("note.md")).unwrap(),
        "theirs"
    );
    assert!(matches!(note.trouble(), Some(Trouble::Changed(t)) if t == "theirs"));
}

#[test]
fn a_deleted_note_offers_save_a_copy_which_never_recreates_the_deleted_one() {
    let (dir, vault) = vault_with(&[("Inbox/plan.md", "the plan")]);
    let (mut note, _) = vault.open_note(&p("Inbox/plan.md")).unwrap();
    fs::remove_file(dir.path().join("Inbox/plan.md")).unwrap();

    assert_eq!(note.check_disk(&vault, false), DiskCheck::Trouble);
    assert_eq!(note.trouble(), Some(&Trouble::Deleted));
    assert!(matches!(
        note.save(&vault, "edits"),
        Err(Error::DeletedOnDisk(_))
    ));
    assert!(!dir.path().join("Inbox/plan.md").exists());

    let copy = note.save_copy(&vault, "rescued edits").unwrap();
    assert_eq!(copy, p("Inbox/plan (copy).md"));
    assert_eq!(note.path(), &copy);
    assert_eq!(note.trouble(), None);
    assert_eq!(vault.read_note(&copy).unwrap(), "rescued edits");
    assert!(!dir.path().join("Inbox/plan.md").exists());
}

#[cfg(unix)]
#[test]
fn a_failed_save_keeps_the_note_unsaved_until_trying_again_works() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, vault) = vault_with(&[("Locked/note.md", "old")]);
    let folder = dir.path().join("Locked");
    let (mut note, _) = vault.open_note(&p("Locked/note.md")).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(folder.join("probe"), "").is_ok() {
        return eprintln!("skipped: running with permission to ignore modes (root?)");
    }

    let failed = note.save(&vault, "new");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(failed.is_err());
    assert!(matches!(note.trouble(), Some(Trouble::SaveFailed(_))));
    assert_eq!(fs::read_to_string(folder.join("note.md")).unwrap(), "old");
    // Try again, once the folder can be written.
    note.save(&vault, "new").unwrap();
    assert_eq!(note.trouble(), None);
    assert_eq!(fs::read_to_string(folder.join("note.md")).unwrap(), "new");
}

#[test]
fn a_note_another_app_made_unreadable_is_reported_not_silently_kept() {
    let (dir, vault) = vault_with(&[("note.md", "text")]);
    let (mut note, _) = vault.open_note(&p("note.md")).unwrap();
    fs::write(dir.path().join("note.md"), b"caf\xe9").unwrap();

    assert_eq!(note.check_disk(&vault, false), DiskCheck::Trouble);
    assert!(matches!(note.trouble(), Some(Trouble::Unreadable(_))));
    // Keep mine replaces the unreadable bytes, as chosen.
    note.keep_mine(&vault, "text").unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("note.md")).unwrap(),
        "text"
    );
}

/// Collects watcher batches until `want` shows up, or 10 s pass.
fn changes_until(
    rx: &std::sync::mpsc::Receiver<Vec<VaultPath>>,
    want: &VaultPath,
) -> Vec<VaultPath> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut seen = Vec::new();
    while !seen.contains(want) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(batch) => seen.extend(batch),
            Err(_) => panic!("{want} never reported; saw {seen:?}"),
        }
    }
    seen
}

#[test]
fn the_watcher_reports_changes_outside_orbit_but_not_in_tool_folders() {
    let (dir, vault) = vault_with(&[("Inbox/a.md", "a"), (".obsidian/app.json", "{}")]);
    let (tx, rx) = std::sync::mpsc::channel();
    let _watch = vault
        .watch(move |paths| {
            tx.send(paths).ok();
        })
        .unwrap();

    fs::write(dir.path().join(".obsidian/app.json"), "{\"x\":1}").unwrap();
    fs::write(dir.path().join("Inbox/new.md"), "new").unwrap();
    let seen = changes_until(&rx, &p("Inbox/new.md"));
    assert!(
        !seen.iter().any(|p| p.as_str().starts_with(".obsidian")),
        "{seen:?}"
    );

    // A rename reports both ends, possibly in different batches.
    fs::rename(dir.path().join("Inbox/a.md"), dir.path().join("b.md")).unwrap();
    let seen = changes_until(&rx, &p("b.md"));
    if !seen.contains(&p("Inbox/a.md")) {
        changes_until(&rx, &p("Inbox/a.md"));
    }
}
