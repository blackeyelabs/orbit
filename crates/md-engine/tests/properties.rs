//! Properties: a note's front matter read as keys with values, each type
//! guessed from its value, and edits that change only one key's lines
//! (ADR 0009). Expected values are worked out by hand.

use md_engine::{Property, PropertyEdit, PropertyError, PropertyType, Value, properties};

/// Each property's key and value.
fn read(source: &str) -> Vec<(String, Value)> {
    properties(source)
        .into_iter()
        .map(|p| (p.key, p.value))
        .collect()
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn list(items: &[&str]) -> Value {
    Value::List(items.iter().map(|s| s.to_string()).collect())
}

#[test]
fn each_value_is_read_with_its_type_guessed() {
    let source = "---\n\
        title: A plain title\n\
        count: 12\n\
        ratio: -1.5\n\
        done: true\n\
        due: 2026-11-08\n\
        created: 2026-11-05T14:32\n\
        tags: [a, \"b c\"]\n\
        aliases:\n  - One\n  - 'Two'\n\
        empty:\n\
        quoted: \"5\"\n\
        ---\nBody";
    assert_eq!(
        read(source),
        [
            ("title".into(), text("A plain title")),
            ("count".into(), Value::Number("12".into())),
            ("ratio".into(), Value::Number("-1.5".into())),
            ("done".into(), Value::Checkbox(true)),
            ("due".into(), Value::Date("2026-11-08".into())),
            ("created".into(), Value::DateTime("2026-11-05T14:32".into())),
            ("tags".into(), list(&["a", "b c"])),
            ("aliases".into(), list(&["One", "Two"])),
            ("empty".into(), text("")),
            ("quoted".into(), text("5")),
        ]
    );
    let types: Vec<_> = properties(source)
        .iter()
        .map(Property::guessed_type)
        .collect();
    use PropertyType::*;
    assert_eq!(
        types,
        [
            Some(Text),
            Some(Number),
            Some(Number),
            Some(Checkbox),
            Some(Date),
            Some(DateTime),
            Some(Tags),
            Some(Aliases),
            Some(Text),
            Some(Text),
        ]
    );
}

#[test]
fn comments_and_trailing_comments_are_not_values() {
    let source = "---\n# about this note\nstatus: draft # for now\nurl: http://a.b/#top\n---\n";
    assert_eq!(
        read(source),
        [
            ("status".into(), text("draft")),
            ("url".into(), text("http://a.b/#top")),
        ]
    );
}

#[test]
fn a_note_without_front_matter_has_no_properties() {
    assert!(properties("Just text\n---\nmore").is_empty());
    assert!(properties("---\n---\nBody").is_empty());
}

#[test]
fn yaml_the_panel_cannot_edit_is_unsupported() {
    let source = "---\n\
        nested:\n  a: 1\n  b: 2\n\
        anchor: &x hello\n\
        alias: *x\n\
        block: |\n  line one\n  line two\n\
        map: {a: 1}\n\
        deep: [[1, 2]]\n\
        open: [a,\n  b]\n\
        tagged: !!str 5\n\
        fine: yes please\n\
        ---\n";
    let keys: Vec<(String, bool)> = properties(source)
        .into_iter()
        .map(|p| (p.key, p.value == Value::Unsupported))
        .collect();
    assert_eq!(
        keys,
        [
            ("nested".into(), true),
            ("anchor".into(), true),
            ("alias".into(), true),
            ("block".into(), true),
            ("map".into(), true),
            ("deep".into(), true),
            ("open".into(), true),
            ("tagged".into(), true),
            ("fine".into(), false),
        ]
    );
    assert_eq!(properties(source)[0].guessed_type(), None);
}

#[test]
fn a_key_written_twice_is_unsupported_both_times() {
    let values: Vec<Value> = properties("---\na: 1\na: 2\n---\n")
        .into_iter()
        .map(|p| p.value)
        .collect();
    assert_eq!(values, [Value::Unsupported, Value::Unsupported]);
}

#[test]
fn a_value_that_does_not_fit_its_type_is_flagged() {
    let props = properties("---\ntags: 5\naliases: [a]\nn: 3\n---\n");
    let fits: Vec<bool> = props
        .iter()
        .map(|p| p.guessed_type().unwrap().fits(&p.value))
        .collect();
    assert_eq!(fits, [false, true, true]);
    assert!(!PropertyType::Number.fits(&text("abc")));
    assert!(!PropertyType::Date.fits(&Value::Date("2026-13-40".into())));
    assert!(PropertyType::Tags.fits(&text("single")));
    assert!(!PropertyType::Tags.fits(&list(&["ok", "not ok"])));
}

/// `source` with `edit` made, the caret at `caret` kept where it was in
/// the text around the edit.
fn edited(source: &str, edit: PropertyEdit, caret: usize) -> (String, usize) {
    let edit = md_engine::property_edit(source, &edit, caret..caret).expect("an edit");
    let mut out = source.to_string();
    out.replace_range(edit.range, &edit.text);
    assert!(edit.selection.is_empty());
    (out, edit.selection.start)
}

fn set(key: &str, value: Value) -> PropertyEdit {
    PropertyEdit::Set {
        key: key.into(),
        value,
    }
}

/// A front matter with comments, odd quoting and an order that isn't
/// sorted, which an edit to one key must leave as it is.
const ODD: &str = "\u{feff}---\r\n\
    # keep this comment\r\n\
    zeta: 'single'   # trailing\r\n\
    tags: [b,  a]\r\n\
    \"quoted key\": x\r\n\
    list:\r\n\
    - one\r\n\
    - two\r\n\
    alpha: 1\r\n\
    ---\r\n\
    Body\r\n";

#[test]
fn setting_a_value_changes_only_its_lines() {
    let body = ODD.find("Body").unwrap();
    let (out, caret) = edited(ODD, set("alpha", Value::Number("2".into())), body);
    assert_eq!(out, ODD.replace("alpha: 1", "alpha: 2"));
    assert_eq!(caret, body, "the caret stays on its text");
    let (out, _) = edited(ODD, set("zeta", text("it's new")), 0);
    assert_eq!(
        out,
        ODD.replace(
            "zeta: 'single'   # trailing",
            "zeta: 'it''s new'   # trailing"
        ),
        "its quoting and comment kept"
    );
    let (out, _) = edited(ODD, set("tags", list(&["b", "a", "c"])), 0);
    assert_eq!(
        out,
        ODD.replace("[b,  a]", "[b, a, c]"),
        "still a flow list"
    );
    let (out, _) = edited(ODD, set("list", list(&["one"])), 0);
    assert_eq!(out, ODD.replace("- one\r\n- two\r\n", "- one\r\n"));
    let (out, _) = edited(ODD, set("quoted key", text("y")), 0);
    assert_eq!(out, ODD.replace("\"quoted key\": x", "\"quoted key\": y"));
}

#[test]
fn text_that_would_read_as_another_type_is_quoted() {
    let source = "---\na: x\n---\n";
    for (value, written) in [
        (text("5"), "a: \"5\""),
        (text("true"), "a: \"true\""),
        (text("key: value"), "a: \"key: value\""),
        (text("\"hi\" she said"), "a: '\"hi\" she said'"),
        (text("#hash"), "a: \"#hash\""),
        (text(""), "a:"),
        (Value::Number("7".into()), "a: 7"),
        (Value::Checkbox(false), "a: false"),
        (
            Value::DateTime("2026-11-08T09:30".into()),
            "a: 2026-11-08T09:30",
        ),
        (list(&[]), "a: []"),
        (list(&["x", "y: z"]), "a:\n  - x\n  - \"y: z\""),
    ] {
        let (out, _) = edited(source, set("a", value.clone()), 0);
        assert_eq!(out, format!("---\n{written}\n---\n"), "{value:?}");
        assert_eq!(read(&out)[0].1, value, "reads back");
    }
}

#[test]
fn a_new_key_goes_last_and_a_note_without_front_matter_gets_some() {
    let (out, caret) = edited("---\na: 1\n---\nBody", set("b", text("two")), 13);
    assert_eq!(out, "---\na: 1\nb: two\n---\nBody");
    assert_eq!(caret, 20);
    let (out, caret) = edited("Body", set("status", text("draft")), 2);
    assert_eq!(out, "---\nstatus: draft\n---\nBody");
    assert_eq!(caret, 24);
    let (out, _) = edited("\u{feff}Body", set("a", text("b")), 0);
    assert_eq!(out, "\u{feff}---\na: b\n---\nBody", "after the BOM");
}

#[test]
fn removing_and_renaming_a_key() {
    let (out, _) = edited(ODD, PropertyEdit::Remove { key: "list".into() }, 0);
    assert_eq!(out, ODD.replace("list:\r\n- one\r\n- two\r\n", ""));
    let rename = PropertyEdit::Rename {
        from: "zeta".into(),
        to: "omega".into(),
    };
    let (out, _) = edited(ODD, rename, 0);
    assert_eq!(out, ODD.replace("zeta:", "omega:"));
}

#[test]
fn edits_that_are_not_safe_are_refused() {
    let source = "---\nnested:\n  a: 1\nb: 2\n---\n";
    let refused = |edit: PropertyEdit| md_engine::property_edit(source, &edit, 0..0).err();
    assert_eq!(
        refused(set("nested", text("x"))),
        Some(PropertyError::Unsupported)
    );
    assert_eq!(
        refused(PropertyEdit::Remove {
            key: "nested".into()
        }),
        Some(PropertyError::Unsupported)
    );
    let rename = |to: &str| PropertyEdit::Rename {
        from: "b".into(),
        to: to.into(),
    };
    assert_eq!(refused(rename("nested")), Some(PropertyError::Taken));
    assert_eq!(refused(rename("  ")), Some(PropertyError::NoName));
    assert_eq!(
        refused(PropertyEdit::Remove { key: "c".into() }),
        Some(PropertyError::Missing)
    );
}

#[test]
fn new_front_matter_uses_the_notes_line_breaks() {
    let (out, _) = edited("Body\r\nmore\r\n", set("a", text("b")), 0);
    assert_eq!(out, "---\r\na: b\r\n---\r\nBody\r\nmore\r\n");
}

#[test]
fn a_quoted_date_stays_quoted() {
    let (out, _) = edited(
        "---\ndue: '2026-01-01'\n---\n",
        set("due", Value::Date("2026-02-02".into())),
        0,
    );
    assert_eq!(out, "---\ndue: '2026-02-02'\n---\n");
}

#[test]
fn a_list_with_comments_between_its_items_is_unsupported() {
    // An edit would rewrite its lines and lose the comment.
    let props = properties("---\nlist:\n  - a\n  # why b\n  - b\nnext: 1\n---\n");
    assert_eq!(props[0].value, Value::Unsupported);
    assert_eq!(props[1].value, Value::Number("1".into()));
}
