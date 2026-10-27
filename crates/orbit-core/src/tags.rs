//! Tags across the vault, counted as Obsidian counts them: each time one is
//! written, under its first spelling (matching ignores case), and once more
//! for every tag it's nested in (`#project/orbit` counts for `#project`).

use std::collections::HashMap;

/// A tag in the vault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagCount {
    /// Without `#`, spelled as first seen.
    pub name: String,
    /// How many times it's written, nested tags included.
    pub count: usize,
}

/// The tags written in each note (in path order), counted, by name
/// ignoring case.
pub(crate) fn count<'a>(notes: impl Iterator<Item = &'a [String]>) -> Vec<TagCount> {
    let mut counts: HashMap<String, TagCount> = HashMap::new();
    for tag in notes.flatten() {
        for name in with_parents(tag) {
            counts
                .entry(name.to_lowercase())
                .or_insert_with(|| TagCount {
                    name: name.to_string(),
                    count: 0,
                })
                .count += 1;
        }
    }
    let mut tags: Vec<TagCount> = counts.into_values().collect();
    tags.sort_by_cached_key(|tag| (tag.name.to_lowercase(), tag.name.clone()));
    tags
}

/// Whether two names (with or without `#`) are the same tag: ignoring case.
pub fn same_tag(a: &str, b: &str) -> bool {
    normal(a) == normal(b)
}

/// A tag name for matching: without `#` or a trailing `/`, lowercased.
fn normal(tag: &str) -> String {
    tag.trim_start_matches('#')
        .trim_end_matches('/')
        .to_lowercase()
}

/// Whether `tags` holds `wanted` (with or without `#`), or a tag nested in
/// it, ignoring case.
pub(crate) fn has(tags: &[String], wanted: &str) -> bool {
    let wanted = normal(wanted);
    !wanted.is_empty()
        && tags.iter().any(|tag| {
            let tag = normal(tag);
            tag == wanted
                || tag
                    .strip_prefix(&wanted)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

/// `a/b/c` → `a`, `a/b`, `a/b/c`. Empty parts name nothing (`a//b` is
/// `a` and `a//b`, `a/` just `a`), and nor do digits alone (`2024/01` is
/// just itself).
fn with_parents(tag: &str) -> impl Iterator<Item = &str> {
    tag.match_indices('/')
        .map(|(i, _)| &tag[..i])
        .chain(std::iter::once(tag))
        .filter(|name| !name.ends_with('/') && !name.chars().all(|c| c.is_ascii_digit()))
}
