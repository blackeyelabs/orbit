//! What a vault remembers between runs: its open tabs (session state under
//! `.vault/`), that it's open, and the app-wide list of recent vaults.

use std::fs;

use orbit_core::{Error, RecentVaults, Session, SessionTab, Vault, VaultPath};

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

fn tab(path: &str, scroll: f32) -> SessionTab {
    SessionTab {
        path: p(path),
        scroll,
    }
}

fn vault_with(files: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for path in files {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "text").unwrap();
    }
    dir
}

#[test]
fn a_vault_opened_for_the_first_time_has_no_session() {
    let dir = vault_with(&["a.md"]);
    let vault = Vault::open(dir.path()).unwrap();
    assert_eq!(vault.session(), Session::default());
}

#[test]
fn tabs_the_active_note_and_scroll_positions_come_back_next_time() {
    let dir = vault_with(&["a.md", "Inbox/b.md", "notes.txt"]);
    let session = Session {
        tabs: vec![
            tab("a.md", 0.0),
            tab("Inbox/b.md", 412.5),
            tab("notes.txt", 30.0),
        ],
        active: Some(1),
    };
    Vault::open(dir.path())
        .unwrap()
        .save_session(&session)
        .unwrap();

    let reopened = Vault::open(dir.path()).unwrap();
    assert_eq!(reopened.session(), session);
    // The session lives with the vault, out of the tree.
    assert!(dir.path().join(".vault").is_dir());
    assert!(reopened.tree().unwrap().iter().all(|n| n.name != ".vault"));
}

#[test]
fn notes_gone_since_last_time_are_left_out_and_the_active_tab_follows() {
    let dir = vault_with(&["a.md", "b.md", "c.md"]);
    let session = Session {
        tabs: vec![tab("a.md", 1.0), tab("b.md", 2.0), tab("c.md", 3.0)],
        active: Some(2),
    };
    Vault::open(dir.path())
        .unwrap()
        .save_session(&session)
        .unwrap();
    fs::remove_file(dir.path().join("a.md")).unwrap();

    let restored = Vault::open(dir.path()).unwrap().session();
    assert_eq!(restored.tabs, [tab("b.md", 2.0), tab("c.md", 3.0)]);
    assert_eq!(restored.active, Some(1));

    // The active note itself gone: the tab beside it is active instead.
    fs::remove_file(dir.path().join("c.md")).unwrap();
    let restored = Vault::open(dir.path()).unwrap().session();
    assert_eq!(restored.tabs, [tab("b.md", 2.0)]);
    assert_eq!(restored.active, Some(0));
}

#[test]
fn only_notes_are_remembered() {
    let dir = vault_with(&["a.md", "photo.png", ".obsidian/app.md"]);
    let session = Session {
        tabs: vec![
            tab("photo.png", 0.0),
            tab(".obsidian/app.md", 0.0),
            tab("a.md", 5.0),
        ],
        active: Some(2),
    };
    let vault = Vault::open(dir.path()).unwrap();
    vault.save_session(&session).unwrap();
    assert_eq!(
        vault.session(),
        Session {
            tabs: vec![tab("a.md", 5.0)],
            active: Some(0),
        }
    );
}

#[test]
fn an_unreadable_session_means_starting_fresh() {
    let dir = vault_with(&["a.md"]);
    fs::create_dir_all(dir.path().join(".vault")).unwrap();
    fs::write(dir.path().join(".vault/session.json"), "{ not json").unwrap();
    assert_eq!(
        Vault::open(dir.path()).unwrap().session(),
        Session::default()
    );
}

#[test]
fn a_vault_already_open_is_refused_until_it_closes() {
    let dir = vault_with(&["a.md"]);
    let first = Vault::open(dir.path()).unwrap();

    assert!(matches!(Vault::open(dir.path()), Err(Error::AlreadyOpen)));
    // However the folder is named.
    let alias = dir.path().join("..").join(dir.path().file_name().unwrap());
    assert!(matches!(Vault::open(&alias), Err(Error::AlreadyOpen)));

    drop(first);
    assert!(Vault::open(dir.path()).is_ok());
}

#[test]
fn recent_vaults_list_roots_newest_first_and_flag_ones_that_moved() {
    let config = tempfile::tempdir().unwrap();
    let (work, home) = (vault_with(&[]), vault_with(&[]));
    let gone = vault_with(&[]);
    let gone_path = gone.path().to_path_buf();

    let mut recent = RecentVaults::load(config.path());
    assert!(recent.vaults().is_empty());
    recent.add(gone.path());
    recent.add(work.path());
    recent.add(home.path());
    // Opening one again moves it to the top rather than listing it twice.
    recent.add(work.path());
    recent.save(config.path()).unwrap();
    drop(gone);

    let recent = RecentVaults::load(config.path());
    let listed: Vec<_> = recent
        .vaults()
        .into_iter()
        .map(|v| (v.path, v.found))
        .collect();
    assert_eq!(
        listed,
        [
            (work.path().canonicalize().unwrap(), true),
            (home.path().canonicalize().unwrap(), true),
            (gone_path.canonicalize().unwrap_or(gone_path), false),
        ]
    );
    let names: Vec<_> = recent.vaults().into_iter().map(|v| v.name).collect();
    assert_eq!(names[0], work.path().file_name().unwrap().to_str().unwrap());
}

#[test]
fn a_recent_vault_that_moved_can_be_located_in_its_new_place() {
    let config = tempfile::tempdir().unwrap();
    let (old, other) = (vault_with(&[]), vault_with(&[]));
    let old_path = old.path().canonicalize().unwrap();
    let mut recent = RecentVaults::default();
    recent.add(&old_path);
    recent.add(other.path());
    let moved = tempfile::tempdir().unwrap();

    recent.replace(&old_path, moved.path());
    recent.save(config.path()).unwrap();

    let paths: Vec<_> = RecentVaults::load(config.path())
        .vaults()
        .into_iter()
        .map(|v| v.path)
        .collect();
    assert_eq!(
        paths,
        [
            moved.path().canonicalize().unwrap(),
            other.path().canonicalize().unwrap()
        ]
    );
}
