//! Spike 4's checks 1-5 in a headless window: rendering through Kit's
//! `editor_host`, the caret across blocks, clicks, drag-selection, typing
//! into a cell, and IME geometry. Driven through simulated keys, mouse and
//! platform input, as a person would.

use gpui::{
    Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext, VisualTestContext, point, px,
};
use spike4_editor_host::host::{self, Editor};

/// A note with one of each block the spike renders.
const NOTE: &str = "# Title\n\nSome **bold** text that runs on long enough to wrap onto a second visual line in a narrow window, at least.\n\n- one\n- two\n\n```rust\nlet x = 1;\n```\n\n| a | b |\n|---|---|\n| c |  |\n";

// NOTE's leaves, in order.
/// `# Title`.
const HEADING: usize = 0;
/// The paragraph.
const PARAGRAPH: usize = 1;
/// List item "one".
const ONE: usize = 2;
/// List item "two".
const TWO: usize = 3;
/// The code block.
const CODE: usize = 4;
/// Header cell "a".
const A: usize = 5;
/// Header cell "b".
const B: usize = 6;
/// Cell "c", below "a".
const C: usize = 7;
/// The empty cell below "b".
const EMPTY_CELL: usize = 8;
/// The empty last line after the table.
const END: usize = 9;

/// Opens `source` in a focused editor in a headless window, drawn once.
fn open<'a>(
    cx: &'a mut TestAppContext,
    source: &str,
) -> (Entity<Editor>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        host::bind_keys(cx);
    });
    let (editor, vcx) = cx.add_window_view(|_, cx| Editor::new(source, cx));
    vcx.update(|window, cx| {
        let focus = editor.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
    });
    settle(vcx);
    (editor, vcx)
}

/// Runs pending work and draws a frame, so geometry is current.
fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
}

/// The leaf the caret is in.
fn leaf_of(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> usize {
    editor.read_with(cx, |e, _| {
        e.doc().leaf_at(e.cursor_offset()).expect("caret in a leaf")
    })
}

/// Presses keys, then draws.
fn press(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    settle(cx);
}

/// A point inside a leaf: `fx` of the way across, on its first line.
fn inside(
    editor: &Entity<Editor>,
    cx: &mut VisualTestContext,
    leaf: usize,
    fx: f32,
) -> Point<Pixels> {
    editor.read_with(cx, |e, _| {
        let g = e.leaf_geometry(leaf).expect("leaf was laid out");
        let b = g.bounds();
        point(b.left() + b.size.width * fx, b.top() + g.line_height() / 2.)
    })
}

#[gpui::test]
fn every_leaf_renders_and_headings_are_taller(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    editor.read_with(cx, |e, _| {
        for leaf in 0..e.doc().leaves.len() {
            assert!(e.leaf_geometry(leaf).is_some(), "leaf {leaf} has geometry");
        }
        let heading = e.leaf_geometry(HEADING).unwrap().line_height();
        let body = e.leaf_geometry(PARAGRAPH).unwrap().line_height();
        assert!(heading > body, "heading {heading:?} vs body {body:?}");
        // Cells of one row share a line; the second column is to the right.
        let a = e.leaf_geometry(A).unwrap().bounds();
        let b = e.leaf_geometry(B).unwrap().bounds();
        assert_eq!(a.top(), b.top());
        assert!(b.left() > a.right() - px(1.));
        // The caret starts on the heading, so its `#` shows; bold elsewhere
        // is concealed.
        let texts = e.display_texts();
        assert_eq!(texts[HEADING], "# Title");
        assert!(texts[PARAGRAPH].starts_with("Some bold text"));
    });
}

#[gpui::test]
fn down_visits_every_block_in_order_and_up_comes_back(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    let mut visited = vec![leaf_of(&editor, cx)];
    for _ in 0..12 {
        press(cx, "down");
        let leaf = leaf_of(&editor, cx);
        if visited.last() != Some(&leaf) {
            visited.push(leaf);
        }
    }
    // Down from cell "a" goes to "c" below it, not to "b" beside it, then
    // past the table to the note's empty last line.
    assert_eq!(visited, vec![HEADING, PARAGRAPH, ONE, TWO, CODE, A, C, END]);
    let mut back = vec![leaf_of(&editor, cx)];
    for _ in 0..12 {
        press(cx, "up");
        let leaf = leaf_of(&editor, cx);
        if back.last() != Some(&leaf) {
            back.push(leaf);
        }
    }
    assert_eq!(back, vec![END, C, A, CODE, TWO, ONE, PARAGRAPH, HEADING]);
}

#[gpui::test]
fn down_moves_between_the_wrapped_lines_of_a_paragraph(cx: &mut TestAppContext) {
    // The test window is wide, so the paragraph is long enough to wrap there.
    let long = "word ".repeat(400);
    let (editor, cx) = open(cx, &format!("# T\n\n{long}\n\n- after\n"));
    press(cx, "down");
    let mut tops = vec![editor.read_with(cx, |e, _| e.caret_point().unwrap().y)];
    for _ in 0..50 {
        if leaf_of(&editor, cx) != PARAGRAPH {
            break;
        }
        press(cx, "down");
        tops.push(editor.read_with(cx, |e, _| e.caret_point().unwrap().y));
    }
    let lines = editor.read_with(cx, |e, _| {
        let g = e.leaf_geometry(PARAGRAPH).unwrap();
        (g.bounds().size.height / g.line_height()).round() as usize
    });
    assert!(lines >= 2, "the paragraph wraps");
    // One Down per visual line, each lower than the last, then the list.
    assert_eq!(tops.len(), lines + 1);
    assert!(tops.windows(2).all(|w| w[1] > w[0]));
    assert_eq!(leaf_of(&editor, cx), ONE);
}

#[gpui::test]
fn right_crosses_from_one_block_into_the_next(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    // To the end of "# Title": the caret is on the heading, so `# ` shows.
    for _ in 0.."# Title".len() {
        press(cx, "right");
    }
    assert_eq!(leaf_of(&editor, cx), HEADING);
    let end = editor.read_with(cx, |e, _| e.caret_point().unwrap());
    press(cx, "right");
    assert_eq!(leaf_of(&editor, cx), PARAGRAPH);
    let next = editor.read_with(cx, |e, _| e.caret_point().unwrap());
    assert!(next.y > end.y);
    press(cx, "left");
    assert_eq!(leaf_of(&editor, cx), HEADING);
}

#[gpui::test]
fn a_click_places_the_caret_in_any_block(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    for leaf in [PARAGRAPH, TWO, CODE, B, C, EMPTY_CELL, HEADING] {
        let at = inside(&editor, cx, leaf, 0.5);
        cx.simulate_click(at, Modifiers::none());
        settle(cx);
        assert_eq!(leaf_of(&editor, cx), leaf, "clicked leaf {leaf}");
    }
}

#[gpui::test]
fn dragging_selects_across_blocks(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    let from = inside(&editor, cx, PARAGRAPH, 0.1);
    let to = inside(&editor, cx, TWO, 0.9);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    settle(cx);
    editor.read_with(cx, |e, _| {
        let sel = e.selected_range();
        let leaves = &e.doc().leaves;
        assert!(leaves[PARAGRAPH].source.contains(&sel.start));
        assert!(leaves[TWO].source.start < sel.end && sel.end <= leaves[TWO].source.end);
        assert!(NOTE[sel].contains("- one\n- t"));
        // The highlight covers exactly the leaves the selection crosses.
        let lit: Vec<usize> = e.selection_highlights().iter().map(|(l, _)| *l).collect();
        assert_eq!(lit, vec![PARAGRAPH, ONE, TWO]);
    });
}

#[gpui::test]
fn right_moves_from_one_cell_into_the_next(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    let at = inside(&editor, cx, A, 1.0);
    cx.simulate_click(at, Modifiers::none());
    settle(cx);
    assert_eq!(leaf_of(&editor, cx), A);
    press(cx, "right");
    assert_eq!(leaf_of(&editor, cx), B);
    press(cx, "right");
    press(cx, "right");
    assert_eq!(
        leaf_of(&editor, cx),
        C,
        "past the end of a row: the next row"
    );
    press(cx, "left");
    assert_eq!(leaf_of(&editor, cx), B);
}

#[gpui::test]
fn typing_into_a_table_cell_edits_that_cell(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    let at = inside(&editor, cx, EMPTY_CELL, 0.5);
    cx.simulate_click(at, Modifiers::none());
    settle(cx);
    cx.simulate_input("xy");
    settle(cx);
    editor.read_with(cx, |e, _| {
        assert!(e.source().contains("| c | xy |"), "{}", e.source());
        assert_eq!(e.doc().leaf_at(e.cursor_offset()), Some(EMPTY_CELL));
    });
    // And into a paragraph.
    let at = inside(&editor, cx, PARAGRAPH, 0.0);
    cx.simulate_click(at, Modifiers::none());
    settle(cx);
    cx.simulate_input("So ");
    settle(cx);
    editor.read_with(cx, |e, _| {
        assert!(e.source().contains("\n\nSo Some **bold**"))
    });
}

/// Composes "にほ" at the caret through the platform input calls, checks
/// the preedit's rectangle starts at the caret, commits "日本", and checks
/// it landed where the caret was.
fn compose_at_caret(editor: &Entity<Editor>, cx: &mut VisualTestContext) {
    use gpui::EntityInputHandler as _;
    let caret = editor.read_with(cx, |e, _| e.caret_point().unwrap());
    let before = editor.read_with(cx, |e, _| e.cursor_offset());
    cx.update(|window, cx| {
        editor.update(cx, |e, cx| {
            e.replace_and_mark_text_in_range(None, "にほ", None, window, cx);
        })
    });
    settle(cx);
    let bounds = cx.update(|window, cx| {
        editor.update(cx, |e, cx| {
            let marked = e.marked_text_range(window, cx).expect("preedit is marked");
            e.bounds_for_range(marked, Default::default(), window, cx)
        })
    });
    let bounds = bounds.expect("the IME has a rectangle to anchor to");
    // The candidate window anchors where the caret was.
    assert!(
        (bounds.origin.x - caret.x).abs() < px(1.),
        "{bounds:?} vs {caret:?}"
    );
    assert!((bounds.origin.y - caret.y).abs() < px(1.));
    assert!(bounds.size.width > px(2.), "the preedit has width");
    cx.update(|window, cx| {
        editor.update(cx, |e, cx| {
            e.replace_text_in_range(None, "日本", window, cx)
        })
    });
    settle(cx);
    editor.read_with(cx, |e, _| {
        assert_eq!(&e.source()[before..before + "日本".len()], "日本");
        assert_eq!(e.cursor_offset(), before + "日本".len());
    });
}

#[gpui::test]
fn ime_composes_in_place_in_a_table_cell(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    let at = inside(&editor, cx, C, 1.0);
    cx.simulate_click(at, Modifiers::none());
    settle(cx);
    compose_at_caret(&editor, cx);
    editor.read_with(cx, |e, _| assert!(e.source().contains("| c日本 |")));
}

#[gpui::test]
fn ime_composes_in_place_in_a_paragraph(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    // After "Some " and before the concealed "**bold**".
    let start = editor.read_with(cx, |e, _| e.doc().leaves[PARAGRAPH].source.start);
    editor.update(cx, |e, cx| e.set_caret(start + "Some ".len(), cx));
    settle(cx);
    compose_at_caret(&editor, cx);
    editor.read_with(cx, |e, _| assert!(e.source().contains("Some 日本**bold**")));
}

#[gpui::test]
fn down_walks_past_the_window_and_scrolls_the_caret_into_view(cx: &mut TestAppContext) {
    // Far more blocks than fit: the list lays out only those in view.
    let note: String = (0..400).map(|i| format!("Paragraph {i}\n\n")).collect();
    let (editor, cx) = open(cx, &note);
    let laid_out = editor.read_with(cx, |e, _| {
        (0..e.doc().leaves.len())
            .filter(|&l| e.leaf_geometry(l).is_some())
            .count()
    });
    assert!(
        laid_out < 200,
        "only blocks in view are laid out ({laid_out})"
    );
    for _ in 0..250 {
        press(cx, "down");
    }
    assert_eq!(leaf_of(&editor, cx), 250);
    editor.read_with(cx, |e, _| {
        assert!(
            e.leaf_geometry(250).is_some(),
            "the caret's block is in view"
        );
        assert!(e.caret_point().is_some());
    });
    for _ in 0..250 {
        press(cx, "up");
    }
    assert_eq!(leaf_of(&editor, cx), 0);
}

#[gpui::test]
fn a_long_list_is_laid_out_only_where_it_is_in_view(cx: &mut TestAppContext) {
    // One list block, as the fixture vault's Long.md is.
    let note: String = (0..600).map(|i| format!("- item {i}\n")).collect();
    let (editor, cx) = open(cx, &note);
    let laid_out = editor.read_with(cx, |e, _| {
        (0..e.doc().leaves.len())
            .filter(|&l| e.leaf_geometry(l).is_some())
            .count()
    });
    assert!(
        laid_out < 300,
        "only the list's runs in view are laid out ({laid_out})"
    );
    // Down crosses from one run of items into the next, past the window.
    for _ in 0..300 {
        press(cx, "down");
    }
    assert_eq!(leaf_of(&editor, cx), 300);
    editor.read_with(cx, |e, _| assert!(e.caret_point().is_some()));
}

// Review of the spike (2026-11-04): each test reproduces one reported bug.

#[gpui::test]
fn typing_a_space_in_a_cell_keeps_the_caret_in_that_cell(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    let at = inside(&editor, cx, A, 1.0);
    cx.simulate_click(at, Modifiers::none());
    settle(cx);
    cx.simulate_input(" b");
    settle(cx);
    editor.read_with(cx, |e, _| {
        assert!(e.source().contains("| a b | b |"), "{}", e.source());
        assert_eq!(e.doc().leaf_at(e.cursor_offset()), Some(A));
    });
}

#[gpui::test]
fn the_caret_is_centred_on_its_line(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, NOTE);
    editor.update(cx, |e, cx| {
        let start = e.doc().leaves[PARAGRAPH].source.start;
        e.set_caret(start, cx);
    });
    settle(cx);
    editor.read_with(cx, |e, _| {
        let caret = e.caret_bounds().expect("a caret");
        let g = e.leaf_geometry(PARAGRAPH).unwrap();
        let line_top = g.bounds().top();
        let line_mid = line_top + g.line_height() / 2.;
        let caret_mid = caret.top() + caret.size.height / 2.;
        assert!(
            (caret_mid - line_mid).abs() < px(1.),
            "caret {caret:?} line top {line_top:?} height {:?}",
            g.line_height()
        );
    });
}

#[gpui::test]
fn down_leaves_a_code_block_that_ends_the_note(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "Intro\n\n```\ncode\n```");
    press(cx, "down");
    assert_eq!(leaf_of(&editor, cx), 1, "in the code");
    press(cx, "down");
    assert_ne!(leaf_of(&editor, cx), 1, "Down goes past the code block");
    cx.simulate_input("after");
    settle(cx);
    editor.read_with(cx, |e, _| {
        assert_eq!(e.source(), "Intro\n\n```\ncode\n```\nafter")
    });
}

#[gpui::test]
fn typing_after_an_opening_fence_goes_into_the_code_not_the_language(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "Intro\n\n");
    editor.update(cx, |e, cx| e.set_caret(e.source().len(), cx));
    settle(cx);
    cx.simulate_input("```");
    settle(cx);
    cx.simulate_input("x");
    settle(cx);
    editor.read_with(cx, |e, _| assert_eq!(e.source(), "Intro\n\n```\nx"));
}

// Second review round: lists, tasks and Enter.

/// Opens `source` with the caret at its end.
fn open_at_end<'a>(
    cx: &'a mut TestAppContext,
    source: &str,
) -> (Entity<Editor>, &'a mut VisualTestContext) {
    let (editor, cx) = open(cx, source);
    editor.update(cx, |e, cx| e.set_caret(e.source().len(), cx));
    settle(cx);
    (editor, cx)
}

#[gpui::test]
fn enter_at_a_lines_end_puts_the_caret_on_the_very_next_line(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "abc\n\nnext");
    editor.update(cx, |e, cx| e.set_caret(3, cx));
    settle(cx);
    press(cx, "enter");
    cx.simulate_input("x");
    settle(cx);
    editor.read_with(cx, |e, _| assert_eq!(e.source(), "abc\nx\n\nnext"));
}

#[gpui::test]
fn typing_a_header_and_delimiter_row_makes_a_table(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "Intro\n\n| a | b |\n\nafter");
    let header_end = "Intro\n\n| a | b |".len();
    editor.update(cx, |e, cx| e.set_caret(header_end, cx));
    settle(cx);
    press(cx, "enter");
    cx.simulate_input("|---|---|");
    settle(cx);
    editor.read_with(cx, |e, _| {
        assert_eq!(e.source(), "Intro\n\n| a | b |\n|---|---|\n\nafter");
        assert!(
            e.doc()
                .blocks
                .iter()
                .any(|b| matches!(b.kind, spike4_editor_host::model::Kind::Table { .. }))
        );
    });
}

#[gpui::test]
fn enter_continues_a_list(cx: &mut TestAppContext) {
    for (note, typed) in [
        ("- one", "- one\n- two"),
        ("* one", "* one\n* two"),
        ("1. one", "1. one\n2. two"),
        ("- [ ] one", "- [ ] one\n- [ ] two"),
        ("- [x] one", "- [x] one\n- [ ] two"),
        ("  - one", "  - one\n  - two"),
    ] {
        let mut cx = cx.clone();
        let (editor, cx) = open_at_end(&mut cx, note);
        press(cx, "enter");
        cx.simulate_input("two");
        settle(cx);
        editor.read_with(cx, |e, _| assert_eq!(e.source(), typed, "from {note:?}"));
    }
}

#[gpui::test]
fn enter_on_an_empty_last_item_ends_the_list(cx: &mut TestAppContext) {
    let (editor, cx) = open_at_end(cx, "- one");
    press(cx, "enter");
    press(cx, "enter");
    cx.simulate_input("after");
    settle(cx);
    editor.read_with(cx, |e, _| {
        assert_eq!(e.source(), "- one\n\nafter");
        assert_eq!(
            e.doc().blocks[1].kind,
            spike4_editor_host::model::Kind::Paragraph
        );
    });
}

#[gpui::test]
fn clicking_a_checkbox_ticks_and_unticks_it(cx: &mut TestAppContext) {
    let (editor, cx) = open(cx, "- [ ] task\n- [x] done\n");
    let boxes = editor.read_with(cx, |e, _| {
        [0, 1].map(|leaf| {
            let g = e.leaf_geometry(leaf).unwrap();
            let b = g.bounds();
            // The checkbox Kit draws sits just left of the item's text.
            point(b.left() - px(10.), b.top() + g.line_height() / 2.)
        })
    });
    cx.simulate_click(boxes[0], Modifiers::none());
    settle(cx);
    editor.read_with(cx, |e, _| {
        assert_eq!(e.source(), "- [x] task\n- [x] done\n")
    });
    cx.simulate_click(boxes[1], Modifiers::none());
    settle(cx);
    editor.read_with(cx, |e, _| {
        assert_eq!(e.source(), "- [x] task\n- [ ] done\n")
    });
    assert!(
        editor.read_with(cx, |e, _| e.checkbox_at(boxes[0]).is_some()),
        "a pointer cursor shows over the box"
    );
}
