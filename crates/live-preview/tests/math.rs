//! Math through the Document facade: inline `$…$` and a `$$…$$` block are
//! each one object (`U+FFFC`) to typeset until the caret is in them; what
//! isn't math stays text. Expected values are worked out by hand.

use live_preview::Document;
use live_preview::blocks::Kind;
use live_preview::conceal::{Math, OBJECT};

#[test]
fn inline_math_is_one_object_until_the_caret_is_in_it() {
    let src = "Area $\\pi r^2$ here\n\nEnd";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    let view = &d.views()[0];
    assert_eq!(view.display_text(), format!("Area {OBJECT} here"));
    assert_eq!(
        view.maths(),
        [Math {
            at: 5,
            latex: "\\pi r^2".into(),
            display: false,
        }]
    );
    d.set_caret(src.find("pi").unwrap());
    assert_eq!(d.views()[0].display_text(), "Area $\\pi r^2$ here");
    assert!(d.views()[0].maths().is_empty());
}

#[test]
fn dollars_that_aren_t_math_stay_text() {
    // Each on a line of its own: a `$` only closes on its line.
    for text in [
        "$5 and $10",
        "a $ spaced $ pair",
        "\\$y$ escaped",
        "`$z$` in code",
        "a$$b mid-line",
    ] {
        let mut d = Document::new(&format!("{text}\n\nEnd"));
        d.move_doc_end(false);
        assert!(d.views()[0].maths().is_empty(), "{text}");
    }
}

#[test]
fn a_math_block_is_one_object_until_the_caret_is_in_it() {
    let src = "Before\n\n$$\n\\sum_i x_i\n$$\n\n$$e = mc^2$$\n\nEnd";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    let kinds: Vec<Kind> = d.blocks().blocks.iter().map(|b| b.kind.clone()).collect();
    assert_eq!(
        kinds,
        [Kind::Paragraph, Kind::Math, Kind::Math, Kind::Paragraph]
    );
    let views = d.views();
    assert_eq!(views[1].display_text(), OBJECT);
    assert_eq!(
        views[1].maths(),
        [Math {
            at: 0,
            latex: "\\sum_i x_i".into(),
            display: true,
        }]
    );
    assert_eq!(views[2].maths()[0].latex, "e = mc^2");
    d.set_caret(src.find("sum").unwrap());
    assert_eq!(d.views()[1].display_text(), "$$\n\\sum_i x_i\n$$");
}

#[test]
fn a_math_block_closes_before_a_blank_line_or_isn_t_one() {
    // Text after the closing `$$`, a price, and `$$` with no close before
    // the blank line: all prose, the note after them untouched.
    for src in ["$$a$$ more\n\nEnd", "$$5 off today\n\nEnd", "$$\nx\n\nEnd"] {
        let d = Document::new(src);
        let kinds: Vec<Kind> = d.blocks().blocks.iter().map(|b| b.kind.clone()).collect();
        assert!(!kinds.contains(&Kind::Math), "{src:?}: {kinds:?}");
        assert_eq!(kinds.last(), Some(&Kind::Paragraph), "{src:?}");
    }
}
