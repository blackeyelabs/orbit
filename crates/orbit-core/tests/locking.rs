//! Locking always wins over saving (ADR 0004): pending edits are sealed,
//! the folder locks, and then their blobs are written; one that can't be
//! written is held in memory, as ciphertext only, until the folder next
//! unlocks (or the vault closes, and it's gone). The newest text wins.

use std::fs;

use orbit_core::{Session, SessionTab, Vault, VaultPath, Zeroizing};
use orbit_crypto::KdfParams;

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

fn edit(path: &VaultPath, text: &str) -> Vec<(VaultPath, Zeroizing<String>)> {
    vec![(path.clone(), Zeroizing::new(text.to_string()))]
}

fn protected() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("Diary")).unwrap();
    fs::write(dir.path().join("Diary/Monday.md"), "first draft").unwrap();
    fs::write(dir.path().join("Plain.md"), "plain").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    vault.set_kdf_params(KdfParams::INSECURE_TEST);
    let keys = vault.prepare_protection("pass").unwrap();
    vault.protect_folder(&p("Diary"), keys).unwrap();
    (dir, vault)
}

/// Every byte under the vault folder, searched for `text`.
fn on_disk(dir: &std::path::Path, text: &str) -> bool {
    fn walk(dir: &std::path::Path, text: &str) -> bool {
        fs::read_dir(dir).unwrap().flatten().any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, text)
            } else {
                let bytes = fs::read(&path).unwrap_or_default();
                bytes.windows(text.len()).any(|w| w == text.as_bytes())
            }
        })
    }
    walk(dir, text)
}

#[test]
fn pending_edits_are_saved_before_locking() {
    let (_dir, vault) = protected();
    let monday = p("Diary/Monday.md");
    let locked = vault.lock_saving(&p("Diary"), edit(&monday, "second draft"));
    assert!(locked.unsaved.is_empty());
    assert!(vault.unlocked_folders().is_empty());
    vault.unlock("pass").unwrap();
    assert_eq!(vault.read_note(&monday).unwrap(), "second draft");
}

/// Makes the protected folder's blobs read-only; `None` if writes land
/// anyway (running as root).
#[cfg(unix)]
fn read_only_storage(vault: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let storage = fs::read_dir(vault.join(".s"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path()
        .join("blobs");
    fs::set_permissions(&storage, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(storage.join("probe"), "").is_ok() {
        eprintln!("skipped: running with permission to ignore modes (root?)");
        return None;
    }
    Some(storage)
}

#[cfg(unix)]
#[test]
fn when_saving_fails_the_lock_still_happens_and_the_edits_wait_encrypted() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, vault) = protected();
    let monday = p("Diary/Monday.md");
    // Encrypted saves can't land: the folder's storage is read-only.
    let Some(storage) = read_only_storage(dir.path()) else {
        return;
    };

    let locked = vault.lock_saving(&p("Diary"), edit(&monday, "edits-in-waiting"));
    assert_eq!(locked.unsaved, std::slice::from_ref(&monday));
    assert!(vault.unlocked_folders().is_empty(), "locked all the same");
    assert!(!on_disk(dir.path(), "edits-in-waiting"));

    // Once it can be saved again, unlocking saves what waited.
    fs::set_permissions(&storage, fs::Permissions::from_mode(0o755)).unwrap();
    vault.unlock("pass").unwrap();
    assert_eq!(vault.read_note(&monday).unwrap(), "edits-in-waiting");
    vault.lock_all();
    vault.unlock("pass").unwrap();
    assert_eq!(vault.read_note(&monday).unwrap(), "edits-in-waiting");
}

#[cfg(unix)]
#[test]
fn a_held_edit_never_overwrites_text_saved_after_it() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, vault) = protected();
    let monday = p("Diary/Monday.md");
    let Some(storage) = read_only_storage(dir.path()) else {
        return;
    };
    let locked = vault.lock_saving(&p("Diary"), edit(&monday, "older edit"));
    assert_eq!(locked.unsaved, std::slice::from_ref(&monday));
    // Still can't be written when the folder unlocks: it stays held.
    vault.unlock("pass").unwrap();
    assert_eq!(vault.read_note(&monday).unwrap(), "first draft");

    fs::set_permissions(&storage, fs::Permissions::from_mode(0o755)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    vault.save_note(&monday, "newer text").unwrap();
    vault.lock_all();
    // The held edit is written now, but it's the older.
    vault.unlock("pass").unwrap();
    assert_eq!(vault.read_note(&monday).unwrap(), "newer text");
    vault.lock_all();
    vault.unlock("pass").unwrap();
    assert_eq!(vault.read_note(&monday).unwrap(), "newer text");
}

#[test]
fn nothing_of_a_locked_folder_is_restorable() {
    let (dir, vault) = protected();
    let tab = |path: &str| SessionTab {
        path: p(path),
        scroll: 0.,
    };
    vault
        .save_session(&Session {
            tabs: vec![tab("Diary/Monday.md"), tab("Plain.md")],
            active: Some(0),
        })
        .unwrap();
    vault.lock_saving(&p("Diary"), Vec::new());
    let tabs: Vec<VaultPath> = vault.session().tabs.into_iter().map(|t| t.path).collect();
    assert_eq!(tabs, [p("Plain.md")]);
    assert!(!on_disk(dir.path(), "Monday"));
    // Not even once unlocked again: tabs of a protected folder never return.
    vault.unlock("pass").unwrap();
    assert_eq!(vault.session().tabs.len(), 1);
}

#[test]
fn auto_lock_settings_default_on_and_can_be_changed() {
    use orbit_core::Settings;
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings::load(dir.path());
    assert_eq!(settings.auto_lock_minutes, Some(10));
    assert!(settings.lock_on_suspend);
    assert!(settings.lock_on_screen_lock);

    fs::write(
        dir.path().join("settings.json"),
        r#"{"auto_lock_minutes": null, "lock_on_suspend": false, "lock_on_screen_lock": "x"}"#,
    )
    .unwrap();
    let settings = Settings::load(dir.path());
    assert_eq!(settings.auto_lock_minutes, None, "never");
    assert!(!settings.lock_on_suspend);
    assert!(
        settings.lock_on_screen_lock,
        "an invalid value keeps the default"
    );
    let mut changed = settings.clone();
    changed.auto_lock_minutes = Some(30);
    changed.save(dir.path()).unwrap();
    assert_eq!(Settings::load(dir.path()).auto_lock_minutes, Some(30));
}
