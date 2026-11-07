//! Editing lists through the Document facade: Enter continues a list item,
//! task or quote, or ends it when it's empty; Tab and Shift+Tab nest and
//! unnest items; numbered lists renumber. Each is one undo step. Expected
//! values are worked out by hand.

use live_preview::Document;

/// A document over `text`, the caret at `caret`.
fn at(text: &str, caret: usize) -> Document {
    let mut d = Document::new(text);
    d.set_caret(caret);
    d
}

#[test]
fn enter_in_an_item_starts_the_next_one_like_it() {
    let mut d = at("- one", 5);
    d.newline();
    assert_eq!(d.text(), "- one\n- ");
    assert_eq!(d.caret(), 8);
    // In the middle, the rest of the line goes into the new item.
    let mut d = at("  * one two", 7);
    d.newline();
    assert_eq!(d.text(), "  * one\n  * two");
    assert_eq!(d.caret(), 12);
    assert!(d.undo());
    assert_eq!(d.text(), "  * one two", "one undo step");
}

#[test]
fn a_new_task_starts_unticked() {
    let mut d = at("- [x] done", 10);
    d.newline();
    assert_eq!(d.text(), "- [x] done\n- [ ] ");
    assert_eq!(d.caret(), 17);
}

#[test]
fn a_numbered_item_continues_with_the_next_number_and_the_rest_renumber() {
    let src = "1. one\n2. two\n3. three\n   - aside\n4. four";
    let mut d = at(src, 6);
    d.newline();
    assert_eq!(
        d.text(),
        "1. one\n2. \n3. two\n4. three\n   - aside\n5. four"
    );
    assert_eq!(d.caret(), 10);
    assert!(d.undo());
    assert_eq!(d.text(), src, "one undo step");
    // The delimiter stays as written.
    let mut d = at("9) nine", 7);
    d.newline();
    assert_eq!(d.text(), "9) nine\n10) ");
}

#[test]
fn a_quote_goes_on_as_deep_as_it_is() {
    let mut d = at("> > said", 8);
    d.newline();
    assert_eq!(d.text(), "> > said\n> > ");
    assert_eq!(d.caret(), 13);
}

#[test]
fn enter_on_an_empty_item_ends_the_list() {
    let mut d = at("- one\n- ", 8);
    d.newline();
    assert_eq!(d.text(), "- one\n");
    assert_eq!(d.caret(), 6);
    let mut d = at("> said\n> ", 9);
    d.newline();
    assert_eq!(d.text(), "> said\n");
    let mut d = at("- [ ] ", 6);
    d.newline();
    assert_eq!(d.text(), "");
}

#[test]
fn tab_nests_the_items_selected_and_shift_tab_unnests_them() {
    let src = "- a\n- b\n- c\n\nafter";
    let mut d = Document::new(src);
    d.select(5..9);
    assert!(d.indent());
    assert_eq!(d.text(), "- a\n\t- b\n\t- c\n\nafter");
    assert_eq!(d.selection(), 6..11, "on the same text");
    assert!(d.undo());
    assert_eq!(d.text(), src, "one undo step");
    // Out: a tab, or up to 4 spaces.
    let mut d = at("- a\n\t- b\n      - c", 10);
    assert!(d.outdent());
    assert_eq!(d.text(), "- a\n\t- b\n  - c");
    assert_eq!(d.caret(), 11, "still at the marker");
    // Not in a list: nothing, so Tab types a tab.
    let mut d = at("text", 2);
    assert!(!d.indent());
    assert_eq!(d.text(), "text");
}

#[test]
fn nesting_a_numbered_item_renumbers_both_levels() {
    let src = "1. a\n2. b\n3. c\n4. d";
    let mut d = at(src, 9);
    assert!(d.indent());
    // `b` starts a list of its own under `a`, and `c` follows `a`.
    assert_eq!(d.text(), "1. a\n\t1. b\n2. c\n3. d");
    assert!(d.outdent());
    assert_eq!(d.text(), src);
}

#[test]
fn shift_tab_leaves_an_item_already_at_the_edge_alone() {
    let mut d = at("5. a\n6. b", 4);
    assert!(!d.outdent());
    assert_eq!(d.text(), "5. a\n6. b");
}

#[test]
fn a_selection_ending_at_a_line_s_start_leaves_that_line_out() {
    let mut d = Document::new("- a\n- b\n- c");
    d.select(4..8);
    assert!(d.indent());
    assert_eq!(d.text(), "- a\n\t- b\n- c");
}

#[test]
fn enter_on_an_empty_nested_item_moves_it_out_a_level() {
    let mut d = at("- a\n\t- ", 7);
    d.newline();
    assert_eq!(d.text(), "- a\n- ");
    assert_eq!(d.caret(), 6);
    // Ending a numbered item renumbers what follows.
    let mut d = at("1. a\n2. \n3. b", 8);
    d.newline();
    // A blank line doesn't end a list, so `b` follows `a`.
    assert_eq!(d.text(), "1. a\n\n2. b");
}

#[test]
fn a_backward_selection_stays_backward() {
    let mut d = Document::new("- a\n- b");
    d.set_selection(live_preview::conceal::Selection { anchor: 7, head: 2 });
    assert!(d.indent());
    let s = d.anchored_selection();
    assert_eq!((s.anchor, s.head), (9, 3));
}
