//! Backlinks: the notes that link to a note, each with the context of its
//! links, kept current as linking notes are created, edited, renamed and
//! deleted, by Orbit or by other apps.

use std::fs;

use orbit_core::{Backlink, Vault, VaultPath};

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

fn vault_with(notes: &[(&str, &str)]) -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in notes {
        let file = dir.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    (dir, vault)
}

/// The notes linking to `to`.
fn froms(vault: &Vault, to: &VaultPath) -> Vec<String> {
    vault
        .backlinks(to)
        .iter()
        .map(|b| b.from.to_string())
        .collect()
}

/// Each linking note with its mentions, the link marked `«…»`.
fn shown(backlinks: &[Backlink]) -> Vec<(&str, Vec<String>)> {
    backlinks
        .iter()
        .map(|b| {
            let mentions = b
                .mentions
                .iter()
                .map(|m| {
                    let (before, rest) = m.context.split_at(m.link.start);
                    let (link, after) = rest.split_at(m.link.len());
                    format!("{before}«{link}»{after}")
                })
                .collect();
            (b.from.as_str(), mentions)
        })
        .collect()
}

#[test]
fn every_note_linking_to_a_note_is_listed_with_the_context_of_each_link() {
    let (_dir, vault) = vault_with(&[
        (
            "Projects/Plan.md",
            "---\naliases: [Roadmap]\n---\n# Plan\nSee [[#Goals]] and [[Plan]].\n",
        ),
        ("Archive/2023/Plan.md", "# The old plan\n"),
        (
            "Home.md",
            "# Home\n\nStart with the [[Projects/Plan|plan]] today.\n- goals: [[Plan#Goals]]\n\n![[Projects/Plan]]\n\n`[[Plan]]` is code.\n",
        ),
        (
            "Projects/Meeting.md",
            "Agreed the order in [[Plan]], paper first.\n",
        ),
        (
            "Inbox/Links.md",
            "A [markdown link](../Projects/Plan.md) and [[Roadmap]].\n",
        ),
        ("Archive/2023/Old.md", "Superseded by [[Plan]].\n"),
        // Plain text has wikilinks, but no markdown links.
        (
            "notes.txt",
            "plain text: [[Projects/Plan]]\n[not](Projects/Plan.md)\n",
        ),
        ("Unrelated.md", "Nothing to see. [[Missing]]\n"),
    ]);

    assert_eq!(
        shown(&vault.backlinks(&p("Projects/Plan.md"))),
        [
            (
                "Home.md",
                vec![
                    "Start with the «[[Projects/Plan|plan]]» today.".to_string(),
                    "- goals: «[[Plan#Goals]]»".to_string(),
                    "«![[Projects/Plan]]»".to_string(),
                ]
            ),
            (
                "Inbox/Links.md",
                vec![
                    "A «[markdown link](../Projects/Plan.md)» and [[Roadmap]].".to_string(),
                    "A [markdown link](../Projects/Plan.md) and «[[Roadmap]]».".to_string(),
                ]
            ),
            (
                "Projects/Meeting.md",
                vec!["Agreed the order in «[[Plan]]», paper first.".to_string()]
            ),
            (
                "notes.txt",
                vec!["plain text: «[[Projects/Plan]]»".to_string()]
            ),
        ],
        "Archive/2023/Old.md's [[Plan]] is its neighbour's, and a note's links to itself aren't backlinks"
    );
    assert_eq!(
        shown(&vault.backlinks(&p("Archive/2023/Plan.md"))),
        [(
            "Archive/2023/Old.md",
            vec!["Superseded by «[[Plan]]».".to_string()]
        )]
    );
    assert!(vault.backlinks(&p("Unrelated.md")).is_empty());
}

#[test]
fn a_long_line_is_cut_to_the_words_around_the_link() {
    let before = "word ".repeat(40);
    let after = " more".repeat(40);
    let (_dir, vault) = vault_with(&[
        ("Plan.md", ""),
        ("Long.md", &format!("{before}see [[Plan]] then{after}\n")),
    ]);
    let backlinks = vault.backlinks(&p("Plan.md"));
    let mention = &backlinks[0].mentions[0];
    assert!(mention.context.starts_with("…word "), "{}", mention.context);
    assert!(mention.context.ends_with("more…"), "{}", mention.context);
    assert!(mention.context.chars().count() < 150, "{}", mention.context);
    assert_eq!(&mention.context[mention.link.clone()], "[[Plan]]");
}

#[test]
fn backlinks_follow_linking_notes_being_created_edited_renamed_and_deleted() {
    let (_dir, vault) = vault_with(&[("Plan.md", "# Plan\n")]);
    let plan = p("Plan.md");
    let froms = |vault: &Vault| froms(vault, &plan);
    assert!(froms(&vault).is_empty());

    let note = vault.create_note(&VaultPath::root()).unwrap();
    vault.save_note(&note, "About [[Plan]].").unwrap();
    assert_eq!(froms(&vault), ["Untitled.md"]);

    vault.save_note(&note, "About nothing.").unwrap();
    assert!(froms(&vault).is_empty());
    vault.save_note(&note, "About [[Plan]] again.").unwrap();

    let renamed = vault.rename(&note, "Meeting").unwrap();
    assert_eq!(froms(&vault), ["Meeting.md"]);
    let folder = vault.create_folder(&VaultPath::root()).unwrap();
    let moved = vault.move_to(&renamed, &folder).unwrap();
    let folder = vault.rename(&folder, "Notes").unwrap();
    assert_eq!(moved, p("Untitled/Meeting.md"));
    assert_eq!(froms(&vault), ["Notes/Meeting.md"]);

    vault.delete(&folder.join("Meeting.md").unwrap()).unwrap();
    assert!(froms(&vault).is_empty());
}

#[test]
fn backlinks_follow_changes_other_apps_make() {
    let (dir, vault) = vault_with(&[("Plan.md", ""), ("Other.md", "[[Plan]]")]);
    let plan = p("Plan.md");
    assert_eq!(vault.backlinks(&plan).len(), 1);
    let (tx, rx) = std::sync::mpsc::channel();
    let _watch = vault
        .watch(move |paths| {
            tx.send(paths).ok();
        })
        .unwrap();

    fs::write(dir.path().join("New.md"), "Links to [[Plan]].").unwrap();
    fs::write(dir.path().join("Other.md"), "No link now.").unwrap();
    wait_for(&rx, &[p("New.md"), p("Other.md")]);
    assert_eq!(froms(&vault, &plan), ["New.md"]);
}

/// Waits until the watcher has reported every one of `want`.
fn wait_for(rx: &std::sync::mpsc::Receiver<Vec<VaultPath>>, want: &[VaultPath]) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut missing = want.to_vec();
    while !missing.is_empty() {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(batch) => missing.retain(|p| !batch.contains(p)),
            Err(_) => panic!("{missing:?} never reported"),
        }
    }
}
