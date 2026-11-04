//! The spike's block and leaf model: how a note splits into leaves and
//! where the caret may stand.

use spike4_editor_host::model::{Doc, Kind, Selection};

/// A note with one of each block the spike handles.
const NOTE: &str = "# Title\n\nSome **bold** text\nwrapped on.\n\n- one\n- [x] two\n\n```rust\nlet x = 1;\n```\n\n| a | b |\n|---|---|\n| c |  |\n";

/// Each leaf's source text.
fn texts(doc: &Doc, source: &str) -> Vec<String> {
    doc.leaves
        .iter()
        .map(|l| source[l.source.clone()].to_string())
        .collect()
}

#[test]
fn a_note_splits_into_blocks_and_leaves() {
    let doc = Doc::parse(NOTE);
    let kinds: Vec<_> = doc.blocks.iter().map(|b| b.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            Kind::Heading(1),
            Kind::Paragraph,
            Kind::List {
                ordered: false,
                checked: vec![None, Some(true)]
            },
            Kind::Code {
                lang: Some("rust".into())
            },
            Kind::Table {
                columns: 2,
                rows: vec![2, 2]
            },
            // The note's end: an empty last line after the table.
            Kind::Empty,
        ]
    );
    assert_eq!(
        texts(&doc, NOTE),
        vec![
            "# Title",
            "Some **bold** text\nwrapped on.",
            "one",
            "two",
            "let x = 1;",
            "a",
            "b",
            "c",
            "",
            "",
        ]
    );
}

#[test]
fn an_empty_cell_still_has_a_place_for_the_caret() {
    let doc = Doc::parse(NOTE);
    let empty = &doc.leaves[8].source;
    assert!(empty.is_empty());
    // Just inside the pipes: "| c |  |" puts it after the first space.
    assert_eq!(&NOTE[empty.start - 2..empty.start], "| ");
}

#[test]
fn extra_blank_lines_are_empty_blocks_the_caret_can_stand_on() {
    let source = "one\n\n\ntwo";
    let doc = Doc::parse(source);
    let kinds: Vec<_> = doc.blocks.iter().map(|b| b.kind.clone()).collect();
    assert_eq!(kinds, vec![Kind::Paragraph, Kind::Empty, Kind::Paragraph]);
    assert_eq!(doc.leaves[1].source, 5..5);
}

#[test]
fn right_steps_through_a_leaf_then_jumps_to_the_next_leaf() {
    let doc = Doc::parse(NOTE);
    // End of the heading -> start of the paragraph, over the blank line.
    assert_eq!(doc.next_stop(NOTE, 7), 9);
    assert_eq!(doc.prev_stop(NOTE, 9), 7);
    // Within a leaf it moves one grapheme.
    assert_eq!(doc.next_stop(NOTE, 9), 10);
    // End of item "one" -> start of "two", past "- [x] ".
    let one = &doc.leaves[2].source;
    assert_eq!(doc.next_stop(NOTE, one.end), doc.leaves[3].source.start);
}

#[test]
fn right_moves_cell_to_cell() {
    let doc = Doc::parse(NOTE);
    let a = doc.leaves[5].source.clone();
    let b = doc.leaves[6].source.clone();
    assert_eq!(doc.next_stop(NOTE, a.end), b.start);
    assert_eq!(doc.prev_stop(NOTE, b.start), a.end);
}

#[test]
fn a_caret_outside_every_leaf_snaps_to_the_nearest_boundary() {
    let doc = Doc::parse(NOTE);
    // Inside "- " before "one".
    let one = doc.leaves[2].source.start;
    assert_eq!(doc.snap(one - 1, false), one);
    assert_eq!(doc.snap(one - 1, true), doc.leaves[1].source.end);
}

#[test]
fn every_caret_stop_is_inside_a_leaf_and_round_trips() {
    let doc = Doc::parse(NOTE);
    let mut at = doc.leaves[0].source.start;
    let mut stops = 0;
    loop {
        let leaf = doc.leaf_at(at).expect("a stop is in a leaf");
        let views = doc.views(NOTE, Selection::caret(at));
        let local = at - doc.leaves[leaf].source.start;
        let view = &views[leaf];
        assert_eq!(view.to_source(view.to_display(local)), local, "stop {at}");
        let next = doc.next_stop(NOTE, at);
        if next == at {
            break;
        }
        assert!(next > at);
        at = next;
        stops += 1;
    }
    assert!(stops > 40);
}

#[test]
fn code_is_verbatim_and_prose_conceals_unless_touched() {
    let doc = Doc::parse(NOTE);
    let far = Selection::caret(0);
    let views = doc.views(NOTE, far);
    assert_eq!(views[1].display_text(), "Some bold text\nwrapped on.");
    assert_eq!(views[4].display_text(), "let x = 1;");
    // The caret on the heading reveals its `#`.
    assert_eq!(views[0].display_text(), "# Title");
    let inside_bold = doc.leaves[1].source.start + 7;
    let views = doc.views(NOTE, Selection::caret(inside_bold));
    assert_eq!(views[1].display_text(), "Some **bold** text\nwrapped on.");
    assert_eq!(views[0].display_text(), "Title");
}

#[test]
fn a_short_table_row_has_only_the_cells_it_has() {
    let source = "| a | b | c |\n|---|---|---|\n| d |\n| e | f |\n\nafter";
    let doc = Doc::parse(source);
    assert_eq!(
        doc.blocks[0].kind,
        Kind::Table {
            columns: 3,
            rows: vec![3, 1, 2]
        }
    );
    // Right steps from "d" (a one-cell row) to "e" on the next row.
    let d = doc.leaves[3].source.clone();
    assert_eq!(&source[d.clone()], "d");
    assert_eq!(doc.next_stop(source, d.end), doc.leaves[4].source.start);
}

#[test]
fn three_dashes_are_a_horizontal_rule() {
    let doc = Doc::parse("above\n\n---\n\nbelow");
    let kinds: Vec<_> = doc.blocks.iter().map(|b| b.kind.clone()).collect();
    assert_eq!(kinds, vec![Kind::Paragraph, Kind::Rule, Kind::Paragraph]);
}

#[test]
fn a_line_starting_with_a_pipe_is_a_table_only_with_a_delimiter_row() {
    let doc = Doc::parse("| just text");
    assert_eq!(doc.blocks[0].kind, Kind::Paragraph);
    let doc = Doc::parse("| a |\n| b |");
    assert_eq!(doc.blocks[0].kind, Kind::Paragraph);
    let doc = Doc::parse("| a |\n|---|\n| b |");
    assert!(matches!(doc.blocks[0].kind, Kind::Table { .. }));
}
