//! The Vault facade over a real folder on disk.

use std::fs;

use orbit_core::{EntryKind, Error, TreeNode, Vault, VaultPath};

/// A vault folder with one of everything.
fn sample_vault() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("Projects/Launch")).unwrap();
    fs::write(root.join("Projects/Orbit roadmap.md"), "# Orbit roadmap\n").unwrap();
    fs::write(root.join("Projects/Launch/plan.txt"), "ship it").unwrap();
    fs::write(root.join("Reading list.md"), "- GPUI").unwrap();
    fs::write(root.join("board.canvas"), "{}").unwrap();
    fs::write(root.join("diagram.png"), [0x89, b'P', b'N', b'G']).unwrap();
    fs::write(root.join("archive.md"), "old").unwrap();
    dir
}

/// Flattens a tree to "indent kind name" lines for easy comparison.
fn outline(nodes: &[TreeNode], depth: usize, out: &mut Vec<String>) {
    for n in nodes {
        let kind = match n.kind {
            EntryKind::Folder => "folder",
            EntryKind::Note => "note",
            EntryKind::Canvas => "canvas",
            EntryKind::Attachment => "attachment",
        };
        out.push(format!("{}{kind} {}", "  ".repeat(depth), n.name));
        outline(&n.children, depth + 1, out);
    }
}

fn outline_of(vault: &Vault) -> Vec<String> {
    let mut out = Vec::new();
    outline(&vault.tree().unwrap(), 0, &mut out);
    out
}

#[test]
fn the_tree_lists_folders_first_then_entries_by_name_with_their_kinds() {
    let dir = sample_vault();
    let vault = Vault::open(dir.path()).unwrap();
    assert_eq!(
        outline_of(&vault),
        [
            "folder Projects",
            "  folder Launch",
            "    note plan.txt",
            "  note Orbit roadmap.md",
            "note archive.md",
            "canvas board.canvas",
            "attachment diagram.png",
            "note Reading list.md",
        ]
    );
}

#[test]
fn tool_folders_are_hidden_wherever_they_are() {
    let dir = sample_vault();
    let root = dir.path();
    for tool in [".vault", ".obsidian", ".git", ".s/abc123"] {
        fs::create_dir_all(root.join(tool)).unwrap();
        fs::write(root.join(tool).join("x.md"), "tool data").unwrap();
    }
    fs::create_dir_all(root.join("Projects/vendored/.git")).unwrap();
    fs::write(root.join(".hidden-note.md"), "still a note").unwrap();

    let lines = outline_of(&Vault::open(root).unwrap());
    for tool in [".vault", ".obsidian", ".git", ".s"] {
        assert!(
            !lines
                .iter()
                .any(|l| l.trim_start().ends_with(&format!(" {tool}"))),
            "{tool} shown: {lines:?}"
        );
    }
    assert!(lines.contains(&"  folder vendored".to_string()));
    assert!(!lines.contains(&"    folder .git".to_string()), "{lines:?}");
    assert!(
        lines.contains(&"note .hidden-note.md".to_string()),
        "{lines:?}"
    );
}

fn p(s: &str) -> VaultPath {
    VaultPath::new(s).unwrap()
}

#[test]
fn a_note_reads_as_its_text() {
    let dir = sample_vault();
    let vault = Vault::open(dir.path()).unwrap();
    assert_eq!(
        vault.read_note(&p("Projects/Orbit roadmap.md")).unwrap(),
        "# Orbit roadmap\n"
    );
    assert_eq!(
        vault.read_note(&p("Projects/Launch/plan.txt")).unwrap(),
        "ship it"
    );
}

#[test]
fn attachments_canvases_and_folders_are_not_read_as_notes() {
    let dir = sample_vault();
    let vault = Vault::open(dir.path()).unwrap();
    for path in ["diagram.png", "board.canvas", "Projects"] {
        assert!(
            matches!(vault.read_note(&p(path)), Err(Error::NotANote(_))),
            "{path}"
        );
    }
}

#[cfg(unix)]
#[test]
fn paths_cannot_leave_the_vault() {
    let dir = sample_vault();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.md"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret.md"), dir.path().join("link.md"))
        .unwrap();

    assert!(VaultPath::new("../secret.md").is_err());
    assert!(VaultPath::new("/etc/passwd").is_err());
    let vault = Vault::open(dir.path()).unwrap();
    assert!(
        vault.read_note(&p("link.md")).is_err(),
        "followed a symlink out of the vault"
    );
    assert!(
        !outline_of(&vault).iter().any(|l| l.contains("link.md")),
        "listed a symlink out of the vault"
    );
}

#[test]
fn opening_a_missing_folder_reports_not_found() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        Vault::open(&dir.path().join("nope")),
        Err(Error::NotFound)
    ));
}

#[cfg(unix)]
#[test]
fn a_symlink_loop_does_not_hang_or_crash_the_tree() {
    let dir = sample_vault();
    std::os::unix::fs::symlink(".", dir.path().join("loop")).unwrap();
    std::os::unix::fs::symlink("..", dir.path().join("Projects/up")).unwrap();
    let lines = outline_of(&Vault::open(dir.path()).unwrap());
    assert!(
        lines.contains(&"note Reading list.md".to_string()),
        "{lines:?}"
    );
    assert!(lines.len() < 50, "loop was followed: {} lines", lines.len());
}

#[cfg(unix)]
#[test]
fn one_odd_or_unreadable_entry_does_not_hide_the_rest_of_the_tree() {
    use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
    let dir = sample_vault();
    let root = dir.path();
    fs::write(root.join("back\\slash.md"), "odd name").unwrap();
    fs::write(
        root.join(std::ffi::OsStr::from_bytes(b"latin1-\xe9.md")),
        "not utf-8",
    )
    .unwrap();
    fs::create_dir(root.join("Locked away")).unwrap();
    fs::set_permissions(root.join("Locked away"), fs::Permissions::from_mode(0o000)).unwrap();

    let result = Vault::open(root).unwrap().tree();
    fs::set_permissions(root.join("Locked away"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut lines = Vec::new();
    outline(&result.expect("tree still loads"), 0, &mut lines);
    assert!(
        lines.contains(&"note Reading list.md".to_string()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"  note Orbit roadmap.md".to_string()),
        "{lines:?}"
    );
}

#[test]
fn tool_folders_are_hidden_regardless_of_case() {
    let dir = sample_vault();
    fs::create_dir_all(dir.path().join(".Obsidian")).unwrap();
    fs::create_dir_all(dir.path().join(".GIT")).unwrap();
    let lines = outline_of(&Vault::open(dir.path()).unwrap());
    assert!(
        !lines
            .iter()
            .any(|l| l.contains(".Obsidian") || l.contains(".GIT")),
        "{lines:?}"
    );
}

#[test]
fn opening_a_file_instead_of_a_folder_reports_not_found() {
    let dir = sample_vault();
    assert!(matches!(
        Vault::open(&dir.path().join("archive.md")),
        Err(Error::NotFound)
    ));
}
