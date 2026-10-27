//! Protected folders through the Vault facade: protect, edit, lock and
//! unlock, by passphrase (blind, across every locked folder) or recovery
//! key. After every lock, a leak check reads every byte under the vault
//! folder, `.vault/` included, for anything of the folder's in plain text.

use std::{fs, path::Path, time::Duration};

use orbit_core::{Error, UnlockError, Vault, VaultPath};
use orbit_crypto::KdfParams;

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

/// What only the protected folder knows: none of it may be on disk while
/// it's locked, in any case.
const SECRETS: &[&str] = &[
    "Zanzibar Diary",
    "otter-confession",
    "Kestrel Plans",
    "the quiet albatross",
    "Hidden Wing",
    "lagoon-map",
    "edited-after-protecting",
];

fn vault_with_secrets() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in [
        ("Public.md", "Nothing to hide."),
        (
            "Zanzibar Diary/otter-confession.md",
            "# Day one\nthe quiet albatross sang",
        ),
        (
            "Zanzibar Diary/Hidden Wing/Kestrel Plans.md",
            "Meet at dawn #lagoon-map",
        ),
        (
            "Zanzibar Diary/Hidden Wing/lagoon-map.png",
            "PNG the quiet albatross",
        ),
    ] {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    fs::create_dir_all(dir.path().join("Zanzibar Diary/Empty Corner")).unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    vault.set_kdf_params(KdfParams::INSECURE_TEST);
    // As in the app: indexed for search, and a session saved, before.
    vault.index_search();
    vault.search("albatross", 10);
    (dir, vault)
}

/// Every byte of every file under `dir`, and every name, searched for
/// each secret as written and lowercased.
fn assert_no_leak(dir: &Path) {
    fn walk(dir: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = if path.is_dir() {
                walk(&path, found);
                Vec::new()
            } else {
                fs::read(&path).unwrap()
            };
            for secret in SECRETS {
                for form in [secret.to_string(), secret.to_lowercase()] {
                    if name.contains(&form)
                        || bytes.windows(form.len()).any(|w| w == form.as_bytes())
                    {
                        found.push(format!("{form:?} in {}", path.display()));
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, &mut found);
    assert!(found.is_empty(), "plaintext on disk:\n{}", found.join("\n"));
}

fn tree_names(vault: &Vault) -> Vec<String> {
    vault.tree().unwrap().into_iter().map(|n| n.name).collect()
}

#[test]
fn protect_edit_lock_and_unlock_round_trip_with_nothing_left_in_plain_text() {
    let (dir, vault) = vault_with_secrets();
    let folder = p("Zanzibar Diary");
    let keys = vault
        .prepare_protection("correct horse battery staple")
        .unwrap();
    let recovery = keys.recovery_key().to_string();
    vault.protect_folder(&folder, keys).unwrap();

    // Unlocked, it's like any folder: in the tree, readable, editable.
    assert_eq!(vault.unlocked_folders(), std::slice::from_ref(&folder));
    assert!(tree_names(&vault).contains(&"Zanzibar Diary".to_string()));
    let note = p("Zanzibar Diary/otter-confession.md");
    assert_eq!(
        vault.read_note(&note).unwrap(),
        "# Day one\nthe quiet albatross sang"
    );
    vault
        .save_note(&note, "edited-after-protecting the quiet albatross")
        .unwrap();
    let new = vault.create_note(&p("Zanzibar Diary/Hidden Wing")).unwrap();
    vault.save_note(&new, "otter-confession again").unwrap();
    let renamed = vault.rename(&new, "Kestrel Plans 2").unwrap();
    assert_eq!(renamed, p("Zanzibar Diary/Hidden Wing/Kestrel Plans 2.md"));
    // Deleted outright: the system trash would keep plaintext.
    vault.delete(&renamed).unwrap();
    assert_eq!(vault.protected_folder_of(&note), Some(folder.clone()));
    assert_eq!(vault.protected_folder_of(&p("Public.md")), None);

    assert!(vault.lock(&folder));
    assert!(vault.unlocked_folders().is_empty());
    assert_eq!(tree_names(&vault), ["Public.md"]);
    assert!(vault.read_note(&note).is_err());
    assert_eq!(vault.resolve(&p("Public.md"), "otter-confession"), None);
    assert_no_leak(dir.path());

    // Blind: no folder named. Then the recovery key, instead.
    assert_eq!(
        vault.unlock("correct horse battery staple").unwrap(),
        std::slice::from_ref(&folder)
    );
    assert_eq!(
        vault.read_note(&note).unwrap(),
        "edited-after-protecting the quiet albatross"
    );
    let kinds: Vec<(String, orbit_core::EntryKind)> = vault
        .tree()
        .unwrap()
        .into_iter()
        .find(|n| n.path == folder)
        .unwrap()
        .children
        .into_iter()
        .map(|n| (n.name, n.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("Empty Corner".to_string(), orbit_core::EntryKind::Folder),
            ("Hidden Wing".to_string(), orbit_core::EntryKind::Folder),
            (
                "otter-confession.md".to_string(),
                orbit_core::EntryKind::Note
            ),
        ]
    );
    assert_eq!(vault.lock_all(), std::slice::from_ref(&folder));
    assert_no_leak(dir.path());
    assert_eq!(
        vault.unlock_with_recovery_key(&recovery).unwrap(),
        std::slice::from_ref(&folder)
    );
    assert!(
        vault
            .read_note(&p("Zanzibar Diary/Hidden Wing/Kestrel Plans.md"))
            .is_ok()
    );
    vault.lock_all();
    assert_no_leak(dir.path());

    // And after reopening the vault.
    drop(vault);
    let vault = Vault::open(dir.path()).unwrap();
    assert!(!tree_names(&vault).contains(&"Zanzibar Diary".to_string()));
    vault.unlock("correct horse battery staple").unwrap();
    assert_eq!(
        vault.read_note(&note).unwrap(),
        "edited-after-protecting the quiet albatross"
    );
}

#[test]
fn one_passphrase_opens_every_folder_it_protects_and_only_those() {
    let dir = tempfile::tempdir().unwrap();
    for folder in ["A", "B", "C"] {
        fs::create_dir_all(dir.path().join(folder)).unwrap();
        fs::write(dir.path().join(folder).join("n.md"), folder).unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    vault.set_kdf_params(KdfParams::INSECURE_TEST);
    for (folder, passphrase) in [
        ("A", "shared words here"),
        ("B", "shared words here"),
        ("C", "other"),
    ] {
        let keys = vault.prepare_protection(passphrase).unwrap();
        vault.protect_folder(&p(folder), keys).unwrap();
    }
    vault.lock_all();
    let mut opened = vault.unlock("shared words here").unwrap();
    opened.sort();
    assert_eq!(opened, [p("A"), p("B")]);
    // Already unlocked folders aren't tried again.
    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(vault.unlock("other").unwrap(), [p("C")]);
}

#[test]
fn wrong_attempts_wait_longer_each_time() {
    let (_dir, vault) = vault_with_secrets();
    let keys = vault.prepare_protection("right").unwrap();
    vault.protect_folder(&p("Zanzibar Diary"), keys).unwrap();
    vault.lock_all();

    let first = match vault.unlock("wrong") {
        Err(UnlockError::Wrong { retry_in }) => retry_in,
        other => panic!("{other:?}"),
    };
    assert!(first > Duration::ZERO);
    // Too soon: not even tried.
    assert!(matches!(
        vault.unlock("right"),
        Err(UnlockError::TooSoon(_))
    ));
    std::thread::sleep(first);
    let second = match vault.unlock_with_recovery_key("0000-0000-0000-0000-0000-0000-0000-0000") {
        Err(UnlockError::Wrong { retry_in }) => retry_in,
        other => panic!("{other:?}"),
    };
    assert!(second > first);
    std::thread::sleep(second);
    assert_eq!(vault.unlock("right").unwrap(), [p("Zanzibar Diary")]);
    // A vault with nothing protected answers the same: the prompt gives
    // nothing away.
    let empty = tempfile::tempdir().unwrap();
    let vault = Vault::open(empty.path()).unwrap();
    assert!(matches!(
        vault.unlock("anything"),
        Err(UnlockError::Wrong { .. })
    ));
}

#[test]
fn a_protected_folder_never_holds_another() {
    let (_dir, vault) = vault_with_secrets();
    let keys = vault.prepare_protection("one").unwrap();
    vault
        .protect_folder(&p("Zanzibar Diary/Hidden Wing"), keys)
        .unwrap();
    for folder in [
        "Zanzibar Diary",
        "Zanzibar Diary/Hidden Wing",
        "Zanzibar Diary/Hidden Wing/Inner",
    ] {
        let keys = vault.prepare_protection("two").unwrap();
        assert!(
            matches!(
                vault.protect_folder(&p(folder), keys),
                Err(Error::Nested(_)) | Err(Error::NotFound)
            ),
            "{folder}"
        );
    }
    // Not the vault itself, a note, or a tool folder.
    for path in ["", "Public.md", ".vault"] {
        let keys = vault.prepare_protection("three").unwrap();
        assert!(vault.protect_folder(&p(path), keys).is_err(), "{path:?}");
    }
}

#[test]
fn passphrases_are_rated_by_how_hard_they_are_to_guess() {
    use orbit_core::{Strength, passphrase_strength};
    assert_eq!(passphrase_strength(""), Strength::Weak);
    assert_eq!(passphrase_strength("hunter2"), Strength::Weak);
    assert_eq!(passphrase_strength("Tr0ub4dor&3x!"), Strength::Fair);
    assert_eq!(passphrase_strength("orbit lantern"), Strength::Weak);
    assert_eq!(passphrase_strength("orbit lantern meadow"), Strength::Fair);
    assert_eq!(
        passphrase_strength("correct horse battery staple"),
        Strength::Strong
    );
    assert_eq!(
        passphrase_strength("aaaaaaaaaaaaaaaaaaaaaaaa"),
        Strength::Weak
    );
}

#[test]
fn a_locked_folders_notes_are_never_written_in_plain_text() {
    let (dir, vault) = vault_with_secrets();
    let folder = p("Zanzibar Diary");
    let keys = vault.prepare_protection("pass").unwrap();
    vault.protect_folder(&folder, keys).unwrap();
    vault.lock(&folder);
    // As a save that lands after the lock would.
    assert!(
        vault
            .save_note(
                &p("Zanzibar Diary/otter-confession.md"),
                "the quiet albatross"
            )
            .is_err()
    );
    assert!(
        !dir.path()
            .join("Zanzibar Diary/otter-confession.md")
            .exists()
    );
    assert_no_leak(dir.path());
}

#[test]
fn a_session_never_keeps_a_protected_note() {
    use orbit_core::{Session, SessionTab};
    let (dir, vault) = vault_with_secrets();
    let note = p("Zanzibar Diary/otter-confession.md");
    let tab = |path: &VaultPath| SessionTab {
        path: path.clone(),
        scroll: 0.,
    };
    // Open before it was protected, then while it's unlocked.
    vault
        .save_session(&Session {
            tabs: vec![tab(&note), tab(&p("Public.md"))],
            active: Some(0),
        })
        .unwrap();
    let keys = vault.prepare_protection("pass").unwrap();
    vault.protect_folder(&p("Zanzibar Diary"), keys).unwrap();
    vault
        .save_session(&Session {
            tabs: vec![tab(&note), tab(&p("Public.md"))],
            active: Some(0),
        })
        .unwrap();
    vault.lock_all();
    assert_no_leak(dir.path());
    let restored: Vec<VaultPath> = vault.session().tabs.into_iter().map(|t| t.path).collect();
    assert_eq!(restored, [p("Public.md")]);
}

#[test]
fn which_folders_can_be_protected_is_known_before_asking_for_a_passphrase() {
    let (_dir, vault) = vault_with_secrets();
    let keys = vault.prepare_protection("pass").unwrap();
    vault
        .protect_folder(&p("Zanzibar Diary/Hidden Wing"), keys)
        .unwrap();
    assert!(vault.can_protect(&p("Zanzibar Diary/Empty Corner")).is_ok());
    for folder in [
        "Zanzibar Diary",
        "Zanzibar Diary/Hidden Wing",
        "Public.md",
        "",
    ] {
        assert!(vault.can_protect(&p(folder)).is_err(), "{folder:?}");
    }
    assert_eq!(
        vault.protectable_folders(),
        [p("Zanzibar Diary/Empty Corner")]
    );
    // Still refused once the inner folder is locked.
    vault.lock_all();
    assert!(matches!(
        vault.can_protect(&p("Zanzibar Diary")),
        Err(Error::Nested(_))
    ));
}
