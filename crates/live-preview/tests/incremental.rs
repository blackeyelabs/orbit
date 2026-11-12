//! Long notes: an edit reparses only the blocks around it, and lays out
//! again only the leaves whose text or reveal changed. Whatever is reused,
//! the result is what a full parse of the same text gives, checked here
//! after every one of many random edits to notes with every kind of
//! block.

use std::rc::Rc;

use live_preview::Document;
use live_preview::blocks::Blocks;

/// Notes with every kind of block, and the corners where a line changes
/// what the lines around it are: fences, lists, blank runs, tables,
/// callouts, math, front matter.
const NOTES: &[&str] = &[
    "---\ntags: [a]\n---\n# Title\n\nPara one\nstill one\n\n- a\n- b\n  - c\n\n  - d\n1. x\n2. y\n\n> quote\n> more\n\n> [!note] T\n> body\n\n```rust\nfn x() {}\n\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n$$\nx^2\n$$\n\n\n\nend",
    "- one\n- two\n- three\n- four\n- five\n- six\n- seven\n- eight",
    "text\n```\ncode\n\nmore\n```\nafter\n\n$$ a $$\n\npara | not\n|---|\nrow",
];

/// Pieces random edits type, each able to change what a line is.
const PIECES: &[&str] = &[
    "x", " ", "\n", "\n\n", "- ", "  ", "# ", "```", "$$", "> ", "[!tip] ", "|", "---", "1. ",
    "**", "[[",
];

/// A small deterministic random number generator (xorshift).
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// A character boundary at or before `at`.
fn boundary(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[test]
fn incremental_reparsing_gives_what_a_full_parse_does() {
    for seed in [
        0x9e37_79b9_7f4a_7c15,
        0x2545_f491_4f6c_dd1d,
        0xdead_beef_cafe_f00d,
    ] {
        random_edits(seed);
    }
}

/// Many random edits to each note, from `seed`, each checked.
fn random_edits(seed: u64) {
    let mut rng = Rng(seed);
    for note in NOTES {
        let mut d = Document::new(note);
        for step in 0..400 {
            let len = d.text().len();
            let start = boundary(d.text(), rng.below(len + 1));
            let end = boundary(d.text(), (start + rng.below(12)).min(len));
            let piece = if rng.below(3) == 0 {
                ""
            } else {
                PIECES[rng.below(PIECES.len())]
            };
            let before = d.text().to_string();
            d.replace(start..end, piece, start..start);
            let full = Blocks::parse(d.text());
            assert!(
                *d.blocks() == full,
                "step {step}: {start}..{end} -> {piece:?} in {before:?}"
            );
            // Views too, cached or not, for the selection as it is.
            let mut fresh = Document::new(d.text());
            fresh.set_selection(d.anchored_selection());
            let views: Vec<String> = d
                .views()
                .iter()
                .map(|v| v.display_text().to_string())
                .collect();
            let want: Vec<String> = fresh
                .views()
                .iter()
                .map(|v| v.display_text().to_string())
                .collect();
            assert_eq!(views, want, "step {step}");
        }
    }
}

/// A long list, like the fixture vault's `Long.md` (which has 2,500
/// items).
fn long_list() -> String {
    (0..500).map(|i| format!("- item {i}\n")).collect()
}

#[test]
fn typing_in_a_long_list_changes_only_its_item() {
    let text = long_list();
    let mut d = Document::new(&text);
    d.take_changed();
    let at = text.find("item 250").unwrap();
    d.set_caret(at);
    d.type_text("x");
    let changed = d.take_changed().expect("a change");
    assert_eq!(changed.old, 250..251, "the item's block alone");
    assert_eq!(changed.new_len, 1);
    // Enter adds a block after it, the rest untouched.
    d.newline();
    let changed = d.take_changed().expect("a change");
    assert_eq!(
        (changed.old.start, changed.new_len - changed.old.len()),
        (250, 1)
    );
    assert!(d.take_changed().is_none(), "taken");
}

#[test]
fn only_leaves_whose_text_or_reveal_changed_are_laid_out_again() {
    let text = long_list();
    let mut d = Document::new(&text);
    d.set_caret(text.find("item 10").unwrap());
    let before = d.views();
    d.set_caret(text.find("item 20").unwrap());
    let after = d.views();
    let redone: Vec<usize> = (0..after.len())
        .filter(|&i| !Rc::ptr_eq(&before[i], &after[i]))
        .collect();
    assert_eq!(
        redone,
        [10, 20],
        "the leaf the caret left, and the one it's in"
    );
}

#[test]
fn every_edit_moves_the_revision_on() {
    let mut d = Document::new("---\na: 1\n---\nBody");
    let first = d.revision();
    d.replace(4..5, "b", 0..0);
    assert_ne!(d.revision(), first, "an edit in front matter, in no block");
    let second = d.revision();
    d.set_caret(0);
    assert_eq!(d.revision(), second, "a caret move isn't an edit");
}

#[test]
fn edits_that_change_the_lines_around_them_reparse_those_too() {
    for (text, at, removed, inserted) in [
        // A quote above a callout's title makes it a quote line.
        ("> quote\nx\n> [!note] T\n> body\n\nend", 8, 1, "> x"),
        // Deleting what came before a quote line makes it a line of its
        // own, above a callout's title: that title is a quote line now.
        ("x> q\n> [!note] T\n> body", 0, 1, ""),
        ("x> q\n> [!note] T\n> body", 1, 0, "\n"),
        // A closing `---` typed below makes the top front matter.
        ("---\na: 1\nbody\n\n- item", 13, 0, "\n---"),
    ] {
        let mut d = Document::new(text);
        d.replace(at..at + removed, inserted, at..at);
        assert!(*d.blocks() == Blocks::parse(d.text()), "{:?}", d.text());
    }
}

#[test]
fn a_rule_like_line_inside_a_list_is_an_item_and_on_its_own_a_rule() {
    use live_preview::blocks::Kind;
    let kinds = |text: &str| -> Vec<bool> {
        Blocks::parse(text)
            .blocks
            .iter()
            .map(|b| matches!(b.kind, Kind::ListItem(_)))
            .collect()
    };
    assert_eq!(kinds("- a\n- - -\n- b"), [true, true, true]);
    assert_eq!(kinds("- - -\n- - -"), [false, false]);
}
