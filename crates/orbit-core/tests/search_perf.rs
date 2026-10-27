//! Vault search over 10,000 generated notes answers within interactive
//! time. Slow to set up, so it's run on its own, built for release:
//!
//!     cargo test --release -p orbit-core --test search_perf -- --ignored --nocapture
//!
//! The budget, recorded in the ticket (#18): each search within 100 ms.

use std::{
    fs,
    time::{Duration, Instant},
};

use orbit_core::Vault;

const NOTES: usize = 10_000;
const WORDS_PER_NOTE: usize = 300;
const BUDGET: Duration = Duration::from_millis(100);

/// Deterministic words: a small vocabulary, unevenly used, as in real notes.
fn words(seed: &mut u64, count: usize) -> String {
    const VOCABULARY: &[&str] = &[
        "orbit", "roadmap", "paper", "theme", "diagram", "meeting", "notes", "plan", "release",
        "search", "index", "tag", "folder", "link", "garden", "coffee", "travel", "budget",
        "recipe", "reading", "project", "weekly", "idea", "draft", "review", "design", "music",
        "the", "and", "a", "of", "to", "in", "is", "for", "with", "on", "that",
    ];
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = (*seed >> 33) as usize;
        // Squared, so earlier words are rarer than later ones.
        let ix = (r % VOCABULARY.len()) * (r / 7 % VOCABULARY.len()) / VOCABULARY.len();
        out.push(VOCABULARY[ix]);
    }
    out.join(" ")
}

#[test]
#[ignore]
fn searching_ten_thousand_notes_takes_under_100_ms() {
    let dir = tempfile::tempdir().unwrap();
    let mut seed = 42;
    for n in 0..NOTES {
        let folder = dir.path().join(format!("Folder {}", n % 50));
        fs::create_dir_all(&folder).unwrap();
        let text = format!(
            "---\ntags: [topic/{}]\n---\n# Note {n}\n\n{}\n",
            n % 20,
            words(&mut seed, WORDS_PER_NOTE)
        );
        fs::write(folder.join(format!("Note {n}.md")), text).unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    let started = Instant::now();
    vault.index_search();
    eprintln!("indexed {NOTES} notes in {:?}", started.elapsed());

    let mut slowest = Duration::ZERO;
    for query in [
        "roadmap",
        "paper theme",
        "garden coffee travel",
        "rev",
        "o",
        "the",
        "plan tag:topic/3",
        "budget in:\"folder 7\"",
        "tag:topic",
        "nothing-matches-this",
    ] {
        let started = Instant::now();
        let found = vault.search(query, 50);
        let took = started.elapsed();
        eprintln!("{query:>28}: {:>5} notes in {took:?}", found.matching_notes);
        slowest = slowest.max(took);
    }
    drop(vault);

    let started = Instant::now();
    let vault = Vault::open(dir.path()).unwrap();
    vault.index_search();
    eprintln!("reopened from the cache in {:?}", started.elapsed());
    assert!(slowest < BUDGET, "slowest search took {slowest:?}");
}
