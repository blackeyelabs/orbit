//! Logs carry no vault content (ADR 0007). Every event, from every crate
//! and at every level (dependencies' `log` messages included), is
//! captured while a protected folder is protected, searched, edited,
//! locked and unlocked; none of what only the folder knows may appear.
//! Paths go into logs through `Vault::log_path`, which hides protected
//! ones.

use std::{
    fs,
    io::Write,
    sync::{Arc, Mutex},
};

use orbit_core::{Vault, VaultPath, Zeroizing};
use orbit_crypto::KdfParams;

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

/// What only the protected folder knows.
const SECRETS: &[&str] = &[
    "Zanzibar Diary",
    "otter-confession",
    "Kestrel Plans",
    "the quiet albatross",
    "lagoon-map",
    "edited-before-locking",
    "correct horse battery staple",
    "brand new meadow kettle",
];

/// Everything logged, as text.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

/// Captures every event from every thread, for the whole test binary;
/// `log` messages too.
fn capture() -> Captured {
    let captured = Captured::default();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .try_init()
        .expect("one subscriber for this test binary");
    captured
}

fn vault_with_secrets() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in [
        ("Public.md", "Nothing to hide."),
        (
            "Zanzibar Diary/otter-confession.md",
            "# Day one\nthe quiet albatross sang",
        ),
        (
            "Zanzibar Diary/Kestrel Plans.md",
            "Meet at dawn #lagoon-map",
        ),
    ] {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    vault.set_kdf_params(KdfParams::INSECURE_TEST);
    (dir, vault)
}

#[test]
fn nothing_a_protected_folder_holds_is_ever_logged() {
    let captured = capture();
    let (_dir, vault) = vault_with_secrets();
    let folder = p("Zanzibar Diary");
    let note = p("Zanzibar Diary/otter-confession.md");
    vault.index_search();

    let keys = vault
        .prepare_protection("correct horse battery staple")
        .unwrap();
    vault.protect_folder(&folder, keys).unwrap();
    vault.index_search();
    vault.search("albatross", 10);
    vault.save_note(&note, "edited-before-locking").unwrap();
    vault
        .rename(&p("Zanzibar Diary/Kestrel Plans.md"), "Kestrel Plans 2")
        .unwrap();
    vault.move_to(&p("Public.md"), &folder).unwrap();
    // The protected folder itself renamed, and back: neither name logged.
    let renamed = vault.rename(&folder, "Zanzibar Diary Two").unwrap();
    vault.rename(&renamed, "Zanzibar Diary").unwrap();
    vault
        .change_passphrase(&folder, "brand new meadow kettle")
        .unwrap();
    vault.new_recovery_key(&folder).unwrap();
    vault.lock_saving(
        &folder,
        vec![(note.clone(), Zeroizing::new("edited-before-locking".into()))],
    );
    vault.unlock("correct horse battery staple").ok();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    vault.unlock("brand new meadow kettle").unwrap();
    vault.read_note(&note).unwrap();
    vault.lock_all();

    let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(!logged.is_empty(), "something was logged at all");
    let mut found = Vec::new();
    for secret in SECRETS {
        for form in [secret.to_string(), secret.to_lowercase()] {
            if let Some(line) = logged.lines().find(|line| line.contains(&form)) {
                found.push(format!("{form:?} in: {line}"));
            }
        }
    }
    assert!(found.is_empty(), "logged:\n{}", found.join("\n"));
}

#[test]
fn protected_paths_are_hidden_in_logs() {
    let (_dir, vault) = vault_with_secrets();
    let folder = p("Zanzibar Diary");
    let note = p("Zanzibar Diary/otter-confession.md");
    assert_eq!(vault.log_path(&note).to_string(), note.as_str());

    let keys = vault
        .prepare_protection("correct horse battery staple")
        .unwrap();
    vault.protect_folder(&folder, keys).unwrap();
    assert_eq!(vault.log_path(&note).to_string(), "<protected>");
    assert_eq!(vault.log_path(&folder).to_string(), "<protected>");
    assert_eq!(vault.log_path(&p("Public.md")).to_string(), "Public.md");

    vault.lock_all();
    assert_eq!(
        vault.log_path(&note).to_string(),
        "<protected>",
        "locked too"
    );
    assert_eq!(vault.log_path(&folder).to_string(), "<protected>");
}
