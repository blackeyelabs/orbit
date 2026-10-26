//! Creating, renaming and moving notes and folders through the Vault facade.

use std::fs;

use orbit_core::{Error, Vault, VaultPath};

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
fn new_notes_and_folders_get_the_next_free_untitled_name() {
    let (dir, vault) = vault_with(&[("Projects/Untitled.md", "taken")]);

    assert_eq!(
        vault.create_note(&p("Projects")).unwrap(),
        p("Projects/Untitled 1.md")
    );
    assert_eq!(
        vault.create_note(&p("Projects")).unwrap(),
        p("Projects/Untitled 2.md")
    );
    assert_eq!(
        vault.create_note(&VaultPath::root()).unwrap(),
        p("Untitled.md")
    );
    assert_eq!(vault.read_note(&p("Projects/Untitled 1.md")).unwrap(), "");
    assert_eq!(
        vault.read_note(&p("Projects/Untitled.md")).unwrap(),
        "taken"
    );

    assert_eq!(
        vault.create_folder(&p("Projects")).unwrap(),
        p("Projects/Untitled")
    );
    assert_eq!(
        vault.create_folder(&p("Projects")).unwrap(),
        p("Projects/Untitled 1")
    );
    assert!(dir.path().join("Projects/Untitled 1").is_dir());
}

#[test]
fn renaming_keeps_the_contents_and_never_overwrites() {
    let (dir, vault) = vault_with(&[
        ("Projects/draft.md", "my draft"),
        ("Projects/plan.md", "the plan"),
        ("Projects/Launch/notes.md", "launch notes"),
    ]);

    let renamed = vault.rename(&p("Projects/draft.md"), "Roadmap.md").unwrap();
    assert_eq!(renamed, p("Projects/Roadmap.md"));
    assert_eq!(vault.read_note(&renamed).unwrap(), "my draft");
    assert!(!dir.path().join("Projects/draft.md").exists());

    // Folders rename with everything in them.
    let folder = vault.rename(&p("Projects/Launch"), "Release").unwrap();
    assert_eq!(folder, p("Projects/Release"));
    assert_eq!(
        vault.read_note(&p("Projects/Release/notes.md")).unwrap(),
        "launch notes"
    );

    assert!(matches!(
        vault.rename(&p("Projects/Roadmap.md"), "plan.md"),
        Err(Error::AlreadyExists(path)) if path == p("Projects/plan.md")
    ));
    assert_eq!(vault.read_note(&p("Projects/plan.md")).unwrap(), "the plan");
}

#[test]
fn names_with_slashes_dots_or_tool_folder_names_are_refused() {
    let (_dir, vault) = vault_with(&[("note.md", "text"), ("Folder/a.md", "")]);
    for bad in ["", "a/b.md", "..", ".", "back\\slash.md", " "] {
        assert!(
            matches!(vault.rename(&p("note.md"), bad), Err(Error::InvalidName(_))),
            "{bad:?}"
        );
    }
    assert!(matches!(
        vault.rename(&p("Folder"), ".obsidian"),
        Err(Error::InvalidName(_))
    ));
    assert_eq!(vault.read_note(&p("note.md")).unwrap(), "text");
}

#[test]
fn notes_and_folders_move_between_folders_but_never_into_themselves() {
    let (dir, vault) = vault_with(&[
        ("Inbox/idea.md", "an idea"),
        ("Projects/Launch/plan.md", "the plan"),
        ("Archive/idea.md", "older idea"),
    ]);

    let moved = vault
        .move_to(&p("Projects/Launch"), &VaultPath::root())
        .unwrap();
    assert_eq!(moved, p("Launch"));
    assert_eq!(vault.read_note(&p("Launch/plan.md")).unwrap(), "the plan");
    assert!(!dir.path().join("Projects/Launch").exists());

    assert!(matches!(
        vault.move_to(&p("Inbox/idea.md"), &p("Archive")),
        Err(Error::AlreadyExists(path)) if path == p("Archive/idea.md")
    ));
    assert_eq!(
        vault.read_note(&p("Archive/idea.md")).unwrap(),
        "older idea"
    );

    assert!(matches!(
        vault.move_to(&p("Launch"), &p("Launch")),
        Err(Error::IntoItself(_))
    ));
    fs::create_dir(dir.path().join("Launch/Sub")).unwrap();
    assert!(matches!(
        vault.move_to(&p("Launch"), &p("Launch/Sub")),
        Err(Error::IntoItself(_))
    ));
    // `Launch` is a prefix of `Launchpad`, not its parent.
    fs::create_dir(dir.path().join("Launchpad")).unwrap();
    assert_eq!(
        vault.move_to(&p("Launch"), &p("Launchpad")).unwrap(),
        p("Launchpad/Launch")
    );
}

#[test]
fn nothing_moves_into_a_tool_folder() {
    let (_dir, vault) = vault_with(&[("note.md", "secret"), (".obsidian/app.json", "{}")]);
    assert!(vault.move_to(&p("note.md"), &p(".obsidian")).is_err());
    assert_eq!(vault.read_note(&p("note.md")).unwrap(), "secret");
}

#[test]
fn renaming_to_the_same_name_changes_nothing() {
    let (_dir, vault) = vault_with(&[("note.md", "text")]);
    assert_eq!(
        vault.rename(&p("note.md"), "note.md").unwrap(),
        p("note.md")
    );
    assert_eq!(vault.read_note(&p("note.md")).unwrap(), "text");
}

#[test]
fn a_note_keeps_its_extension_unless_a_note_extension_is_typed() {
    let (_dir, vault) = vault_with(&[("draft.md", "a"), ("Notes.md/inside.md", "b")]);

    let renamed = vault.rename(&p("draft.md"), "Roadmap").unwrap();
    assert_eq!(renamed, p("Roadmap.md"));
    let as_text = vault.rename(&renamed, "Roadmap.txt").unwrap();
    assert_eq!(as_text, p("Roadmap.txt"));
    // A folder's name is all name, even when it looks like a note's.
    assert_eq!(
        vault.rename(&p("Notes.md"), "Archive").unwrap(),
        p("Archive")
    );
}

#[test]
fn nothing_inside_a_tool_folder_is_renamed_or_moved() {
    let (_dir, vault) = vault_with(&[(".obsidian/app.json", "{}"), ("Folder/a.md", "")]);
    assert!(vault.rename(&p(".obsidian/app.json"), "b.json").is_err());
    assert!(
        vault
            .move_to(&p(".obsidian/app.json"), &p("Folder"))
            .is_err()
    );
    assert!(vault.rename(&VaultPath::root(), "x").is_err());
}
