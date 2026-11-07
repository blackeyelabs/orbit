//! Tables through the Document facade: each cell its own text, edited in
//! place; Tab, Shift+Tab and Enter moving between cells (and adding rows);
//! a typed `|` kept inside its cell. Expected values are worked out by
//! hand.

use live_preview::Document;
use live_preview::blocks::{Align, Kind, Table};

const TABLE: &str = "| Name | **Qty** |\n|:-----|------:|\n| tea  | 2 |\n| jam |\n\nafter";

/// What each leaf shows.
fn shown(d: &Document) -> Vec<String> {
    d.views()
        .iter()
        .map(|v| v.display_text().to_string())
        .collect()
}

#[test]
fn each_cell_is_its_own_text_its_syntax_concealed() {
    let mut d = Document::new(TABLE);
    d.move_doc_end(false);
    assert_eq!(
        d.blocks().blocks[0].kind,
        Kind::Table(Table {
            align: vec![Align::Left, Align::Right],
            // A short row keeps only the cells it has.
            rows: vec![2, 2, 1],
        })
    );
    assert_eq!(shown(&d), ["Name", "Qty", "tea ", "2", "jam", "after"]);
}

#[test]
fn typing_in_a_cell_changes_that_cell_s_source() {
    let mut d = Document::new(TABLE);
    d.set_caret(TABLE.find("2").unwrap() + 1);
    d.type_text("0");
    assert_eq!(
        d.text(),
        "| Name | **Qty** |\n|:-----|------:|\n| tea  | 20 |\n| jam |\n\nafter"
    );
    // Into an empty cell, too.
    let mut d = Document::new("| a | b |\n|---|---|\n|  | x |");
    d.set_caret(d.text().find("|  |").unwrap() + 2);
    d.type_text("new");
    assert_eq!(d.text(), "| a | b |\n|---|---|\n| new | x |");
}

#[test]
fn tab_and_shift_tab_move_cell_to_cell() {
    let mut d = Document::new(TABLE);
    d.set_caret(TABLE.find("Name").unwrap());
    assert!(d.tab_in_table(false));
    assert_eq!(
        d.caret(),
        TABLE.find("Qty**").unwrap() + 5,
        "the next cell's end"
    );
    assert!(d.tab_in_table(false));
    assert_eq!(
        d.caret(),
        TABLE.find("tea").unwrap() + 4,
        "on to the next row"
    );
    assert!(d.tab_in_table(true));
    assert_eq!(d.caret(), TABLE.find("Qty**").unwrap() + 5);
    // Moving isn't an edit.
    assert!(!d.undo());
    // Outside a table, Tab is left to the rest.
    d.move_doc_end(false);
    assert!(!d.tab_in_table(false));
}

#[test]
fn tab_in_the_last_cell_adds_a_row() {
    let src = "| a | b |\n|---|---|\n| c | d |\n\nafter";
    let mut d = Document::new(src);
    d.set_caret(src.find("d").unwrap());
    assert!(d.tab_in_table(false));
    assert_eq!(
        d.text(),
        "| a | b |\n|---|---|\n| c | d |\n|  |  |\n\nafter"
    );
    assert_eq!(d.caret(), src.find("d |").unwrap() + 6, "in its first cell");
    d.type_text("e");
    assert_eq!(
        d.text(),
        "| a | b |\n|---|---|\n| c | d |\n| e |  |\n\nafter"
    );
    assert!(d.undo());
    assert!(d.undo());
    assert_eq!(d.text(), src, "the row is an undo step");
}

#[test]
fn enter_goes_down_a_column_adding_a_row_at_the_bottom() {
    let src = "| a | b |\n|---|---|\n| c | d |";
    let mut d = Document::new(src);
    d.set_caret(src.find("b").unwrap());
    d.newline();
    assert_eq!(d.text(), src, "no line break");
    assert_eq!(d.caret(), src.find("d").unwrap() + 1);
    d.newline();
    assert_eq!(d.text(), "| a | b |\n|---|---|\n| c | d |\n|  |  |");
    assert_eq!(d.caret(), src.len() + 6, "in the new row's second cell");
}

#[test]
fn a_pipe_typed_in_a_cell_stays_in_it() {
    let src = "| a | b |\n|---|---|\n| c | d |";
    let mut d = Document::new(src);
    d.set_caret(src.find("c").unwrap() + 1);
    d.type_text("|x");
    assert_eq!(d.text(), "| a | b |\n|---|---|\n| c\\|x | d |");
    d.move_doc_end(false);
    assert_eq!(d.views()[2].display_text(), "c|x");
}

#[test]
fn backspace_and_delete_stop_at_a_cell_s_edges() {
    let src = "| a | b |\n|---|---|\n| c | d |";
    let mut d = Document::new(src);
    d.set_caret(src.find("b").unwrap());
    d.backspace();
    d.backspace_word();
    assert_eq!(d.text(), src, "the pipe before stays");
    d.set_caret(src.find("c").unwrap() + 1);
    d.delete();
    d.delete_word();
    assert_eq!(d.text(), src, "the pipe after stays");
    // Inside a cell, they delete as ever.
    d.backspace();
    assert_eq!(d.text(), "| a | b |\n|---|---|\n|  | d |");
}

#[test]
fn a_pipe_from_an_input_method_stays_in_its_cell_too() {
    let src = "| a | b |\n|---|---|\n| c | d |";
    let mut d = Document::new(src);
    let at = src.find("c").unwrap() + 1;
    d.set_caret(at);
    d.commit_composition(at..at, "|");
    assert_eq!(d.text(), "| a | b |\n|---|---|\n| c\\| | d |");
}

#[test]
fn only_an_unescaped_pipe_divides_cells() {
    // `\\|` is a backslash, then a real pipe; a row may end without one.
    let d = Document::new("| a | b |\n|---|---|\n| x\\\\| y\n| p | q\\|\n\nend");
    let mut d = d;
    d.move_doc_end(false);
    let shown: Vec<String> = d
        .views()
        .iter()
        .map(|v| v.display_text().to_string())
        .collect();
    assert_eq!(shown[..6], ["a", "b", "x\\", "y", "p", "q|"]);
}

#[test]
fn a_delimiter_row_without_a_leading_pipe_still_makes_a_table() {
    let mut d = Document::new("| a | b |\n-|-\n| c | d |");
    d.move_doc_end(false);
    assert!(matches!(d.blocks().blocks[0].kind, Kind::Table(_)));
    assert_eq!(d.views()[3].display_text(), "d");
}

#[test]
fn a_row_added_at_the_bottom_goes_after_the_table_s_last_line() {
    // The last row has no cells: the new row still goes below it.
    let src = "| a |\n|---|\n| c |\n|";
    let mut d = Document::new(src);
    d.set_caret(src.find("c").unwrap());
    assert!(d.tab_in_table(false));
    assert_eq!(d.text(), "| a |\n|---|\n| c |\n|\n|  |");
}
