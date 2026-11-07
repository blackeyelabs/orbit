//! Callouts through the Document facade: a quote whose first line is
//! `[!type]` is read as one, its title line showing just its title, and
//! the whole callout's source shows while the caret is anywhere in it.
//! Expected values are worked out by hand.

use live_preview::Document;
use live_preview::blocks::{CalloutLine, Kind};
use md_engine::CalloutKind;

/// What each leaf shows.
fn shown(d: &Document) -> Vec<String> {
    d.views()
        .iter()
        .map(|v| v.display_text().to_string())
        .collect()
}

/// A callout line of `kind`: its title line, or one of its body.
fn line(kind: CalloutKind, title: bool) -> Kind {
    Kind::Callout(CalloutLine { kind, title })
}

#[test]
fn a_callout_shows_its_title_and_body_without_markers() {
    let src = "> [!warning]- Mind **this**\n> body *line*\n\n> plain quote\n\nafter";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    let kinds: Vec<Kind> = d.blocks().blocks.iter().map(|b| b.kind.clone()).collect();
    assert_eq!(
        kinds,
        [
            line(CalloutKind::Warning, true),
            line(CalloutKind::Warning, false),
            Kind::Quote(1),
            Kind::Paragraph,
        ]
    );
    assert_eq!(
        shown(&d),
        ["Mind this", "body line", "plain quote", "after"]
    );
}

#[test]
fn a_callout_with_no_title_shows_its_type() {
    let mut d = Document::new("> [!faq]\n> why\n\nafter");
    d.move_doc_end(false);
    assert_eq!(shown(&d), ["Faq", "why", "after"]);
}

#[test]
fn the_whole_callout_shows_its_source_while_the_caret_is_in_it() {
    let src = "> [!note] Title\n> one\n> two\n\nafter";
    let mut d = Document::new(src);
    d.set_caret(src.find("two").unwrap() + 1);
    assert_eq!(shown(&d), ["> [!note] Title", "> one", "> two", "after"]);
    assert!(d.reveals(0), "its title line too");
}

#[test]
fn a_type_marker_inside_a_plain_quote_is_just_quoted() {
    let d = Document::new("> plain\n> [!note] not a callout\n\nafter");
    let kinds: Vec<Kind> = d.blocks().blocks.iter().map(|b| b.kind.clone()).collect();
    assert_eq!(kinds, [Kind::Quote(1), Kind::Quote(1), Kind::Paragraph]);
}

#[test]
fn enter_in_a_callout_goes_on_with_a_plain_quote_line() {
    for (src, caret, want) in [
        ("> [!note] Title", 15, "> [!note] Title\n> "),
        ("> [!note]", 9, "> [!note]\n> "),
        ("> [!note] T\n> body", 18, "> [!note] T\n> body\n> "),
    ] {
        let mut d = Document::new(src);
        d.set_caret(caret);
        d.newline();
        assert_eq!(d.text(), want, "{src:?}");
    }
}
