//! `.txt` notes are notes like any other, as plain text: their wikilinks
//! resolve and count as backlinks, and search finds them. `.canvas` files
//! (JSON Canvas, ADR 0005) are listed and searched by their cards' text,
//! but aren't notes to open.

use std::fs;

use orbit_core::{EntryKind, Error, Vault, VaultPath};

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

const BOARD: &str = r##"{
  "nodes": [
    {"id": "a", "type": "text", "x": 0, "y": 0, "width": 200, "height": 80,
     "text": "# Launch plan\n\nShip the **zebra** release first."},
    {"id": "b", "type": "file", "x": 300, "y": 0, "width": 200, "height": 80, "file": "Plan.md"},
    {"id": "c", "type": "group", "x": -20, "y": -20, "width": 600, "height": 200, "label": "Quarter goals"},
    {"id": "d", "type": "link", "x": 0, "y": 300, "width": 200, "height": 80, "url": "https://example.com/zebra"}
  ],
  "edges": [{"id": "e", "fromNode": "a", "toNode": "b", "label": "then"}]
}"##;

fn vault() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in [
        ("Plan.md", "# Plan\n"),
        (
            "Inbox/todo.txt",
            "call about [[Plan]] and the [x](Plan.md) thing\n",
        ),
        ("Boards/Launch.canvas", BOARD),
        ("Boards/Broken.canvas", "{ not json"),
    ] {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    vault.index_search();
    (dir, vault)
}

#[test]
fn a_txt_notes_wikilinks_resolve_count_as_backlinks_and_it_is_searchable() {
    let (_dir, vault) = vault();
    let todo = p("Inbox/todo.txt");
    assert_eq!(vault.resolve(&todo, "Plan"), Some(p("Plan.md")));
    let backlinks = vault.backlinks(&p("Plan.md"));
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[0].from, todo);
    // Plain text: the markdown link is just text.
    assert_eq!(backlinks[0].mentions.len(), 1);
    let found = vault.search("call", 10);
    assert_eq!(found.results[0].path, todo);
    assert_eq!(found.results[0].title, "todo.txt");
}

#[test]
fn a_canvas_is_listed_and_found_by_the_text_of_its_cards() {
    let (_dir, vault) = vault();
    let boards = vault
        .tree()
        .unwrap()
        .into_iter()
        .find(|n| n.name == "Boards")
        .unwrap();
    let kinds: Vec<(&str, EntryKind)> = boards
        .children
        .iter()
        .map(|n| (n.name.as_str(), n.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("Broken.canvas", EntryKind::Canvas),
            ("Launch.canvas", EntryKind::Canvas)
        ]
    );

    let found = vault.search("zebra", 10);
    assert_eq!(
        found.results.len(),
        1,
        "a link card's address isn't card text"
    );
    let launch = &found.results[0];
    assert_eq!(launch.path, p("Boards/Launch.canvas"));
    assert_eq!(launch.title, "Launch.canvas");
    assert_eq!(launch.snippets[0].text, "Ship the **zebra** release first.");
    // Group labels are card text too.
    assert_eq!(vault.search("quarter goals", 10).results.len(), 1);
    // Searchable by name, like a note.
    assert_eq!(
        vault.search("broken", 10).results[0].path,
        p("Boards/Broken.canvas")
    );
}

#[test]
fn a_canvas_is_not_a_note_to_open() {
    let (_dir, vault) = vault();
    assert!(matches!(
        vault.open_note(&p("Boards/Launch.canvas")),
        Err(Error::NotANote(_))
    ));
}

#[test]
fn an_edges_label_is_canvas_text_too() {
    let (_dir, vault) = vault();
    assert_eq!(
        vault.search("then", 10).results[0].path,
        p("Boards/Launch.canvas")
    );
}

#[test]
fn renaming_never_turns_a_note_into_a_canvas_or_back() {
    let (_dir, vault) = vault();
    // The extension typed is kept as part of the name.
    assert_eq!(
        vault.rename(&p("Plan.md"), "Plan.canvas").unwrap(),
        p("Plan.canvas.md")
    );
    assert_eq!(
        vault
            .rename(&p("Boards/Launch.canvas"), "Launch.md")
            .unwrap(),
        p("Boards/Launch.md.canvas")
    );
    // Between the kinds of note is fine.
    assert_eq!(
        vault.rename(&p("Inbox/todo.txt"), "todo.md").unwrap(),
        p("Inbox/todo.md")
    );
}
