//! Lists, task lists and quotes through the Document facade: their line
//! markers hidden unless the caret is on the line, how deep each item
//! nests, and ticking a task. Expected values are worked out by hand.

use live_preview::Document;
use live_preview::blocks::{Item, Kind};

/// What each leaf shows.
fn shown(d: &Document) -> Vec<String> {
    d.views()
        .iter()
        .map(|v| v.display_text().to_string())
        .collect()
}

#[test]
fn list_markers_show_only_on_the_caret_s_line() {
    let src = "- one\n- **two**\n1. three";
    let mut d = Document::new(src);
    // On the second line: its marker and syntax show, the others' don't.
    d.set_caret(8);
    assert_eq!(shown(&d), ["one", "- **two**", "three"]);
    d.set_caret(src.len());
    assert_eq!(shown(&d), ["one", "two", "1. three"]);
}

/// Each block's kind.
fn kinds(d: &Document) -> Vec<Kind> {
    d.blocks().blocks.iter().map(|b| b.kind.clone()).collect()
}

/// A bullet item this deep.
fn bullet(depth: usize) -> Kind {
    Kind::ListItem(Item {
        depth,
        number: None,
        task: None,
    })
}

#[test]
fn an_item_indented_past_the_one_before_nests_under_it() {
    let src = "- a\n  - b\n    - c\n  - d\n- e\n\t- f";
    let d = Document::new(src);
    assert_eq!(
        kinds(&d),
        [
            bullet(0),
            bullet(1),
            bullet(2),
            bullet(1),
            bullet(0),
            bullet(1)
        ]
    );
    // The indentation isn't text to show, even on the caret's line.
    let mut d = d;
    d.set_caret(src.find("c").unwrap());
    assert_eq!(shown(&d), ["a", "b", "- c", "d", "e", "f"]);
    d.move_line_start(false);
    assert_eq!(d.caret(), src.find("- c").unwrap(), "Home: the marker");
}

#[test]
fn numbers_tasks_and_quotes_hide_their_markers_too() {
    let src = "3. third\n- [ ] open\n- [x] done\n\n> said\n> > again\n\nend";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    let item = |number, task| {
        Kind::ListItem(Item {
            depth: 0,
            number,
            task,
        })
    };
    assert_eq!(
        kinds(&d),
        [
            item(Some(3), None),
            item(None, Some(false)),
            item(None, Some(true)),
            Kind::Quote(1),
            Kind::Quote(2),
            Kind::Paragraph,
        ]
    );
    assert_eq!(shown(&d), ["third", "open", "done", "said", "again", "end"]);
    d.set_caret(src.find("again").unwrap());
    assert_eq!(shown(&d)[4], "> > again");
}

#[test]
fn ticking_a_task_writes_its_box_as_one_undo_step() {
    let src = "- [ ] open\n  - [X] done\n- plain";
    let mut d = Document::new(src);
    d.set_caret(src.len());
    assert!(d.toggle_task(0));
    assert_eq!(d.text(), "- [x] open\n  - [X] done\n- plain");
    assert_eq!(d.caret(), src.len(), "the caret stays");
    assert!(d.toggle_task(1));
    assert_eq!(d.text(), "- [x] open\n  - [ ] done\n- plain");
    assert!(!d.toggle_task(2), "not a task");
    assert!(d.undo());
    assert_eq!(d.text(), "- [x] open\n  - [X] done\n- plain");
}

#[test]
fn an_item_nests_once_it_reaches_the_text_of_the_one_above() {
    // One space in isn't past `- `, so `b` is a sibling; a blank line
    // inside a list doesn't end its nesting.
    let d = Document::new("- a\n - b\n   - c\n\n   - d\n\n1) e\n   - f");
    let number = |n| {
        Kind::ListItem(Item {
            depth: 0,
            number: Some(n),
            task: None,
        })
    };
    assert_eq!(
        kinds(&d),
        [
            bullet(0),
            bullet(0),
            bullet(1),
            bullet(1),
            number(1),
            bullet(1)
        ]
    );
}

#[test]
fn tab_before_a_marker_nests_the_item_deeper() {
    let src = "- a\n- b";
    let mut d = Document::new(src);
    d.set_caret(src.len());
    d.move_line_start(false);
    d.type_text("\t");
    assert_eq!(d.text(), "- a\n\t- b");
    assert_eq!(kinds(&d), [bullet(0), bullet(1)]);
}
