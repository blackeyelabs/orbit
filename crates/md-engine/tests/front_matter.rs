//! Lists read from front matter. How Obsidian reads tags is checked in
//! orbit-core's tests; these are the YAML corners its fixture doesn't hit.

use md_engine::{aliases, front_matter_tags};

#[test]
fn an_apostrophe_inside_an_item_does_not_start_a_quote() {
    assert_eq!(
        aliases("---\naliases: [don't, \"a, b\", x]\n---\n"),
        ["don't", "a, b", "x"]
    );
}

#[test]
fn tags_that_yaml_reads_as_numbers_booleans_or_null_are_left_out() {
    assert_eq!(
        front_matter_tags("---\ntags : [nan, inf, 1.5, 1e3, .inf, null, True, 'true', x]\n---\n"),
        ["nan", "inf", "true", "x"]
    );
}
