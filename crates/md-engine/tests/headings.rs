//! A note's headings, for its Outline.

use md_engine::{Heading, headings};

/// Each heading's level, text and the source line it starts.
fn found(source: &str) -> Vec<(u8, String, &str)> {
    headings(source)
        .into_iter()
        .map(|h| {
            let line = source[h.offset..].lines().next().unwrap_or("");
            (h.level, h.text, line)
        })
        .collect()
}

#[test]
fn headings_by_level_with_where_each_starts() {
    let source = "---\ntitle: x\n---\n# Plan\n\nIntro.\n\n## Goals ##\n\n   ### Risks & costs\n#### Café 日本\n";
    assert_eq!(
        found(source),
        [
            (1, "Plan".into(), "# Plan"),
            (2, "Goals".into(), "## Goals ##"),
            (3, "Risks & costs".into(), "   ### Risks & costs"),
            (4, "Café 日本".into(), "#### Café 日本"),
        ]
    );
}

#[test]
fn only_real_headings_with_text_count() {
    let source = "#tag is not a heading\n    # indented code\n```\n# fenced\n```\n####### seven\n#\n## \\# escaped hash\n";
    let all: Vec<Heading> = headings(source);
    let texts: Vec<&str> = all.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(texts, ["\\# escaped hash"]);
}

#[test]
fn underlined_headings_count_but_not_rules_or_list_items() {
    let source = "Title\n=====\n\nSection\n---\n\n---\n\n- item\n---\n\nPara\ngraph\n--\n";
    assert_eq!(
        found(source),
        [
            (1, "Title".into(), "Title"),
            (2, "Section".into(), "Section"),
            (2, "Para graph".into(), "Para"),
        ]
    );
}
