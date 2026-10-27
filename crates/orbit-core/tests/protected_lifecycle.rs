//! Protected folders over time, through the Vault facade: a new passphrase
//! or recovery key, removing protection, moving notes in and out, and a
//! plain folder made where a locked one is. After each, a leak check reads
//! every byte under the vault folder for the protected text.

use std::{collections::BTreeSet, fs, path::Path, time::Duration};

use orbit_core::{Error, UnprotectedCopy, Vault, VaultPath};
use orbit_crypto::KdfParams;

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

const PASSPHRASE: &str = "old harbour lantern rope";

/// Only the protected folder knows these.
const SECRETS: &[&str] = &[
    "Zanzibar Diary",
    "otter-confession",
    "the quiet albatross",
    "lagoon-map",
    "kestrel at dawn",
    "Hidden Wing",
];

/// A vault with `Zanzibar Diary` protected and unlocked, and its recovery
/// key.
fn protected() -> (tempfile::TempDir, Vault, String) {
    let dir = tempfile::tempdir().unwrap();
    let diary = dir.path().join("Zanzibar Diary");
    fs::create_dir_all(&diary).unwrap();
    fs::write(diary.join("otter-confession.md"), "the quiet albatross").unwrap();
    fs::write(dir.path().join("Public.md"), "Nothing to hide.").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    vault.set_kdf_params(KdfParams::INSECURE_TEST);
    let keys = vault.prepare_protection(PASSPHRASE).unwrap();
    let recovery = keys.recovery_key().to_string();
    vault.protect_folder(&p("Zanzibar Diary"), keys).unwrap();
    (dir, vault, recovery)
}

/// Every byte of every file under `dir`, and every name, searched for
/// each secret.
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

/// The names of the blobs stored for every protected folder.
fn blobs(dir: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for store in fs::read_dir(dir.join(".s")).unwrap().flatten() {
        for blob in fs::read_dir(store.path().join("blobs")).unwrap().flatten() {
            names.insert(blob.file_name().to_string_lossy().into_owned());
        }
    }
    names
}

/// Waits out the pause after a wrong unlock attempt.
fn after_a_wrong_attempt() {
    std::thread::sleep(Duration::from_millis(1100));
}

#[test]
fn a_new_passphrase_rewraps_the_key_and_nothing_is_re_encrypted() {
    let (dir, vault, recovery) = protected();
    let folder = p("Zanzibar Diary");
    let before = blobs(dir.path());
    vault
        .change_passphrase(&folder, "new meadow kettle stone")
        .unwrap();
    assert_eq!(blobs(dir.path()), before, "no note re-encrypted");

    vault.lock_all();
    assert_no_leak(dir.path());
    assert!(vault.unlock(PASSPHRASE).is_err(), "the old one is gone");
    after_a_wrong_attempt();
    assert_eq!(
        vault.unlock("new meadow kettle stone").unwrap(),
        std::slice::from_ref(&folder)
    );
    vault.lock_all();
    assert!(
        vault.unlock_with_recovery_key(&recovery).is_ok(),
        "the recovery key still works"
    );
}

#[test]
fn a_new_recovery_key_replaces_the_old_one() {
    let (dir, vault, old) = protected();
    let folder = p("Zanzibar Diary");
    let new = vault.new_recovery_key(&folder).unwrap().to_string();
    assert_ne!(new, old);

    vault.lock_all();
    assert_no_leak(dir.path());
    assert!(vault.unlock_with_recovery_key(&old).is_err());
    after_a_wrong_attempt();
    assert!(vault.unlock_with_recovery_key(&new).is_ok());
    vault.lock_all();
    assert!(
        vault.unlock(PASSPHRASE).is_ok(),
        "the passphrase still works"
    );
}

#[test]
fn managing_a_folder_needs_it_unlocked() {
    let (dir, vault, _) = protected();
    let folder = p("Zanzibar Diary");
    vault.lock_all();
    assert!(matches!(
        vault.change_passphrase(&folder, "new meadow kettle stone"),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        vault.new_recovery_key(&folder),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        vault.remove_protection(&folder),
        Err(Error::NotFound)
    ));
    // Nor is a plain folder managed as one.
    assert!(matches!(
        vault.change_passphrase(&p("Public.md"), "new meadow kettle stone"),
        Err(Error::NotFound)
    ));
    assert_no_leak(dir.path());
}

#[test]
fn removing_protection_leaves_a_plain_folder_and_nothing_encrypted() {
    let (dir, vault, _) = protected();
    let folder = p("Zanzibar Diary");
    vault
        .save_note(
            &p("Zanzibar Diary/otter-confession.md"),
            "edited while protected",
        )
        .unwrap();
    vault.create_folder(&folder).unwrap();
    vault.remove_protection(&folder).unwrap();

    assert!(vault.unlocked_folders().is_empty());
    assert_eq!(
        fs::read_to_string(dir.path().join("Zanzibar Diary/otter-confession.md")).unwrap(),
        "edited while protected"
    );
    assert!(dir.path().join("Zanzibar Diary/Untitled").is_dir());
    assert!(blobs(dir.path()).is_empty(), "its storage is gone");
    // A plain folder now: it stays when everything locks.
    assert!(vault.lock_all().is_empty());
    assert_eq!(
        vault
            .read_note(&p("Zanzibar Diary/otter-confession.md"))
            .unwrap(),
        "edited while protected"
    );
}

#[test]
fn moving_notes_and_folders_in_encrypts_them_and_deletes_the_plain_copies() {
    let (dir, vault, _) = protected();
    let folder = p("Zanzibar Diary");
    fs::write(dir.path().join("lagoon-map.md"), "kestrel at dawn").unwrap();
    fs::create_dir_all(dir.path().join("Hidden Wing/Deeper")).unwrap();
    fs::write(
        dir.path().join("Hidden Wing/Deeper/Note.md"),
        "kestrel at dawn",
    )
    .unwrap();

    let note = vault.move_to(&p("lagoon-map.md"), &folder).unwrap();
    assert_eq!(note, p("Zanzibar Diary/lagoon-map.md"));
    let wing = vault.move_to(&p("Hidden Wing"), &folder).unwrap();
    assert_eq!(wing, p("Zanzibar Diary/Hidden Wing"));
    assert!(!dir.path().join("lagoon-map.md").exists());
    assert!(!dir.path().join("Hidden Wing").exists());
    assert_eq!(vault.read_note(&note).unwrap(), "kestrel at dawn");
    assert_eq!(
        vault
            .read_note(&p("Zanzibar Diary/Hidden Wing/Deeper/Note.md"))
            .unwrap(),
        "kestrel at dawn"
    );

    vault.lock_all();
    assert_no_leak(dir.path());
    vault.unlock(PASSPHRASE).unwrap();
    assert_eq!(vault.read_note(&note).unwrap(), "kestrel at dawn");
}

#[test]
fn moving_a_note_out_writes_it_in_plain_text() {
    let (dir, vault, _) = protected();
    let note = p("Zanzibar Diary/otter-confession.md");
    let before = blobs(dir.path()).len();
    let moved = vault.move_to(&note, &VaultPath::root()).unwrap();
    assert_eq!(moved, p("otter-confession.md"));
    assert_eq!(
        fs::read_to_string(dir.path().join("otter-confession.md")).unwrap(),
        "the quiet albatross"
    );
    assert_eq!(blobs(dir.path()).len(), before - 1);
    vault.lock_all();
    assert_eq!(vault.read_note(&moved).unwrap(), "the quiet albatross");
    assert!(vault.read_note(&note).is_err());
}

#[test]
fn a_move_never_replaces_what_is_there() {
    let (dir, vault, _) = protected();
    fs::write(dir.path().join("otter-confession.md"), "plain").unwrap();
    assert!(matches!(
        vault.move_to(&p("Zanzibar Diary/otter-confession.md"), &VaultPath::root()),
        Err(Error::AlreadyExists(_))
    ));
    assert!(matches!(
        vault.move_to(&p("otter-confession.md"), &p("Zanzibar Diary")),
        Err(Error::AlreadyExists(_))
    ));
    assert_eq!(
        fs::read_to_string(dir.path().join("otter-confession.md")).unwrap(),
        "plain"
    );
    assert_eq!(
        vault
            .read_note(&p("Zanzibar Diary/otter-confession.md"))
            .unwrap(),
        "the quiet albatross"
    );
}

/// Locks the vault's protected folder, then makes a plain folder at its
/// path with a note in it, as another app (or a sync) might.
fn plain_folder_where_the_locked_one_is(dir: &Path, vault: &Vault) {
    vault.lock_all();
    let plain = dir.join("Zanzibar Diary");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join("stray.md"), "made while locked").unwrap();
    vault.unlock(PASSPHRASE).unwrap();
}

#[test]
fn a_plain_folder_at_a_locked_folders_path_becomes_an_unprotected_copy() {
    let (dir, vault, _) = protected();
    plain_folder_where_the_locked_one_is(dir.path(), &vault);
    let (folder, copy) = (p("Zanzibar Diary"), p("Zanzibar Diary (unprotected copy)"));
    assert_eq!(
        vault.unprotected_copies(),
        [UnprotectedCopy {
            copy: copy.clone(),
            folder: folder.clone()
        }]
    );
    assert_eq!(
        vault.read_note(&copy.join("stray.md").unwrap()).unwrap(),
        "made while locked"
    );
    assert_eq!(
        vault
            .read_note(&p("Zanzibar Diary/otter-confession.md"))
            .unwrap(),
        "the quiet albatross"
    );

    // Its contents move in: encrypted, and the copy is gone.
    assert!(vault.merge_unprotected_copy(&copy).unwrap().is_empty());
    assert!(vault.unprotected_copies().is_empty());
    assert!(
        !dir.path()
            .join("Zanzibar Diary (unprotected copy)")
            .exists()
    );
    assert_eq!(
        vault.read_note(&p("Zanzibar Diary/stray.md")).unwrap(),
        "made while locked"
    );
    vault.lock_all();
    assert_no_leak(dir.path());
    assert!(!dir.path().join("Zanzibar Diary").exists());
}

#[test]
fn an_unprotected_copy_can_be_kept_separate() {
    let (dir, vault, _) = protected();
    plain_folder_where_the_locked_one_is(dir.path(), &vault);
    let copy = p("Zanzibar Diary (unprotected copy)");
    vault.keep_unprotected_copy(&copy);
    assert!(vault.unprotected_copies().is_empty());
    vault.lock_all();
    vault.unlock(PASSPHRASE).unwrap();
    assert!(vault.unprotected_copies().is_empty(), "asked once");
    assert_eq!(
        fs::read_to_string(
            dir.path()
                .join("Zanzibar Diary (unprotected copy)/stray.md")
        )
        .unwrap(),
        "made while locked"
    );
}

/// Protects `Second` too, with the same passphrase, holding a note named
/// like the first folder's.
fn second_protected(dir: &Path, vault: &Vault) -> VaultPath {
    let second = dir.join("Second");
    fs::create_dir_all(&second).unwrap();
    fs::write(second.join("kept.md"), "kestrel at dawn").unwrap();
    let keys = vault.prepare_protection(PASSPHRASE).unwrap();
    vault.protect_folder(&p("Second"), keys).unwrap();
    p("Second")
}

#[test]
fn a_note_moves_between_protected_folders_without_touching_plain_text() {
    let (dir, vault, _) = protected();
    let second = second_protected(dir.path(), &vault);
    let moved = vault
        .move_to(&p("Zanzibar Diary/otter-confession.md"), &second)
        .unwrap();
    assert_eq!(moved, p("Second/otter-confession.md"));
    vault.lock_all();
    assert_no_leak(dir.path());
    vault.unlock(PASSPHRASE).unwrap();
    assert_eq!(vault.read_note(&moved).unwrap(), "the quiet albatross");
}

#[test]
fn nothing_moves_out_onto_a_locked_folders_note() {
    let (dir, vault, _) = protected();
    let second = second_protected(dir.path(), &vault);
    vault.lock(&second);
    // A plain folder where the locked one is: its note's path is refused.
    fs::create_dir(dir.path().join("Second")).unwrap();
    vault
        .rename(&p("Zanzibar Diary/otter-confession.md"), "kept")
        .unwrap();
    assert!(matches!(
        vault.move_to(&p("Zanzibar Diary/kept.md"), &second),
        Err(Error::AlreadyExists(_))
    ));
    vault.lock_all();
    assert_no_leak(dir.path());
}

#[test]
fn removing_protection_never_replaces_what_is_at_its_place() {
    let (dir, vault, _) = protected();
    let folder = p("Zanzibar Diary");
    // Made at the protected folder's place by another app, unseen by Orbit.
    fs::write(dir.path().join("Zanzibar Diary"), "plain").unwrap();
    assert!(matches!(
        vault.remove_protection(&folder),
        Err(Error::AlreadyExists(_))
    ));
    assert_eq!(vault.unlocked_folders(), std::slice::from_ref(&folder));
    fs::remove_file(dir.path().join("Zanzibar Diary")).unwrap();
    vault.lock_all();
    assert_no_leak(dir.path());
}

#[test]
fn a_second_copy_gets_a_name_of_its_own_and_questions_go_when_the_folder_locks() {
    let (dir, vault, _) = protected();
    fs::create_dir_all(dir.path().join("Zanzibar Diary (unprotected copy)")).unwrap();
    plain_folder_where_the_locked_one_is(dir.path(), &vault);
    let copy = p("Zanzibar Diary (unprotected copy 2)");
    assert_eq!(
        vault
            .unprotected_copies()
            .into_iter()
            .map(|c| c.copy)
            .collect::<Vec<_>>(),
        std::slice::from_ref(&copy)
    );
    // Search sees it under its new name.
    vault.index_search();
    assert!(
        vault
            .search("made while locked", 10)
            .results
            .iter()
            .any(|r| r.path.is_within(&copy))
    );
    vault.lock_all();
    assert!(
        vault.unprotected_copies().is_empty(),
        "nothing to move into"
    );
    assert!(matches!(
        vault.merge_unprotected_copy(&copy),
        Err(Error::NotFound)
    ));
}
