//! The Live Preview document model through its facade: edits and key
//! commands in, then the source text, the display layout and the caret
//! out, all in source offsets. Expected values are worked out by hand.

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use live_preview::Document;

/// A clock the test moves by hand.
#[derive(Clone)]
struct Clock(Rc<Cell<Instant>>);

impl Clock {
    /// Starts at the time it is now.
    fn new() -> Self {
        Clock(Rc::new(Cell::new(Instant::now())))
    }

    /// Moves time on by `ms` milliseconds.
    fn wait(&self, ms: u64) {
        self.0.set(self.0.get() + Duration::from_millis(ms));
    }
}

/// A document over `text` on a clock the test moves.
fn doc(text: &str) -> (Document, Clock) {
    let clock = Clock::new();
    let time = clock.0.clone();
    (
        Document::with_clock(text, Box::new(move || time.get())),
        clock,
    )
}

#[test]
fn typing_inserts_at_the_caret_and_moves_it_on() {
    let (mut d, _) = doc("Hello world");
    d.set_caret(5);
    d.type_text(",");
    assert_eq!(d.text(), "Hello, world");
    assert_eq!(d.caret(), 6);
    d.type_text(" big");
    assert_eq!(d.text(), "Hello, big world");
    assert_eq!(d.caret(), 10);
}

#[test]
fn undo_takes_back_one_burst_of_typing_at_a_time() {
    let (mut d, clock) = doc("");
    for c in ["o", "n", "e"] {
        d.type_text(c);
        clock.wait(100);
    }
    // A pause of 300 ms or more ends the burst.
    clock.wait(300);
    for c in [" ", "t", "w", "o"] {
        d.type_text(c);
        clock.wait(100);
    }
    assert_eq!(d.text(), "one two");
    assert!(d.undo());
    assert_eq!(d.text(), "one");
    assert_eq!(d.caret(), 3);
    assert!(d.undo());
    assert_eq!(d.text(), "");
    assert!(!d.undo(), "nothing left to undo");
    assert!(d.redo());
    assert_eq!(d.text(), "one");
    assert!(d.redo());
    assert_eq!((d.text(), d.caret()), ("one two", 7));
    assert!(!d.redo());
}

#[test]
fn a_change_of_edit_kind_starts_a_new_undo_step() {
    let (mut d, clock) = doc("");
    d.type_text("abc");
    clock.wait(50);
    d.backspace();
    clock.wait(50);
    d.backspace();
    clock.wait(50);
    d.type_text("Z");
    assert_eq!(d.text(), "aZ");
    assert!(d.undo());
    assert_eq!(d.text(), "a", "the typing after the deletes");
    assert!(d.undo());
    assert_eq!(d.text(), "abc", "both deletes at once");
    assert!(d.undo());
    assert_eq!(d.text(), "");
}

#[test]
fn a_paste_is_always_an_undo_step_of_its_own() {
    let (mut d, _) = doc("");
    d.type_text("a");
    d.paste("PASTED");
    d.type_text("b");
    assert_eq!(d.text(), "aPASTEDb");
    d.undo();
    assert_eq!(d.text(), "aPASTED");
    d.undo();
    assert_eq!(d.text(), "a");
}

#[test]
fn undo_puts_back_the_selection_the_edit_replaced() {
    let (mut d, _) = doc("keep this word");
    d.select(5..9);
    d.type_text("that");
    assert_eq!(d.text(), "keep that word");
    d.undo();
    assert_eq!(d.text(), "keep this word");
    assert_eq!(d.selection(), 5..9);
}

#[test]
fn typing_after_an_undo_drops_the_redo() {
    let (mut d, clock) = doc("");
    d.type_text("one");
    clock.wait(500);
    d.type_text(" two");
    d.undo();
    clock.wait(500);
    d.type_text("!");
    assert!(!d.redo());
    assert_eq!(d.text(), "one!");
}

#[test]
fn loading_from_disk_isnt_an_edit_and_forgetting_drops_the_history() {
    let (mut d, _) = doc("first");
    d.set_caret(5);
    d.type_text(" edit");
    d.load("from disk");
    assert_eq!(d.text(), "from disk");
    assert!(!d.undo(), "a reload can't be undone into the old text");
    d.set_caret(9);
    d.type_text("!");
    d.forget_history();
    assert!(!d.undo());
    assert_eq!(d.text(), "from disk!");
}

/// What each leaf shows now.
fn shown(d: &Document) -> Vec<String> {
    d.views()
        .iter()
        .map(|v| v.display_text().to_string())
        .collect()
}

/// A heading, a paragraph with bold, and a last line: the heading is
/// 0..7, the blank line 7..9 separates, the paragraph is 9..27, then the
/// last line 29..32.
const NOTE: &str = "# Title\n\nSome **bold** text\n\nEnd";

#[test]
fn syntax_shows_only_where_the_caret_touches_it() {
    let (mut d, _) = doc(NOTE);
    d.set_caret(31);
    assert_eq!(shown(&d), ["Title", "Some bold text", "End"]);
    // On the heading's line, anywhere: its `#` shows.
    d.set_caret(4);
    assert_eq!(shown(&d), ["# Title", "Some bold text", "End"]);
    // Inside the bold, or touching it: its markers show.
    d.set_caret(17);
    assert_eq!(shown(&d), ["Title", "Some **bold** text", "End"]);
    d.set_caret(14);
    assert_eq!(shown(&d)[1], "Some **bold** text");
    d.set_caret(13);
    assert_eq!(shown(&d)[1], "Some bold text");
    // A selection reaching into it does too.
    d.select(10..16);
    assert_eq!(shown(&d)[1], "Some **bold** text");
}

#[test]
fn right_and_left_cross_blocks_without_stopping_between_them() {
    let (mut d, _) = doc(NOTE);
    d.set_caret(7);
    d.move_right(false);
    assert_eq!(
        d.caret(),
        9,
        "the end of the heading to the paragraph's start"
    );
    d.move_left(false);
    assert_eq!(d.caret(), 7);
    // A caret between blocks snaps into the next one.
    d.set_caret(8);
    assert_eq!(d.caret(), 9);
}

#[test]
fn word_movement_steps_over_words_and_on_into_the_next_block() {
    let (mut d, _) = doc(NOTE);
    d.set_caret(9);
    d.move_word_right(false);
    assert_eq!(d.caret(), 13, "the end of Some");
    d.move_word_right(false);
    assert_eq!(d.caret(), 20, "past ** to the end of bold");
    d.move_word_right(false);
    assert_eq!(d.caret(), 27, "the end of text");
    d.move_word_right(false);
    assert_eq!(d.caret(), 29, "into the last block");
    d.move_word_left(false);
    assert_eq!(d.caret(), 27);
    d.move_word_left(false);
    assert_eq!(d.caret(), 23, "the start of text");
}

#[test]
fn home_and_end_go_to_the_line_s_ends_within_its_block() {
    let (mut d, _) = doc("one\ntwo three\n\nlast");
    // A paragraph of two lines, 0..13.
    d.set_caret(6);
    d.move_line_start(false);
    assert_eq!(d.caret(), 4);
    d.move_line_end(true);
    assert_eq!(d.selection(), 4..13);
    d.move_doc_end(false);
    assert_eq!(d.caret(), 19);
    d.move_doc_start(true);
    assert_eq!(d.selection(), 0..19);
}

#[test]
fn shift_selection_grows_from_its_anchor_either_way() {
    let (mut d, _) = doc("abcdef");
    d.set_caret(3);
    d.move_right(true);
    d.move_right(true);
    assert_eq!(d.selection(), 3..5);
    d.move_left(true);
    d.move_left(true);
    d.move_left(true);
    assert_eq!(d.selection(), 2..3);
    assert_eq!(d.caret(), 2);
    // Without Shift, Left goes to the selection's start.
    d.move_left(false);
    assert_eq!(d.selection(), 2..2);
}

#[test]
fn double_click_selects_a_word_and_triple_click_a_line() {
    let (d, _) = doc("Some **bold** text\nnext line");
    assert_eq!(d.word_at(8), 7..11, "bold, not its markers");
    assert_eq!(d.word_at(4), 0..4, "touching the end of Some");
    assert_eq!(d.line_range(20), 19..28);
}

#[test]
fn backspace_and_delete_remove_one_character_or_the_selection() {
    let (mut d, _) = doc("naïve café");
    d.set_caret(4);
    d.backspace();
    assert_eq!(d.text(), "nave café", "ï is two bytes, one character");
    assert_eq!(d.caret(), 2);
    d.delete();
    assert_eq!(d.text(), "nae café");
    d.select(4..9);
    d.delete();
    assert_eq!(d.text(), "nae ");
    d.backspace_word();
    assert_eq!(d.text(), "");
}

#[test]
fn lists_code_and_tables_show_as_their_source() {
    let note = "- one\n- **two**\n\n```\nlet **x** = 1;\n```\n\n| a | b |\n|---|---|\n| c | d |";
    let (mut d, _) = doc(note);
    d.move_doc_end(false);
    assert_eq!(
        shown(&d),
        [
            "- one\n- **two**",
            "```\nlet **x** = 1;\n```",
            "| a | b |\n|---|---|\n| c | d |",
        ]
    );
}

#[test]
fn front_matter_is_shown_as_its_source_and_never_a_rule() {
    let (d, _) = doc("---\ncreated: 2026-11-05T14:32\ntags: []\n---\nBody **b**");
    assert_eq!(
        shown(&d),
        ["---\ncreated: 2026-11-05T14:32\ntags: []\n---", "Body b"]
    );
}

#[test]
fn text_edited_in_source_mode_comes_back_as_one_undo_step() {
    let (mut d, _) = doc("one two three");
    d.take_text("one 2 three");
    assert_eq!(d.text(), "one 2 three");
    assert_eq!(d.caret(), 5, "after what changed");
    d.undo();
    assert_eq!(d.text(), "one two three");
}

#[test]
fn line_and_column_count_the_source() {
    let (mut d, _) = doc("# Head\né **b**");
    d.set_caret(12);
    assert_eq!(d.line_column(), (1, 4), "é is one column");
    assert_eq!(d.line_start(1), 7);
    assert_eq!(d.line_start(9), 7, "past the last line: the last line");
}

#[test]
fn ime_offsets_convert_between_bytes_and_utf16() {
    let (d, _) = doc("日本語 🙂x");
    // 日本語 is 9 bytes, 3 UTF-16 units; the space 1; 🙂 4 bytes, 2 units.
    assert_eq!(d.to_utf16(10), 4);
    assert_eq!(d.to_utf16(14), 6);
    assert_eq!(d.from_utf16(6), 14);
}

/// Notes mixing every block the model knows.
const FIXTURES: &[&str] = &[
    NOTE,
    "---\ntags: [a]\n---\n# H **b**\n\npara *i* [[link|alias]] `c`\nmore\n\n\n\n- a\n- [x] b\n\n> quote\n\n```rs\nx\n```\n\n| a | b |\n|---|---|\n| c | |\n\n---\nend",
    "日本語の **太字** と [[リンク|別名]]\n\n## 見出し",
    "",
    "\n\n\n",
    "unclosed **bold and [link( and [[wiki",
    "a **bold *it*** ==hi== ~~x~~ \\*esc\\* ``co`de`` [[N#H]] [[#H]] [**t**](u.md) <https://a.b> https://c.d #tag",
    "***x*** *a **b** c* [[N#H|al]] ![[e.png]] [x](<a b.md> \"t\")",
];

/// From the first stop to the last, every caret stop is inside a leaf and
/// round-trips through what that leaf shows with the caret there: the
/// caret never lands in hidden text.
#[test]
fn no_caret_stop_in_any_note_lands_in_hidden_text() {
    for note in FIXTURES {
        let (mut d, _) = doc(note);
        d.move_doc_start(false);
        let mut stops = 0;
        loop {
            let at = d.caret();
            let leaf = d.blocks().leaf_at(at).expect("a stop is in a leaf");
            let start = d.blocks().leaves[leaf].source.start;
            let view = &d.views()[leaf];
            assert_eq!(
                start + view.to_source(view.to_display(at - start)),
                at,
                "stop {at} in {note:?}"
            );
            d.move_right(false);
            if d.caret() == at {
                break;
            }
            assert!(d.caret() > at);
            stops += 1;
        }
        assert!(stops >= note.chars().count() / 2, "{note:?}: {stops} stops");
    }
}

#[test]
fn an_ime_composition_is_one_undo_step_however_long_it_pauses() {
    let (mut d, clock) = doc("");
    d.type_text("a ");
    clock.wait(500);
    // Composing にほん: the preedit grows, with long pauses between keys,
    // then commits as 日本.
    d.compose(2..2, "に");
    clock.wait(400);
    d.compose(2..5, "にほ");
    clock.wait(400);
    d.compose(2..8, "にほん");
    clock.wait(400);
    d.commit_composition(2..11, "日本");
    assert_eq!(d.text(), "a 日本");
    clock.wait(500);
    d.type_text("!");
    d.undo();
    assert_eq!(d.text(), "a 日本");
    d.undo();
    assert_eq!(d.text(), "a ", "the whole composition at once");
}

#[test]
fn two_compositions_are_two_undo_steps() {
    let (mut d, _) = doc("");
    d.compose(0..0, "に");
    d.commit_composition(0..3, "二");
    d.compose(3..3, "ほ");
    d.commit_composition(3..6, "保");
    assert_eq!(d.text(), "二保");
    d.undo();
    assert_eq!(d.text(), "二");
}
