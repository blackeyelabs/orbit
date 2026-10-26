//! Ctrl+B, Ctrl+I and Ctrl+K in the source editor.
//!
//! Fixtures mark the selection with ‹ and ›, or the caret with a lone ‸.

use std::ops::Range;

use md_engine::{Format, TextEdit, format_edit};

/// Splits a fixture into its text and selection.
fn parse(fixture: &str) -> (String, Range<usize>) {
    if let Some(at) = fixture.find('‸') {
        return (fixture.replace('‸', ""), at..at);
    }
    let start = fixture.find('‹').expect("no selection in fixture");
    let text = fixture.replace('‹', "");
    let end = text.find('›').expect("no selection end in fixture");
    (text.replace('›', ""), start..end)
}

/// Renders text and a selection in fixture notation.
fn render(text: &str, selection: Range<usize>) -> String {
    if selection.is_empty() {
        return format!("{}‸{}", &text[..selection.start], &text[selection.start..]);
    }
    format!(
        "{}‹{}›{}",
        &text[..selection.start],
        &text[selection.clone()],
        &text[selection.end..]
    )
}

fn apply(kind: Format, fixture: &str) -> String {
    let (mut text, selection) = parse(fixture);
    let TextEdit {
        range,
        text: replacement,
        selection,
    } = format_edit(&text, selection, kind);
    text.replace_range(range, &replacement);
    render(&text, selection)
}

#[test]
fn bold_wraps_the_selection_and_keeps_it_selected() {
    assert_eq!(apply(Format::Bold, "ship ‹it› now"), "ship **‹it›** now");
}

#[test]
fn bold_again_unwraps_whether_or_not_the_markers_are_selected() {
    assert_eq!(apply(Format::Bold, "ship **‹it›** now"), "ship ‹it› now");
    assert_eq!(apply(Format::Bold, "ship ‹**it**› now"), "ship ‹it› now");
}

#[test]
fn italic_toggles_one_asterisk_and_leaves_bold_alone() {
    assert_eq!(apply(Format::Italic, "a ‹word›"), "a *‹word›*");
    assert_eq!(apply(Format::Italic, "a *‹word›*"), "a ‹word›");
    // Bold text gains italic instead of losing one of its asterisks.
    assert_eq!(apply(Format::Italic, "a **‹word›**"), "a ***‹word›***");
    assert_eq!(apply(Format::Italic, "a ***‹word›***"), "a **‹word›**");
    assert_eq!(apply(Format::Bold, "a ***‹word›***"), "a *‹word›*");
}

#[test]
fn with_only_a_caret_the_markers_go_around_it_and_a_second_press_removes_them() {
    assert_eq!(apply(Format::Bold, "say ‸"), "say **‸**");
    assert_eq!(apply(Format::Bold, "say **‸**"), "say ‸");
    assert_eq!(apply(Format::Italic, "say ‸"), "say *‸*");
}

#[test]
fn spaces_at_the_ends_of_the_selection_stay_outside_the_markers() {
    assert_eq!(apply(Format::Bold, "ship‹ it ›now"), "ship **‹it›** now");
}

#[test]
fn link_uses_selected_text_as_the_label_and_a_selected_url_as_the_target() {
    assert_eq!(apply(Format::Link, "read ‹the docs›"), "read [the docs](‸)");
    assert_eq!(
        apply(Format::Link, "see ‹https://orbit.example/docs›"),
        "see [‸](https://orbit.example/docs)"
    );
    assert_eq!(apply(Format::Link, "see ‸"), "see [‸]()");
}

#[test]
fn two_separate_emphases_are_not_mistaken_for_one_wrapping_pair() {
    assert_eq!(apply(Format::Italic, "‹*a* and *b*›"), "*‹*a* and *b*›*");
}
