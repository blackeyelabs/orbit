//! Every inline construct through the Document facade: what a paragraph
//! shows with the caret away, what each span reveals when the caret
//! touches it, and the links a paragraph holds. Expected values are
//! worked out by hand from Obsidian's Live Preview.

use live_preview::Document;
use live_preview::conceal::Target;

/// What a one-paragraph note shows with the caret at `caret`.
fn shown_at(text: &str, caret: usize) -> String {
    let mut d = Document::new(text);
    d.set_caret(caret);
    d.views()[0].display_text().to_string()
}

/// What a one-paragraph note shows with the caret away from it, at the
/// note's end (past a blank line).
fn shown(text: &str) -> String {
    let note = format!("{text}\n\n");
    shown_at(&note, note.len())
}

#[test]
fn nested_emphasis_conceals_each_level_on_its_own() {
    let src = "a **bold *it*** b";
    assert_eq!(shown(src), "a bold it b");
    // On the bold's opening edge: only the bold's markers show.
    assert_eq!(shown_at(src, 2), "a **bold it** b");
    // Inside the italic: it and the bold around it show.
    assert_eq!(shown_at(src, 11), src);
    assert_eq!(shown("***both***"), "both");
    assert_eq!(shown("*a **b** c*"), "a b c");
}

#[test]
fn strike_highlight_and_double_underscores_conceal() {
    assert_eq!(shown("~~old~~ ==key== __big__"), "old key big");
    let src = "a ==key== b";
    assert_eq!(shown_at(src, 5), src);
}

#[test]
fn nothing_inside_code_is_concealed() {
    assert_eq!(
        shown("x `**not bold** [[nor]]` y"),
        "x **not bold** [[nor]] y"
    );
    // Longer fences hold shorter ones.
    assert_eq!(shown("``a ` b``"), "a ` b");
    // Code inside emphasis still keeps its own text.
    assert_eq!(shown("**see `*this*`**"), "see *this*");
}

#[test]
fn an_escaped_marker_shows_as_typed_and_its_backslash_is_hidden() {
    let src = r"\*not italic\*";
    assert_eq!(shown(src), "*not italic*");
    // Touching the first escape reveals its backslash only.
    assert_eq!(shown_at(src, 0), r"\*not italic*");
    assert_eq!(shown(r"a\_b c\\d"), r"a_b c\d");
}

#[test]
fn wikilinks_show_their_alias_or_their_target_with_headings() {
    assert_eq!(shown("[[Note|the alias]]"), "the alias");
    assert_eq!(shown("[[Note#Part]]"), "Note > Part");
    assert_eq!(shown("[[Note#Part#Sub]]"), "Note > Part > Sub");
    assert_eq!(shown("[[#Part]]"), "Part");
    assert_eq!(shown("[[Note#Part|alias]]"), "alias");
    let src = "see [[Note#Part]] now";
    assert_eq!(shown_at(src, 10), src);
}

#[test]
fn markdown_links_show_their_text_with_its_own_syntax_concealed() {
    assert_eq!(shown("[**big** news](https://a.b/c)"), "big news");
    assert_eq!(shown("[x](<Other note.md> \"title\")"), "x");
    // Images are objects in the text (see tests/images.rs).
    assert_eq!(
        shown("![[pic.png]] and ![alt](a.png)"),
        "\u{FFFC} and \u{FFFC}"
    );
}

#[test]
fn web_addresses_show_as_typed_and_autolinks_lose_their_brackets() {
    assert_eq!(shown("see https://a.b/c."), "see https://a.b/c.");
    assert_eq!(shown("<https://a.b>"), "https://a.b");
}

#[test]
fn tags_show_as_typed() {
    assert_eq!(
        shown("#tag and #snake_case_tag"),
        "#tag and #snake_case_tag"
    );
}

/// The links a one-paragraph note shows with the caret at `caret`: their
/// display text, where they go, and whether a click follows them.
fn links_at(text: &str, caret: usize) -> Vec<(String, Target, bool)> {
    let mut d = Document::new(text);
    d.set_caret(caret);
    let view = &d.views()[0];
    view.links()
        .iter()
        .map(|l| {
            let text = view.display_text()[l.range.clone()].to_string();
            (text, l.target.clone(), l.followable)
        })
        .collect()
}

#[test]
fn a_paragraph_knows_its_links_and_which_a_click_follows() {
    let src =
        "[[Note#Part|go]], [doc](Other%20note.md#Top), [web](https://a.b) and https://c.d\n\n";
    let note = |t: &str| Target::Note(t.to_string());
    let url = |t: &str| Target::Url(t.to_string());
    assert_eq!(
        links_at(src, src.len()),
        [
            ("go".into(), note("Note#Part"), true),
            ("doc".into(), note("Other note.md#Top"), true),
            ("web".into(), url("https://a.b"), true),
            ("https://c.d".into(), url("https://c.d"), true),
        ]
    );
    // With the caret in it, a link's syntax shows and a click edits it; a
    // bare address has none to show and is still followed.
    let links = links_at(src, 3);
    assert_eq!(links[0], ("go".into(), note("Note#Part"), false));
    assert!(links[1].2);
    let links = links_at("see https://c.d", 6);
    assert!(links[0].2);
}

#[test]
fn a_paragraph_of_unclosed_markers_lays_out_quickly() {
    // Openers only, each after a space, so none can close: a search for
    // a close doesn't parse what it passes over again and again, nor
    // nest as deep as the openers run.
    let src = "**a ~~b ==c [d *e _f ".repeat(200);
    let started = std::time::Instant::now();
    assert!(shown(&src) == src, "nothing closes");
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn each_construct_shows_its_syntax_while_the_caret_touches_it() {
    for (src, caret) in [
        ("a ~~old~~ b", 4),
        ("a __big__ b", 4),
        ("a `code` b", 4),
        ("a [text](u.md) b", 4),
        ("a <https://a.b> b", 4),
        ("a [[N#H|al]] b", 4),
    ] {
        assert_eq!(shown_at(src, caret), src);
        assert_ne!(shown(src), src, "{src:?} conceals with the caret away");
    }
}

#[test]
fn a_web_address_ends_before_closing_markers_and_unbalanced_brackets() {
    assert_eq!(shown("**https://a.b** x"), "https://a.b x");
    assert_eq!(shown("*see https://a.b* x"), "see https://a.b x");
    let src = "(see https://w.org/F_(b))";
    assert_eq!(
        links_at(src, 0)[0].1,
        Target::Url("https://w.org/F_(b)".into())
    );
}

#[test]
fn malformed_syntax_stays_as_typed() {
    // A run of backticks nothing closes is text, all of it.
    assert_eq!(shown("x ``a` y"), "x ``a` y");
    // A destination with a space in it, not a title, isn't a link.
    assert_eq!(shown("[a](b c)"), "[a](b c)");
    // A heading link with no heading has no separator.
    assert_eq!(shown("[[N#]]"), "N");
}

#[test]
fn emphasis_never_overlaps() {
    let mut d = Document::new("**a *b** c*\n\n");
    d.set_caret(13);
    let view = &d.views()[0];
    let runs = view.runs();
    for (i, a) in runs.iter().enumerate() {
        for b in &runs[i + 1..] {
            let nested = (a.range.start <= b.range.start && b.range.end <= a.range.end)
                || (b.range.start <= a.range.start && a.range.end <= b.range.end);
            let apart = a.range.end <= b.range.start || b.range.end <= a.range.start;
            assert!(
                nested || apart,
                "{a:?} and {b:?} in {:?}",
                view.display_text()
            );
        }
    }
}
