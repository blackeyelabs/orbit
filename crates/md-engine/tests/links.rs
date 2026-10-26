//! Links in a note's source: wikilinks, embeds and markdown links, with
//! where each sits; front matter aliases; and finding a linked heading.
//! How Obsidian parses the fixture vault is checked in orbit-core's tests.

use md_engine::{Link, LinkStyle, aliases, heading_offset, links};

fn found(source: &str) -> Vec<(String, String, bool, &str)> {
    links(source)
        .into_iter()
        .map(|l| {
            let original = &source[l.span.clone()];
            (l.target, l.display, l.embed, original)
        })
        .collect()
}

fn link(
    target: &str,
    display: &str,
    embed: bool,
    original: &'static str,
) -> (String, String, bool, &'static str) {
    (target.into(), display.into(), embed, original)
}

#[test]
fn wikilinks_with_aliases_headings_and_embeds() {
    let source = "See [[Plan]], [[Plan|the plan]], [[Plan#Goals]] and ![[diagram.png]].";
    assert_eq!(
        found(source),
        [
            link("Plan", "Plan", false, "[[Plan]]"),
            link("Plan", "the plan", false, "[[Plan|the plan]]"),
            link("Plan#Goals", "Plan > Goals", false, "[[Plan#Goals]]"),
            link("diagram.png", "diagram.png", true, "![[diagram.png]]"),
        ]
    );
    let goals = &links(source)[2];
    assert_eq!((goals.path(), goals.subpath()), ("Plan", Some("Goals")));
    assert_eq!(goals.style, LinkStyle::Wiki);
    assert_eq!(links("[[#Risks]]")[0].display, "Risks");
}

#[test]
fn markdown_links_are_decoded_and_web_addresses_left_out() {
    let source = "[a](Projects/Plan.md) [b](<Inbox/My note.md>) [c](My%20note.md#Top) \
                  ![pic](assets/pic.png) [web](https://example.com) [mail](mailto:a@b.c)";
    assert_eq!(
        found(source),
        [
            link("Projects/Plan.md", "a", false, "[a](Projects/Plan.md)"),
            link("Inbox/My note.md", "b", false, "[b](<Inbox/My note.md>)"),
            link("My note.md#Top", "c", false, "[c](My%20note.md#Top)"),
            link("assets/pic.png", "pic", true, "![pic](assets/pic.png)"),
        ]
    );
    assert!(links(source).iter().all(|l| l.style == LinkStyle::Markdown));
}

#[test]
fn code_front_matter_and_escapes_hold_no_links() {
    let source = "---\nup: \"[[Front]]\"\n---\n\
                  `[[inline code]]` and ``[[double `tick`]]``\n\
                  ```\n[[fenced]]\n```\n\
                  ~~~md\n[x](fenced.md)\n~~~\n\
                  \\[[escaped]] and [[real]]\n";
    let targets: Vec<String> = links(source).into_iter().map(|l| l.target).collect();
    assert_eq!(targets, ["real"]);
}

#[test]
fn broken_syntax_is_not_a_link() {
    for source in [
        "[[]]",
        "[[open",
        "[[a\nb]]",
        "[text](",
        "[text] (x.md)",
        "plain",
    ] {
        assert_eq!(links(source), Vec::<Link>::new(), "{source:?}");
    }
}

#[test]
fn a_pipe_escaped_in_a_table_still_starts_the_alias() {
    let link = &links("| [[Plan\\|shown]] |")[0];
    assert_eq!(
        (link.target.as_str(), link.display.as_str()),
        ("Plan", "shown")
    );
}

#[test]
fn spans_are_byte_offsets_in_the_source() {
    let source = "Café → [[Ünïcode]]";
    let link = &links(source)[0];
    assert_eq!(&source[link.span.clone()], "[[Ünïcode]]");
}

#[test]
fn aliases_come_from_front_matter_in_any_list_form() {
    assert_eq!(
        aliases("---\naliases: [Roadmap, \"The Plan\"]\n---\n"),
        ["Roadmap", "The Plan"]
    );
    assert_eq!(
        aliases("---\naliases:\n  - Old plan\n  - 'Older'\n---\n# x"),
        ["Old plan", "Older"]
    );
    assert_eq!(aliases("---\nalias: Single\ntags: [a]\n---\n"), ["Single"]);
    assert_eq!(aliases("---\ntags: [a]\n---\n"), Vec::<String>::new());
    assert_eq!(aliases("aliases: [Not front matter]"), Vec::<String>::new());
}

#[test]
fn a_linked_heading_is_found_ignoring_case_and_nesting() {
    let source = "---\ntitle: x\n---\n# Plan\n\n## Goals\n\nShip.\n\n### Risks & costs\n";
    assert_eq!(heading_offset(source, "Goals"), source.find("## Goals"));
    assert_eq!(heading_offset(source, "goals"), source.find("## Goals"));
    assert_eq!(
        heading_offset(source, "Plan#Risks & costs"),
        source.find("### Risks")
    );
    assert_eq!(heading_offset(source, "Missing"), None);
    assert_eq!(
        heading_offset("```\n# not a heading\n```\n", "not a heading"),
        None
    );
}
