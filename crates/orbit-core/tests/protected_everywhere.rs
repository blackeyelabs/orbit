//! While unlocked, protected notes are found everywhere a plain note is:
//! Vault search, the Quick switcher, link resolution, backlinks and tags.
//! Locked, every trace is gone, and links into the folder look like any
//! unresolved link. Nothing of them reaches the on-disk search cache.

use std::{fs, path::Path};

use orbit_core::{Vault, VaultPath};
use orbit_crypto::KdfParams;

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

const SECRETS: &[&str] = &["Quokka Ledger", "marmalade-heist", "zeppelin"];

fn vault() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in [
        ("Public.md", "Plain note."),
        (
            "Vault Room/Quokka Ledger.md",
            "---\naliases: [Ledger of Quokkas]\n---\nThe zeppelin plan, see [[Public]]. #marmalade-heist\n",
        ),
    ] {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    vault.set_kdf_params(KdfParams::INSECURE_TEST);
    let keys = vault.prepare_protection("pass").unwrap();
    vault.protect_folder(&p("Vault Room"), keys).unwrap();
    vault.index_search();
    (dir, vault)
}

/// Every question that could show the protected note, answered.
struct Seen {
    search: bool,
    switcher: bool,
    alias: bool,
    resolved: bool,
    backlink: bool,
    tag: bool,
}

fn seen(vault: &Vault) -> Seen {
    let public = p("Public.md");
    let ledger = p("Vault Room/Quokka Ledger.md");
    Seen {
        search: vault
            .search("zeppelin", 10)
            .results
            .iter()
            .any(|r| r.path == ledger),
        switcher: vault
            .find_notes("quokka", 10)
            .iter()
            .any(|m| m.path == ledger),
        alias: vault
            .find_notes("ledger of", 10)
            .iter()
            .any(|m| m.path == ledger),
        resolved: vault.resolve(&public, "Quokka Ledger") == Some(ledger.clone()),
        backlink: vault.backlinks(&public).iter().any(|b| b.from == ledger),
        tag: vault.tags().iter().any(|t| t.name == "marmalade-heist"),
    }
}

fn assert_seen_everywhere(vault: &Vault, expected: bool) {
    let s = seen(vault);
    for (what, got) in [
        ("search", s.search),
        ("switcher", s.switcher),
        ("alias", s.alias),
        ("link", s.resolved),
        ("backlink", s.backlink),
        ("tag", s.tag),
    ] {
        assert_eq!(got, expected, "{what}");
    }
}

fn assert_no_leak(dir: &Path) {
    fn walk(dir: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let bytes = if path.is_dir() {
                walk(&path, found);
                continue;
            } else {
                fs::read(&path).unwrap()
            };
            for secret in SECRETS {
                for form in [secret.to_string(), secret.to_lowercase()] {
                    if bytes.windows(form.len()).any(|w| w == form.as_bytes()) {
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

#[test]
fn unlocked_protected_notes_are_found_everywhere_and_locked_ones_nowhere() {
    let (dir, vault) = vault();
    // Just protected, it's unlocked.
    assert_seen_everywhere(&vault, true);
    vault.lock_all();
    assert_seen_everywhere(&vault, false);
    // A link into the locked folder is like any unresolved link.
    assert_eq!(vault.resolve(&p("Public.md"), "Quokka Ledger"), None);
    assert_no_leak(dir.path());

    vault.unlock("pass").unwrap();
    assert_seen_everywhere(&vault, true);
    // Edits show in search at once.
    vault
        .save_note(&p("Vault Room/Quokka Ledger.md"), "A new marmalade recipe")
        .unwrap();
    assert!(
        vault
            .search("recipe", 10)
            .results
            .iter()
            .any(|r| r.title == "Quokka Ledger")
    );
    assert!(vault.search("zeppelin", 10).results.is_empty());
    vault.index_search();
    assert_no_leak(dir.path());

    vault.lock_all();
    assert!(vault.search("recipe", 10).results.is_empty());
    assert!(vault.search("marmalade", 10).results.is_empty());
    assert_no_leak(dir.path());
}

#[test]
fn search_finds_plain_and_protected_notes_together() {
    let (_dir, vault) = vault();
    vault
        .save_note(&p("Zeppelins.md"), "zeppelin zeppelin zeppelin")
        .unwrap();
    let found = vault.search("zeppelin", 10);
    let mut paths: Vec<&str> = found.results.iter().map(|r| r.path.as_str()).collect();
    // Scores from separate indexes aren't comparable: the order across them
    // is only rough, so only what's found is checked.
    paths.sort();
    assert_eq!(paths, ["Vault Room/Quokka Ledger.md", "Zeppelins.md"]);
    assert_eq!(found.matching_notes, 2);
    assert_eq!(vault.search("zeppelin", 1).results.len(), 1);
}

#[test]
fn before_any_unlock_nothing_of_a_protected_folder_is_found() {
    let (dir, vault) = vault();
    vault.lock_all();
    drop(vault);
    // Reopened, the folder locked, the search cache already on disk.
    let vault = Vault::open(dir.path()).unwrap();
    vault.index_search();
    assert_seen_everywhere(&vault, false);
    assert!(vault.notes_tagged("marmalade-heist").is_empty());
    let suggested = vault.link_suggestions(&p("Public.md"), "quokka", 10);
    assert!(suggested.notes.is_empty());
    assert_eq!(suggested.new_note.as_deref(), Some("quokka"));
    assert_no_leak(dir.path());

    vault.unlock("pass").unwrap();
    assert_seen_everywhere(&vault, true);
    assert_eq!(
        vault.notes_tagged("marmalade-heist"),
        [p("Vault Room/Quokka Ledger.md")]
    );
    assert_eq!(
        vault.link_suggestions(&p("Public.md"), "quokka", 10).notes[0].path,
        p("Vault Room/Quokka Ledger.md")
    );
}

#[test]
fn a_link_into_a_locked_folder_is_like_any_unresolved_link() {
    let (dir, vault) = vault();
    vault.lock_all();
    let public = p("Public.md");
    assert_eq!(vault.resolve(&public, "Vault Room/Quokka Ledger"), None);
    // Following it creates a plain note, as for any unresolved link.
    let followed = vault.follow_link(&public, "Vault Room/New idea").unwrap();
    assert!(followed.created);
    assert_eq!(followed.path, p("Vault Room/New idea.md"));
    assert!(dir.path().join("Vault Room/New idea.md").is_file());
    assert_no_leak(dir.path());
}

#[test]
fn a_renamed_or_deleted_protected_folder_leaves_no_search_behind() {
    let (_dir, vault) = vault();
    let renamed = vault.rename(&p("Vault Room"), "Strong Room").unwrap();
    assert_eq!(
        vault.search("zeppelin", 10).results[0].path,
        p("Strong Room/Quokka Ledger.md")
    );
    vault.lock_all();
    assert!(vault.search("zeppelin", 10).results.is_empty());
    vault.unlock("pass").unwrap();
    vault.delete(&renamed).unwrap();
    assert!(vault.search("zeppelin", 10).results.is_empty());
}
