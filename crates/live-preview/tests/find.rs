//! Find and replace through the Document facade: the matches of a query
//! in the note's source, replacing them, and the current match revealed
//! when it's inside hidden syntax. Expected values are worked out by hand.

use live_preview::Document;

#[test]
fn find_returns_every_match_in_the_source_ignoring_case_unless_asked() {
    let d = Document::new("Plan the plan. PLAN B");
    assert_eq!(d.find("plan", false), [0..4, 9..13, 15..19]);
    assert_eq!(d.find("plan", true), vec![9..13]);
    assert_eq!(d.find("", false), []);
    assert_eq!(d.find("zeppelin", false), []);
}

#[test]
fn replace_all_changes_every_match_as_one_undo_step() {
    let mut d = Document::new("Plan the plan. PLAN B");
    let matches = d.find("plan", false);
    d.replace_all(&matches, "trip");
    assert_eq!(d.text(), "trip the trip. trip B");
    // The caret ends after the last replacement.
    assert_eq!(d.caret(), 19);
    assert!(d.undo());
    assert_eq!(d.text(), "Plan the plan. PLAN B");
    assert!(!d.undo(), "one step");
}

#[test]
fn the_current_match_is_selected_and_shows_the_syntax_it_is_in() {
    let src = "See [[Orbit roadmap|the plan]] now\n\nEnd";
    let mut d = Document::new(src);
    d.set_caret(src.len());
    // Hidden with the caret away: the link shows its alias.
    assert_eq!(d.views()[0].display_text(), "See the plan now");
    let current = d.find("roadmap", false)[0].clone();
    d.select(current.clone());
    assert_eq!(d.selection(), current);
    assert_eq!(
        d.views()[0].display_text(),
        "See [[Orbit roadmap|the plan]] now"
    );
}

#[test]
fn replace_all_skips_matches_the_text_no_longer_has() {
    let mut d = Document::new("Plan the plan");
    let stale = d.find("plan", false);
    d.load("short");
    d.replace_all(&stale, "trip");
    assert_eq!(d.text(), "short");
}
