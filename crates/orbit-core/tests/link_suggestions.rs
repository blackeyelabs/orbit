//! Wikilink autocomplete: the notes a typed `[[query` suggests, and the
//! link Orbit writes for each. Links use the shortest unique path, as
//! Obsidian's "Shortest path when possible" writes them
//! (`fileToLinktext`): the bare name when no other file has it, else the
//! path from the vault root.

use orbit_core::{LinkSuggestion, VaultPath};

mod common;
use common::fixture_vault;

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

/// Each suggestion's note, the alias it matched and the text it writes
/// between `[[` and `]]`.
fn shown(suggestions: &[LinkSuggestion]) -> Vec<(&str, Option<&str>, String)> {
    suggestions
        .iter()
        .map(|s| (s.path.as_str(), s.alias.as_deref(), s.text()))
        .collect()
}

#[test]
fn a_link_is_the_bare_name_unless_another_file_has_that_name() {
    let (_dir, vault) = fixture_vault("links");
    for (path, link) in [
        ("Home.md", "Home"),
        ("Inbox/Capture.md", "Capture"),
        ("My.Dotted.Name.md", "My.Dotted.Name"),
        ("notes.txt", "notes.txt"),
        // Four notes are named Ideas, three Same, two Plan: each is written
        // with its whole path, the root one too.
        ("Ideas.md", "Ideas"),
        ("Inbox/Ideas.md", "Inbox/Ideas"),
        ("Projects/Deep/Ideas.md", "Projects/Deep/Ideas"),
        ("Long/Way/Down/Same.md", "Long/Way/Down/Same"),
        ("Projects/Plan.md", "Projects/Plan"),
        ("assets/diagram.png", "assets/diagram.png"),
    ] {
        assert_eq!(vault.link_text(&p(path)), link, "{path}");
    }
}

#[test]
fn a_name_stops_being_unique_when_another_note_takes_it() {
    let (_dir, vault) = fixture_vault("links");
    let capture = p("Inbox/Capture.md");
    assert_eq!(vault.link_text(&capture), "Capture");
    vault.save_note(&p("Daily/Capture.md"), "").unwrap();
    assert_eq!(vault.link_text(&capture), "Inbox/Capture");
    // Names match ignoring case.
    vault.delete(&p("Daily/Capture.md")).unwrap();
    vault.save_note(&p("capture.md"), "").unwrap();
    assert_eq!(vault.link_text(&capture), "Inbox/Capture");
}

#[test]
fn suggestions_fuzzy_match_names_and_aliases_best_first() {
    let (_dir, vault) = fixture_vault("links");
    let home = p("Home.md");
    let found = vault.link_suggestions(&home, "plan", 10).notes;
    assert_eq!(
        &shown(&found)[..2],
        [
            ("Archive/Plan.md", None, "Archive/Plan".to_string()),
            ("Projects/Plan.md", None, "Projects/Plan".to_string()),
        ],
        "both Plans, before \"The Plan\" (an alias of one of them)"
    );
    // An alias writes the note's link, showing the alias.
    assert_eq!(
        shown(&vault.link_suggestions(&home, "roadmap", 10).notes),
        [(
            "Projects/Plan.md",
            Some("Roadmap"),
            "Projects/Plan|Roadmap".to_string()
        )]
    );
    // Fuzzy: letters in order, ignoring case.
    let capture = vault.link_suggestions(&home, "cptr", 10).notes;
    assert_eq!(capture[0].path, p("Inbox/Capture.md"));
    assert_eq!(capture[0].text(), "Capture");
    // The folder can narrow it.
    let deep = vault.link_suggestions(&home, "deep ideas", 10).notes;
    assert_eq!(deep[0].path, p("Projects/Deep/Ideas.md"));
}

#[test]
fn only_notes_are_suggested_up_to_the_limit() {
    let (_dir, vault) = fixture_vault("links");
    let home = p("Home.md");
    let notes = |query: &str, limit: usize| vault.link_suggestions(&home, query, limit).notes;
    assert!(notes("diagram", 10).is_empty());
    assert!(notes("zzzz", 10).is_empty());
    assert_eq!(notes("", 3).len(), 3);
    assert_eq!(notes("e", 2).len(), 2);
}

#[test]
fn a_name_no_link_resolves_to_is_offered_as_a_new_note() {
    let (_dir, vault) = fixture_vault("links");
    let home = p("Home.md");
    let new_note = |query: &str| vault.link_suggestions(&home, query, 10).new_note;
    assert_eq!(new_note("Missing note").as_deref(), Some("Missing note"));
    assert_eq!(new_note("  Spaced  ").as_deref(), Some("Spaced"));
    assert_eq!(new_note("Ideas"), None);
    assert_eq!(new_note("projects/plan"), None);
    // An alias leads to its note already.
    assert_eq!(new_note("Roadmap"), None);
    assert_eq!(new_note(""), None);
}

#[test]
fn colliding_files_that_are_not_markdown_keep_their_extension() {
    let (_dir, vault) = fixture_vault("links");
    vault.save_note(&p("Daily/notes.txt"), "").unwrap();
    assert_eq!(vault.link_text(&p("notes.txt")), "notes.txt");
    assert_eq!(vault.link_text(&p("Daily/notes.txt")), "Daily/notes.txt");
    // Only a lowercase `.md` is left out, as Obsidian does.
    vault.save_note(&p("Shout.MD"), "").unwrap();
    assert_eq!(vault.link_text(&p("Shout.MD")), "Shout.MD");
}
