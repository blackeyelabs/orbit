//! Word counts for the status bar.

use md_engine::word_count;

#[test]
fn words_are_counted_outside_front_matter_ignoring_markdown_punctuation() {
    let note = "---\ntags: [orbit, notes]\ntitle: Ignored words here\n---\n\
                # Launch plan\n\n- [ ] **Ship** the *first* build\n> don't forget ~~it~~\n";
    // Launch, plan, Ship, the, first, build, don't, forget, it
    assert_eq!(word_count(note), 9);
}

#[test]
fn each_cjk_character_counts_as_a_word() {
    assert_eq!(word_count("日本語 notes"), 4);
}

#[test]
fn an_empty_or_front_matter_only_note_has_no_words() {
    assert_eq!(word_count(""), 0);
    assert_eq!(word_count("\u{feff}---\r\ntitle: x\r\n---\r\n"), 0);
}
