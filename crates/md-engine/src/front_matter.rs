//! Lists in a note's front matter: aliases and tags. Front matter is only
//! read, never written back.

use crate::front_matter_len;

/// A value in a front matter list.
struct Value {
    text: String,
    /// Written in quotes, so text whatever it looks like.
    quoted: bool,
}

impl Value {
    /// Text YAML reads as a string, not a number, boolean or null.
    fn is_string(&self) -> bool {
        let text = self.text.to_ascii_lowercase();
        let special = [
            "true", "false", "null", "~", ".inf", "-.inf", "+.inf", ".nan",
        ];
        // Rust also parses `inf` and `nan`, which YAML reads as strings.
        let number = text.parse::<f64>().is_ok() && !text.contains(['i', 'n']);
        self.quoted || !(special.contains(&text.as_str()) || number)
    }
}

/// The front matter's `aliases` (or `alias`): a flow list, a block list or
/// one value, each unquoted.
pub fn aliases(source: &str) -> Vec<String> {
    list(source, |key| matches!(key, "aliases" | "alias"))
        .into_iter()
        .map(|value| value.text)
        .collect()
}

/// The front matter's `tags` (the key in any case), without any leading
/// `#`, as Obsidian takes them: string values that are whole tags.
pub fn front_matter_tags(source: &str) -> Vec<String> {
    list(source, |key| key.eq_ignore_ascii_case("tags"))
        .into_iter()
        .filter(Value::is_string)
        .map(|value| {
            value
                .text
                .strip_prefix('#')
                .unwrap_or(&value.text)
                .to_string()
        })
        .filter(|tag| crate::tags::is_tag(tag))
        .collect()
}

/// The non-empty values of the first key `is_key` accepts.
fn list(source: &str, is_key: impl Fn(&str) -> bool) -> Vec<Value> {
    let len = front_matter_len(source);
    if len == 0 {
        return Vec::new();
    }
    let mut lines = source[..len].lines().skip(1);
    while let Some(line) = lines.next() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if !is_key(key.trim_end()) {
            continue;
        }
        let value = value.trim();
        if let Some(list) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            return flow_items(list).filter_map(unquote).collect();
        }
        if !value.is_empty() {
            return unquote(value).into_iter().collect();
        }
        return lines
            .map_while(|line| line.trim_start().strip_prefix("- "))
            .filter_map(unquote)
            .collect();
    }
    Vec::new()
}

/// A flow list's items: split at commas outside quotes.
fn flow_items(list: &str) -> impl Iterator<Item = &str> {
    let mut quote = None;
    let mut start = 0;
    let mut items = Vec::new();
    for (i, c) in list.char_indices() {
        // Only an item's first character opens a quote: `don't` isn't one.
        let item_start = list[start..i].trim().is_empty();
        match (c, quote) {
            ('"' | '\'', None) if item_start => quote = Some(c),
            (c, Some(q)) if c == q => quote = None,
            (',', None) => {
                items.push(&list[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    items.push(&list[start..]);
    items.into_iter()
}

fn unquote(value: &str) -> Option<Value> {
    let value = value.trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|&q| value.strip_prefix(q).and_then(|v| v.strip_suffix(q)));
    let text = unquoted.unwrap_or(value);
    (!text.is_empty()).then(|| Value {
        text: text.to_string(),
        quoted: unquoted.is_some(),
    })
}
