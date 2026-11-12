//! Property types set by hand: saved for the vault in `.vault/types.json`
//! as Obsidian's `types.json` holds them, and used in every note. One set
//! in a protected note is saved in that folder's encrypted storage and
//! used only while it's unlocked (ADR 0010).

use std::{fs, path::Path};

use md_engine::PropertyType;
use orbit_core::{Vault, VaultPath};
use orbit_crypto::KdfParams;

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

/// A vault of these notes.
fn vault_of(notes: &[&str]) -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    for path in notes {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "Body").unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    (dir, vault)
}

#[test]
fn a_type_set_by_hand_is_saved_for_the_vault_and_used_in_every_note() {
    let (dir, vault) = vault_of(&["A.md", "Sub/B.md"]);
    let before = vault.property_types_revision();
    vault
        .set_property_type(&p("A.md"), "Count", PropertyType::Number)
        .unwrap();
    assert_ne!(vault.property_types_revision(), before, "panels redraw");
    assert_eq!(
        vault.property_types(&p("Sub/B.md")).get("count"),
        Some(PropertyType::Number),
        "in another note, ignoring case"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(".vault/types.json")).unwrap(),
        "{\n  \"types\": {\n    \"Count\": \"number\"\n  }\n}"
    );
    // Setting it again, in another case, replaces it.
    vault
        .set_property_type(&p("A.md"), "count", PropertyType::Date)
        .unwrap();
    // Obsidian keeps the name it was first set under.
    assert!(
        fs::read_to_string(dir.path().join(".vault/types.json"))
            .unwrap()
            .contains("\"Count\": \"date\"")
    );
    drop(vault);
    let reopened = Vault::open(dir.path()).unwrap();
    assert_eq!(
        reopened.property_types(&p("A.md")).get("COUNT"),
        Some(PropertyType::Date)
    );
}

#[test]
fn nothing_is_saved_until_a_type_is_set_and_others_types_are_kept() {
    let (dir, vault) = vault_of(&["A.md"]);
    assert_eq!(vault.property_types(&p("A.md")).get("x"), None);
    assert!(!dir.path().join(".vault/types.json").exists());
    // A type Orbit doesn't know (a newer Obsidian's) stays as it was.
    fs::create_dir_all(dir.path().join(".vault")).unwrap();
    fs::write(
        dir.path().join(".vault/types.json"),
        r#"{"types":{"mood":"emoji"}}"#,
    )
    .unwrap();
    vault
        .set_property_type(&p("A.md"), "done", PropertyType::Checkbox)
        .unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join(".vault/types.json")).unwrap(),
        "{\n  \"types\": {\n    \"done\": \"checkbox\",\n    \"mood\": \"emoji\"\n  }\n}"
    );
    assert_eq!(vault.property_types(&p("A.md")).get("mood"), None);
}

/// Plaintext on disk anywhere under `dir` holding `secret`.
fn found_on_disk(dir: &Path, secret: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if fs::read(&path)
                .unwrap()
                .windows(secret.len())
                .any(|w| w == secret.as_bytes())
            {
                found.push(path.display().to_string());
            }
        }
    }
    found
}

#[test]
fn a_type_set_in_a_protected_note_stays_in_its_folder_encrypted() {
    let (dir, vault) = vault_of(&["Public.md", "Vault Room/Ledger.md"]);
    vault.set_kdf_params(KdfParams::INSECURE_TEST);
    let keys = vault.prepare_protection("pass").unwrap();
    vault.protect_folder(&p("Vault Room"), keys).unwrap();
    let ledger = p("Vault Room/Ledger.md");
    vault
        .set_property_type(&ledger, "salary-band", PropertyType::Number)
        .unwrap();
    vault
        .set_property_type(&p("Public.md"), "status", PropertyType::Text)
        .unwrap();
    // Used in its folder; outside it, unknown.
    assert_eq!(
        vault.property_types(&ledger).get("salary-band"),
        Some(PropertyType::Number)
    );
    assert_eq!(
        vault.property_types(&ledger).get("status"),
        Some(PropertyType::Text),
        "the vault's types apply inside too"
    );
    assert_eq!(
        vault.property_types(&p("Public.md")).get("salary-band"),
        None
    );
    // Not in the tree, and no key name on disk.
    let tree = format!("{:?}", vault.tree().unwrap());
    assert!(!tree.contains("types.json"), "{tree}");
    assert!(vault.lock(&p("Vault Room")));
    assert_eq!(
        found_on_disk(dir.path(), "salary-band"),
        Vec::<String>::new()
    );
    assert_eq!(
        vault.property_types(&ledger).get("salary-band"),
        None,
        "locked: not used"
    );
    // Unlocked again, it's back.
    vault.unlock("pass").unwrap();
    assert_eq!(
        vault.property_types(&ledger).get("salary-band"),
        Some(PropertyType::Number)
    );
}

/// A vault whose `Vault Room` folder is protected, with a note in it.
fn protected_vault() -> (tempfile::TempDir, Vault) {
    let (dir, vault) = vault_of(&["Public.md", "Vault Room/Ledger.md"]);
    vault.set_kdf_params(KdfParams::INSECURE_TEST);
    let keys = vault.prepare_protection("pass").unwrap();
    vault.protect_folder(&p("Vault Room"), keys).unwrap();
    (dir, vault)
}

#[test]
fn no_type_is_set_for_a_note_in_a_locked_folder() {
    let (dir, vault) = protected_vault();
    assert!(vault.lock(&p("Vault Room")));
    let set = vault.set_property_type(
        &p("Vault Room/Ledger.md"),
        "salary-band",
        PropertyType::Number,
    );
    assert!(set.is_err());
    assert_eq!(
        found_on_disk(dir.path(), "salary-band"),
        Vec::<String>::new()
    );
}

#[test]
fn removing_protection_moves_its_types_to_the_vaults() {
    let (dir, vault) = protected_vault();
    let ledger = p("Vault Room/Ledger.md");
    vault
        .set_property_type(&ledger, "salary-band", PropertyType::Number)
        .unwrap();
    vault.remove_protection(&p("Vault Room")).unwrap();
    assert_eq!(
        vault.property_types(&p("Public.md")).get("salary-band"),
        Some(PropertyType::Number)
    );
    assert!(!dir.path().join("Vault Room/.vault/types.json").exists());
}
