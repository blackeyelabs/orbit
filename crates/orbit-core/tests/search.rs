//! Vault search: ranked full-text results with highlighted snippets,
//! narrowed by `tag:` and `in:`, kept current as notes change, and cached
//! under `.vault/` so reopening a vault doesn't read every note again.

use std::fs;

use orbit_core::{SearchResults, Vault, VaultPath};

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

fn write(dir: &std::path::Path, notes: &[(&str, &str)]) {
    for (path, text) in notes {
        let file = dir.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
}

fn indexed(dir: &std::path::Path) -> Vault {
    let vault = Vault::open(dir).unwrap();
    vault.index_search();
    vault
}

fn paths(results: &SearchResults) -> Vec<&str> {
    results.results.iter().map(|r| r.path.as_str()).collect()
}

/// A snippet with its marked parts in `[…]`.
fn marked(snippet: &orbit_core::Snippet) -> String {
    let mut out = String::new();
    let mut at = 0;
    for range in &snippet.highlights {
        out.push_str(&snippet.text[at..range.start]);
        out.push('[');
        out.push_str(&snippet.text[range.clone()]);
        out.push(']');
        at = range.end;
    }
    out.push_str(&snippet.text[at..]);
    out
}

fn sample() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            (
                "Projects/Roadmap.md",
                "---\ntags: [project/orbit]\n---\n# Roadmap\n\nThe paper theme first.\nThen diagrams.\n",
            ),
            (
                "Meetings/Weekly.md",
                "Talked about the roadmap.\n\nThe roadmap slips; roadmap again.\n#planning\n",
            ),
            ("Archive/Old.md", "An old roadmap, superseded. #project\n"),
            ("Ideas.md", "Café notes and a road trip.\n"),
            ("Two words/Inside.md", "Paper cuts.\n"),
            (
                "notes.txt",
                "Plain text, with a mention of the roadmap somewhere for later.\n",
            ),
            ("image.png", "roadmap"),
        ],
    );
    let vault = indexed(dir.path());
    (dir, vault)
}

#[test]
fn a_title_match_ranks_first_then_notes_saying_it_more() {
    let (_dir, vault) = sample();
    let found = vault.search("roadmap", 20);
    assert!(found.complete);
    assert_eq!(
        paths(&found),
        [
            "Projects/Roadmap.md",
            "Meetings/Weekly.md",
            "Archive/Old.md",
            "notes.txt"
        ]
    );
    // Matches count in the text: the title is the note's name.
    assert_eq!((found.matching_notes, found.matches), (4, 6));

    let weekly = &found.results[1];
    assert_eq!(weekly.title, "Weekly");
    assert_eq!(weekly.matches, 3);
    let snippets: Vec<String> = weekly.snippets.iter().map(marked).collect();
    assert_eq!(
        snippets,
        [
            "Talked about the [roadmap].",
            "The [roadmap] slips; [roadmap] again."
        ]
    );
}

#[test]
fn words_match_in_any_case_and_accent_and_the_last_as_a_prefix() {
    let (_dir, vault) = sample();
    assert_eq!(
        paths(&vault.search("ROADMAP SLIPS", 20)),
        ["Meetings/Weekly.md"]
    );
    assert_eq!(paths(&vault.search("cafe", 20)), ["Ideas.md"]);
    // Every word must match: the last one may be unfinished.
    assert_eq!(
        paths(&vault.search("paper th", 20)),
        ["Projects/Roadmap.md"]
    );
    let road = vault.search("road", 20);
    assert_eq!(road.results.len(), 5, "road and roadmap");
    assert!(vault.search("zebra", 20).results.is_empty());
    assert!(vault.search("", 20).results.is_empty());
}

#[test]
fn tag_and_in_narrow_the_results_nested_ones_included() {
    let (_dir, vault) = sample();
    assert_eq!(
        paths(&vault.search("roadmap tag:project", 20)),
        ["Projects/Roadmap.md", "Archive/Old.md"],
        "#project/orbit is within #project"
    );
    assert_eq!(
        paths(&vault.search("roadmap tag:#Planning", 20)),
        ["Meetings/Weekly.md"]
    );
    assert_eq!(
        paths(&vault.search("roadmap in:Meetings", 20)),
        ["Meetings/Weekly.md"]
    );
    assert_eq!(
        paths(&vault.search(r#"paper in:"two words""#, 20)),
        ["Two words/Inside.md"]
    );
    // Filters alone list what they match.
    assert_eq!(
        paths(&vault.search("tag:project/orbit", 20)),
        ["Projects/Roadmap.md"]
    );
    assert!(vault.search("roadmap in:Nowhere", 20).results.is_empty());
}

#[test]
fn results_follow_notes_being_saved_renamed_and_deleted() {
    let (_dir, vault) = sample();
    let note = vault.create_note(&VaultPath::root()).unwrap();
    vault.save_note(&note, "Zebra crossing").unwrap();
    assert_eq!(paths(&vault.search("zebra", 20)), ["Untitled.md"]);

    let renamed = vault.rename(&note, "Stripes").unwrap();
    assert_eq!(paths(&vault.search("zebra", 20)), ["Stripes.md"]);
    assert_eq!(
        paths(&vault.search("stripes", 20)),
        ["Stripes.md"],
        "by title"
    );

    vault.save_note(&renamed, "Horses now").unwrap();
    assert!(vault.search("zebra", 20).results.is_empty());

    vault.delete(&p("Meetings")).unwrap();
    assert!(!paths(&vault.search("roadmap", 20)).contains(&"Meetings/Weekly.md"));
    vault.delete(&renamed).unwrap();
    assert!(vault.search("horses", 20).results.is_empty());
}

#[test]
fn results_follow_changes_other_apps_make() {
    let (dir, vault) = sample();
    let (tx, rx) = std::sync::mpsc::channel();
    let _watch = vault
        .watch(move |paths| {
            tx.send(paths).ok();
        })
        .unwrap();
    fs::write(dir.path().join("Synced.md"), "A zebra arrives").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !rx
        .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        .expect("the watcher reports the new note")
        .contains(&p("Synced.md"))
    {}
    assert_eq!(paths(&vault.search("zebra", 20)), ["Synced.md"]);
}

#[cfg(unix)]
#[test]
fn reopening_uses_the_cache_and_reads_only_what_changed() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, vault) = sample();
    drop(vault);
    // Unreadable but unchanged: only the cache can still know what it says.
    let ideas = dir.path().join("Ideas.md");
    fs::set_permissions(&ideas, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&ideas).is_ok() {
        eprintln!("skipped: running with permission to ignore modes (root?)");
        return;
    }
    // Changed and deleted while the vault was closed.
    fs::write(dir.path().join("Archive/Old.md"), "A new giraffe.").unwrap();
    fs::remove_file(dir.path().join("Projects/Roadmap.md")).unwrap();

    let vault = indexed(dir.path());
    fs::set_permissions(&ideas, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(paths(&vault.search("cafe", 20)), ["Ideas.md"]);
    assert_eq!(paths(&vault.search("giraffe", 20)), ["Archive/Old.md"]);
    assert_eq!(
        paths(&vault.search("roadmap", 20)),
        ["Meetings/Weekly.md", "notes.txt"]
    );
    assert!(dir.path().join(".vault").is_dir());
}

#[test]
fn until_the_vault_is_indexed_results_are_marked_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("a.md", "alpha"), ("b.md", "beta")]);
    let vault = Vault::open(dir.path()).unwrap();
    assert!(!vault.search("alpha", 20).complete);
    vault.index_search();
    assert_eq!(vault.search_progress(), None);
    let found = vault.search("alpha", 20);
    assert!(found.complete);
    assert_eq!(paths(&found), ["a.md"]);
}
