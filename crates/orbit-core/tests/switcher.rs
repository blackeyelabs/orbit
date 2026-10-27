//! The Switcher: the Quick switcher's notes, fuzzy-matched on their paths
//! and aliases, and the Command palette's commands, by name; each match
//! with the parts of its label to mark.

use orbit_core::{NoteMatch, VaultPath, command_query, match_commands};

mod common;
use common::fixture_vault;

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

/// Each match's label with its marked parts in `[…]`.
fn marked(label: &str, highlight: &[std::ops::Range<usize>]) -> String {
    let mut out = String::new();
    let mut at = 0;
    for range in highlight {
        out.push_str(&label[at..range.start]);
        out.push('[');
        out.push_str(&label[range.clone()]);
        out.push(']');
        at = range.end;
    }
    out.push_str(&label[at..]);
    out
}

fn notes(found: &[NoteMatch]) -> Vec<(&str, String)> {
    found
        .iter()
        .map(|m| (m.path.as_str(), marked(&m.label, &m.highlight)))
        .collect()
}

#[test]
fn notes_match_by_name_or_alias_best_first_with_the_match_marked() {
    let (_dir, vault) = fixture_vault("links");
    assert_eq!(
        &notes(&vault.find_notes("capt", 10))[..1],
        [("Inbox/Capture.md", "[Capt]ure".to_string())]
    );
    // An alias is shown as itself, marked.
    assert_eq!(
        notes(&vault.find_notes("roadmap", 10)),
        [("Projects/Plan.md", "[Roadmap]".to_string())]
    );
    // Letters of the folder count, but only the name's are marked.
    let deep = vault.find_notes("deepide", 10);
    assert_eq!(deep[0].path, p("Projects/Deep/Ideas.md"));
    assert_eq!(marked(&deep[0].label, &deep[0].highlight), "[Ide]as");
    // Unicode names are marked on character boundaries.
    vault.save_note(&p("Café 日本.md"), "").unwrap();
    assert_eq!(
        notes(&vault.find_notes("日本", 10)),
        [("Café 日本.md", "Café [日本]".to_string())]
    );
}

#[test]
fn every_note_is_listed_for_an_empty_query_up_to_the_limit() {
    let (_dir, vault) = fixture_vault("links");
    let all = vault.find_notes("", 100);
    assert_eq!(all.len(), 15, "the fixture's .md and .txt notes");
    assert!(all.iter().all(|m| m.highlight.is_empty()));
    assert_eq!(vault.find_notes("", 4).len(), 4);
}

#[test]
fn a_leading_angle_bracket_asks_for_commands() {
    assert_eq!(command_query(">"), Some(""));
    assert_eq!(command_query("> new"), Some("new"));
    assert_eq!(command_query(">toggle "), Some("toggle"));
    assert_eq!(command_query("plan"), None);
    assert_eq!(command_query(" >x"), None);
}

#[test]
fn commands_match_by_name_best_first_keeping_their_order_otherwise() {
    let commands = [
        "New note",
        "New folder",
        "Toggle sidebar",
        "Toggle right panel",
        "Open settings",
    ];
    let names = |query: &str| -> Vec<String> {
        match_commands(query, &commands)
            .into_iter()
            .map(|m| marked(commands[m.index], &m.highlight))
            .collect()
    };
    assert_eq!(names(""), commands.map(String::from));
    assert_eq!(names("new"), ["[New] note", "[New] folder"]);
    assert_eq!(match_commands("tgrp", &commands)[0].index, 3);
    assert_eq!(names("SETTINGS"), ["Open [settings]"]);
    assert!(names("zzz").is_empty());
}
