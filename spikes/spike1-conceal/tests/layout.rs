use spike1_conceal::{Run, Selection, Style, layout};

#[test]
fn plain_text_is_shown_as_is() {
    let l = layout("Orbit keeps notes as plain files", Selection::caret(0));
    assert_eq!(l.display_text(), "Orbit keeps notes as plain files");
}

#[test]
fn bold_markers_are_hidden_when_the_caret_is_elsewhere() {
    let l = layout("Ship **first** now", Selection::caret(0));
    assert_eq!(l.display_text(), "Ship first now");
}

#[test]
fn bold_markers_show_while_the_caret_touches_the_span() {
    let src = "Ship **first** now";
    // Inside the bold text.
    assert_eq!(layout(src, Selection::caret(9)).display_text(), src);
    // On either boundary: touching counts.
    assert_eq!(layout(src, Selection::caret(5)).display_text(), src);
    assert_eq!(layout(src, Selection::caret(14)).display_text(), src);
    // One character away from the span: hidden again.
    assert_eq!(
        layout(src, Selection::caret(4)).display_text(),
        "Ship first now"
    );
    assert_eq!(
        layout(src, Selection::caret(15)).display_text(),
        "Ship first now"
    );
}

#[test]
fn a_selection_overlapping_the_span_reveals_it() {
    let src = "Ship **first** now";
    let sel = Selection { anchor: 0, head: 3 };
    assert_eq!(layout(src, sel).display_text(), "Ship first now");
    let sel = Selection {
        anchor: 16,
        head: 2,
    };
    assert_eq!(layout(src, sel).display_text(), src);
}

// "Ship **first** now" with the caret away: source offsets
//   S0 h1 i2 p3 _4 *5 *6 f7 i8 r9 s10 t11 *12 *13 _14 n15 o16 w17
// display "Ship first now": f is at 5, the space after "first" at 10.
#[test]
fn source_offsets_map_to_display_offsets_skipping_hidden_syntax() {
    let l = layout("Ship **first** now", Selection::caret(0));
    assert_eq!(l.to_display(0), 0);
    assert_eq!(l.to_display(5), 5); // before the opening **
    assert_eq!(l.to_display(6), 5); // inside a hidden marker collapses to its edge
    assert_eq!(l.to_display(7), 5); // the f
    assert_eq!(l.to_display(11), 9); // the t
    assert_eq!(l.to_display(14), 10); // after the closing **
    assert_eq!(l.to_display(18), 14); // end of text
}

#[test]
fn display_offsets_map_to_the_start_of_the_next_visible_source_char() {
    let l = layout("Ship **first** now", Selection::caret(0));
    assert_eq!(l.to_source(0), 0);
    assert_eq!(l.to_source(5), 7); // before f: inside the bold, after **
    assert_eq!(l.to_source(9), 11);
    assert_eq!(l.to_source(10), 14); // before the space: outside the bold
    assert_eq!(l.to_source(14), 18);
}

#[test]
fn italic_markers_are_hidden_with_either_delimiter() {
    let away = Selection::caret(0);
    assert_eq!(layout("a *soft* b", away).display_text(), "a soft b");
    assert_eq!(layout("a _soft_ b", away).display_text(), "a soft b");
    assert_eq!(
        layout("a **bold** and *soft* b", away).display_text(),
        "a bold and soft b"
    );
}

#[test]
fn strikethrough_and_code_markers_are_hidden() {
    let away = Selection::caret(0);
    assert_eq!(layout("x ~~gone~~ y", away).display_text(), "x gone y");
    assert_eq!(
        layout("x `cargo run` y", away).display_text(),
        "x cargo run y"
    );
}

#[test]
fn markdown_inside_inline_code_stays_literal() {
    let l = layout("x `a **b** c` y", Selection::caret(0));
    assert_eq!(l.display_text(), "x a **b** c y");
}

#[test]
fn links_show_only_their_text() {
    let away = Selection::caret(0);
    assert_eq!(
        layout(
            "Read the [Mermaid guide](https://mermaid.js.org) first.",
            away
        )
        .display_text(),
        "Read the Mermaid guide first."
    );
}

#[test]
fn wikilinks_show_their_target_or_alias() {
    let away = Selection::caret(0);
    assert_eq!(
        layout("See [[Mermaid basics]].", away).display_text(),
        "See Mermaid basics."
    );
    assert_eq!(
        layout("See [[Orbit roadmap|the roadmap]].", away).display_text(),
        "See the roadmap."
    );
}

#[test]
fn heading_markers_hide_unless_the_caret_is_on_the_heading_line() {
    let src = "# Orbit roadmap\nBody text\n## Next up";
    let in_body = Selection::caret(20);
    assert_eq!(
        layout(src, in_body).display_text(),
        "Orbit roadmap\nBody text\nNext up"
    );
    let on_first_heading = Selection::caret(9);
    assert_eq!(
        layout(src, on_first_heading).display_text(),
        "# Orbit roadmap\nBody text\nNext up"
    );
}

#[test]
fn a_hash_without_a_space_is_not_a_heading() {
    let l = layout("#planning is a tag", Selection::caret(18));
    assert_eq!(l.display_text(), "#planning is a tag");
}

const FIXTURES: &[&str] = &[
    "Orbit keeps notes as plain files",
    "Ship **first** now",
    "a *soft* b and _also_ c",
    "x ~~gone~~ y `cargo run` z",
    "x `a **b** c` y",
    "Read the [Mermaid guide](https://mermaid.js.org) first.",
    "See [[Mermaid basics]] and [[Orbit roadmap|the roadmap]].",
    "# Orbit roadmap\nBody with **bold** text\n## Next up",
    "## A heading with [[a link]] inside",
    "**adjacent**_spans_`touch`",
    "日本語の **太字** と [[リンク|別名]]",
    "unclosed **bold and [link( and [[wiki",
    "empty alias [[Orbit roadmap|]] and snake_case_name",
];

/// Every caret position survives the round trip through the display text:
/// the caret is never strictly inside, or at the start of, hidden syntax, so
/// typing, deleting and IME composition at the caret always act on text the
/// user can see.
#[test]
fn no_caret_position_ever_lands_in_hidden_text() {
    for src in FIXTURES {
        for (c, _) in src.char_indices().chain([(src.len(), ' ')]) {
            let l = layout(src, Selection::caret(c));
            assert_eq!(
                l.to_source(l.to_display(c)),
                c,
                "caret {c} in {src:?} (display {:?})",
                l.display_text()
            );
        }
    }
}

#[test]
fn unclosed_syntax_is_left_as_text() {
    let src = "unclosed **bold and [link( and [[wiki";
    assert_eq!(layout(src, Selection::caret(0)).display_text(), src);
}

#[test]
fn concealed_spans_style_their_visible_text() {
    let l = layout("Ship **first** [[now]]", Selection::caret(0));
    assert_eq!(l.display_text(), "Ship first now");
    assert_eq!(
        l.runs(),
        vec![
            Run {
                range: 5..10,
                style: Style::Bold
            },
            Run {
                range: 11..14,
                style: Style::Link
            },
        ]
    );
}

#[test]
fn revealed_syntax_is_styled_as_syntax() {
    // Caret inside the bold: markers visible and styled as syntax.
    let l = layout("Ship **first** now", Selection::caret(9));
    assert_eq!(
        l.runs(),
        vec![
            Run {
                range: 5..7,
                style: Style::Syntax
            },
            Run {
                range: 7..12,
                style: Style::Bold
            },
            Run {
                range: 12..14,
                style: Style::Syntax
            },
        ]
    );
}

#[test]
fn headings_carry_their_level() {
    let l = layout("## Next up", Selection::caret(99));
    assert_eq!(
        l.runs(),
        vec![Run {
            range: 0..7,
            style: Style::Heading(2)
        }]
    );
}

#[test]
fn a_wikilink_with_an_empty_alias_shows_its_target() {
    let l = layout("See [[Orbit roadmap|]].", Selection::caret(0));
    assert_eq!(l.display_text(), "See Orbit roadmap.");
}

#[test]
fn underscores_inside_words_are_not_italic() {
    let src = "call snake_case_name here";
    assert_eq!(layout(src, Selection::caret(0)).display_text(), src);
}
