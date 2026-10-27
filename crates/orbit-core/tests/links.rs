//! Links resolve like Obsidian's: the fixture vault in `fixtures/links` is
//! Obsidian-shaped (duplicate names, aliases, headings, embeds, unresolved
//! links), and `fixtures/links.obsidian.json` is what real Obsidian answers
//! for it.

use std::fs;

use orbit_core::{Error, VaultPath};
use serde_json::Value;

mod common;
use common::{fixture_vault, fixtures, obsidian};

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

/// Orbit resolves a link no file matches to a note with that alias, where
/// Obsidian leaves it unresolved (front matter aliases count as targets).
fn alias_target(link: &str) -> Option<&'static str> {
    match link {
        "Roadmap" | "The Plan" => Some("Projects/Plan.md"),
        "Old plan" => Some("Archive/Plan.md"),
        _ => None,
    }
}

#[test]
fn links_resolve_as_in_obsidian_including_its_tie_break_for_duplicate_names() {
    let (_dir, vault) = fixture_vault("links");
    let mut mismatches = Vec::new();
    for case in obsidian("links")["resolution"].as_array().unwrap() {
        let (from, link) = (
            case["from"].as_str().unwrap(),
            case["link"].as_str().unwrap(),
        );
        let expected = case["to"].as_str().or(alias_target(link));
        let got = vault.resolve(&p(from), link);
        if got.as_ref().map(VaultPath::as_str) != expected {
            mismatches.push(format!(
                "[[{link}]] from {from}: {got:?}, Obsidian {expected:?}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn links_are_parsed_as_obsidian_parses_them() {
    let dir = fixtures().join("links");
    for (note, expected) in obsidian("links")["parsed"].as_object().unwrap() {
        let source = fs::read_to_string(dir.join(note)).unwrap();
        let got: Vec<Value> = md_engine::links(&source)
            .into_iter()
            .map(|l| {
                serde_json::json!({
                    "link": l.target, "original": &source[l.span.clone()],
                    "displayText": l.display, "start": l.span.start, "end": l.span.end,
                    "embed": l.embed,
                })
            })
            .collect();
        assert_eq!(&Value::Array(got), expected, "{note}");
    }
}

#[test]
fn a_txt_note_has_its_wikilinks_too() {
    let (_dir, vault) = fixture_vault("links");
    let text = vault.read_note(&p("notes.txt")).unwrap();
    let link = &md_engine::links(&text)[0];
    assert_eq!(
        vault.resolve(&p("notes.txt"), link.path()),
        Some(p("Ideas.md"))
    );
}

#[test]
fn following_an_unresolved_link_creates_its_note() {
    let (dir, vault) = fixture_vault("links");
    let from = p("Inbox/Capture.md");
    assert_eq!(vault.resolve(&from, "Missing note"), None);

    // A bare name goes at the vault root, like a new note.
    let created = vault.create_linked_note(&from, "Missing note").unwrap();
    assert_eq!(created, p("Missing note.md"));
    assert_eq!(vault.read_note(&created).unwrap(), "");
    assert_eq!(vault.resolve(&from, "Missing note"), Some(created));

    // A path is kept, with any folders it needs; relative paths start at
    // the linking note's folder.
    assert_eq!(
        vault
            .create_linked_note(&from, "Projects/New/Idea")
            .unwrap(),
        p("Projects/New/Idea.md")
    );
    assert_eq!(
        vault.create_linked_note(&from, "./Draft").unwrap(),
        p("Inbox/Draft.md")
    );
    // A dotted name is a note name, as Obsidian reads it; .txt stays .txt.
    assert_eq!(
        vault.create_linked_note(&from, "v1.2 notes").unwrap(),
        p("v1.2 notes.md")
    );
    assert_eq!(
        vault.create_linked_note(&from, "todo.txt").unwrap(),
        p("todo.txt")
    );
    assert!(dir.path().join("Projects/New/Idea.md").is_file());

    // Never over an existing file, into a tool folder, or out of the vault.
    assert!(matches!(
        vault.create_linked_note(&from, "Ideas"),
        Err(Error::AlreadyExists(_))
    ));
    assert!(vault.create_linked_note(&from, ".obsidian/x").is_err());
    assert!(vault.create_linked_note(&from, "../../outside").is_err());
}

#[test]
fn following_a_link_opens_its_target_at_the_heading_or_creates_the_note() {
    let (dir, vault) = fixture_vault("links");
    let from = p("Home.md");

    let plan = vault.follow_link(&from, "Projects/Plan#Goals").unwrap();
    assert_eq!(plan.path, p("Projects/Plan.md"));
    assert_eq!(plan.heading.as_deref(), Some("Goals"));
    assert!(!plan.created);
    // An alias leads to its note too.
    assert_eq!(
        vault.follow_link(&from, "Roadmap").unwrap().path,
        p("Projects/Plan.md")
    );
    // A heading of the same note.
    assert_eq!(vault.follow_link(&from, "#Risks").unwrap().path, from);

    let missing = vault.follow_link(&from, "Missing note#Later").unwrap();
    assert_eq!(
        (missing.path.as_str(), missing.created),
        ("Missing note.md", true)
    );
    assert!(dir.path().join("Missing note.md").is_file());

    // A missing image is an Attachment to add, not a note to create.
    assert!(vault.follow_link(&from, "missing.png").is_err());
    assert!(!dir.path().join("missing.png.md").exists());
    // Refused before any folder it needs is made.
    assert!(matches!(
        vault.create_linked_note(&from, "Inbox/Ideas"),
        Err(Error::AlreadyExists(_))
    ));
    assert!(vault.create_linked_note(&from, "Fresh/x.png").is_err());
    assert!(!dir.path().join("Fresh").exists());
}

#[test]
fn notes_added_or_removed_by_other_apps_are_found_through_the_watcher() {
    let (dir, vault) = fixture_vault("links");
    let from = p("Home.md");
    assert_eq!(vault.resolve(&from, "Later"), None);
    let (tx, rx) = std::sync::mpsc::channel();
    let _watch = vault
        .watch(move |paths| {
            tx.send(paths).ok();
        })
        .unwrap();

    fs::write(dir.path().join("Daily/Later.md"), "# Later").unwrap();
    wait_for(&rx, &p("Daily/Later.md"));
    assert_eq!(vault.resolve(&from, "Later"), Some(p("Daily/Later.md")));

    // A whole folder arriving, as from a sync: the watcher may report just
    // the folder, which is read afresh.
    fs::create_dir_all(dir.path().join("Synced/Deeper")).unwrap();
    fs::write(dir.path().join("Synced/Deeper/Arrived.md"), "").unwrap();
    wait_for(&rx, &p("Synced"));
    assert_eq!(
        vault.resolve(&from, "Arrived"),
        Some(p("Synced/Deeper/Arrived.md"))
    );

    fs::remove_file(dir.path().join("Ideas.md")).unwrap();
    wait_for(&rx, &p("Ideas.md"));
    // The root Ideas gone, the shortest other one wins.
    assert_eq!(vault.resolve(&from, "Ideas"), Some(p("Inbox/Ideas.md")));
}

fn wait_for(rx: &std::sync::mpsc::Receiver<Vec<VaultPath>>, want: &VaultPath) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(batch) if batch.contains(want) => return,
            Ok(_) => {}
            Err(_) => panic!("{want} never reported"),
        }
    }
}
