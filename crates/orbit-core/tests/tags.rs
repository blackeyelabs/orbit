//! Tags, from front matter `tags:` and inline `#tag`, as Obsidian finds
//! them: the fixture vault in `fixtures/tags` is full of edge cases, and
//! `fixtures/tags.obsidian.json` is what real Obsidian answers for it.

use std::{collections::BTreeMap, fs};

use orbit_core::VaultPath;

mod common;
use common::{fixture_vault, fixtures, obsidian};

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

fn paths(paths: Vec<VaultPath>) -> Vec<String> {
    paths.into_iter().map(|p| p.to_string()).collect()
}

#[test]
fn the_vaults_tags_and_their_counts_are_obsidians() {
    let (_dir, vault) = fixture_vault("tags");
    let got: BTreeMap<String, u64> = vault
        .tags()
        .into_iter()
        .map(|tag| (format!("#{}", tag.name), tag.count as u64))
        .collect();
    let expected: BTreeMap<String, u64> = obsidian("tags")["counts"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(tag, count)| (tag.clone(), count.as_u64().unwrap()))
        .collect();
    assert_eq!(got, expected);
}

#[test]
fn inline_tags_are_found_where_obsidian_finds_them() {
    let dir = fixtures().join("tags");
    for (note, expected) in obsidian("tags")["inline"].as_object().unwrap() {
        let source = fs::read_to_string(dir.join(note)).unwrap();
        let got: Vec<String> = md_engine::inline_tags(&source)
            .into_iter()
            .map(|tag| format!("#{tag}"))
            .collect();
        let expected: Vec<&str> = expected
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert_eq!(got, expected, "{note}");
    }
}

#[test]
fn the_tags_are_listed_by_name_with_their_first_spelling() {
    let (_dir, vault) = fixture_vault("tags");
    let names: Vec<String> = vault.tags().into_iter().map(|t| t.name).collect();
    let mut sorted = names.clone();
    sorted.sort_by_key(|n| n.to_lowercase());
    assert_eq!(names, sorted);
    // `#orbit` came before `#Orbit` and `#ORBIT`.
    assert!(names.contains(&"orbit".to_string()));
    assert!(!names.iter().any(|n| n == "Orbit" || n == "ORBIT"));
}

#[test]
fn a_tag_lists_its_notes_and_those_of_tags_nested_in_it_ignoring_case() {
    let (_dir, vault) = fixture_vault("tags");
    let orbit = ["Inline.md", "Projects/Orbit.md"];
    assert_eq!(paths(vault.notes_tagged("project")), orbit);
    assert_eq!(paths(vault.notes_tagged("#PROJECT/orbit")), orbit);
    assert_eq!(
        paths(vault.notes_tagged("project/orbit/ui")),
        ["Projects/Orbit.md"]
    );
    assert_eq!(paths(vault.notes_tagged("orbit")), orbit);
    assert_eq!(
        paths(vault.notes_tagged("planning")),
        ["Archive/Old.md", "Inline.md"]
    );
    // A tag is whole names: `#pro` isn't `#project`.
    assert!(vault.notes_tagged("pro").is_empty());
    assert!(vault.notes_tagged("1984").is_empty());
    // Front matter, in any case of `tags:`.
    assert_eq!(paths(vault.notes_tagged("gamma")), ["Front.md"]);
    assert_eq!(paths(vault.notes_tagged("capital")), ["Contexts.md"]);
}

#[test]
fn tags_follow_notes_being_saved_moved_and_deleted() {
    let (_dir, vault) = fixture_vault("tags");
    let note = vault.create_note(&VaultPath::root()).unwrap();
    vault.save_note(&note, "New #Fresh/idea here").unwrap();
    assert_eq!(paths(vault.notes_tagged("fresh")), ["Untitled.md"]);
    assert!(vault.tags().iter().any(|t| t.name == "Fresh/idea"));

    let moved = vault.move_to(&note, &p("Projects")).unwrap();
    assert_eq!(paths(vault.notes_tagged("fresh")), [moved.to_string()]);
    vault.delete(&moved).unwrap();
    assert!(vault.notes_tagged("fresh").is_empty());
    assert!(!vault.tags().iter().any(|t| t.name.starts_with("Fresh")));
}

#[test]
fn a_txt_note_has_no_tags() {
    let (_dir, vault) = fixture_vault("tags");
    assert!(vault.notes_tagged("plain").is_empty());
}
