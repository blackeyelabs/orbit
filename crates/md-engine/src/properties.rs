//! Properties: a note's front matter as keys with values, each value's
//! type guessed from how it's written, and edits that rewrite only the
//! lines of the one key they change (ADR 0009). Comments, key order,
//! quoting and keys this doesn't understand stay byte for byte. What it
//! can't edit safely (nested maps, anchors, block scalars, a key written
//! twice) reads as [`Value::Unsupported`], and edits to it are refused.

use std::ops::Range;

use crate::TextEdit;
use crate::front_matter::flow_items;

/// One key of a note's front matter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub key: String,
    pub value: Value,
    /// Where the key's lines are in the source: from its key to the end
    /// of its last line, without the line break.
    pub lines: Range<usize>,
}

/// A property's value, as it's written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A string; empty for a key with no value (or `null`).
    Text(String),
    /// A flow (`[a, b]`) or block (`- a`) list of strings.
    List(Vec<String>),
    /// A number, as written.
    Number(String),
    Checkbox(bool),
    /// `YYYY-MM-DD`, as written.
    Date(String),
    /// `YYYY-MM-DDTHH:mm`, with seconds or not, as written.
    DateTime(String),
    /// YAML the panel can't edit safely: edit it in Source mode.
    Unsupported,
}

/// What kind of value a property holds, and so how it's edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyType {
    Text,
    List,
    Number,
    Checkbox,
    Date,
    DateTime,
    /// `tags`: a list of tags.
    Tags,
    /// `aliases`: other names for the note.
    Aliases,
}

impl Property {
    /// The type its value suggests (`tags` and `aliases` by their key);
    /// `None` for a value that can't be edited here.
    pub fn guessed_type(&self) -> Option<PropertyType> {
        if self.key.eq_ignore_ascii_case("tags") {
            return (self.value != Value::Unsupported).then_some(PropertyType::Tags);
        }
        if matches!(self.key.to_ascii_lowercase().as_str(), "aliases" | "alias") {
            return (self.value != Value::Unsupported).then_some(PropertyType::Aliases);
        }
        PropertyType::of_value(&self.value)
    }
}

impl PropertyType {
    /// The type a value is written as, whatever its key.
    pub fn of_value(value: &Value) -> Option<Self> {
        Some(match value {
            Value::Text(_) => PropertyType::Text,
            Value::List(_) => PropertyType::List,
            Value::Number(_) => PropertyType::Number,
            Value::Checkbox(_) => PropertyType::Checkbox,
            Value::Date(_) => PropertyType::Date,
            Value::DateTime(_) => PropertyType::DateTime,
            Value::Unsupported => return None,
        })
    }

    /// Whether `value` is one this type holds. An empty value fits any.
    pub fn fits(self, value: &Value) -> bool {
        let tag = |t: &str| crate::tags::is_tag(t.strip_prefix('#').unwrap_or(t));
        match (self, value) {
            (_, Value::Unsupported) => false,
            (_, Value::Text(s)) if s.is_empty() => true,
            (PropertyType::Text, Value::Text(_))
            | (PropertyType::List, Value::List(_))
            | (PropertyType::Checkbox, Value::Checkbox(_))
            | (PropertyType::Aliases, Value::Text(_) | Value::List(_)) => true,
            (PropertyType::Number, Value::Number(s)) => is_number(s),
            (PropertyType::Date, Value::Date(s)) => is_date(s),
            (PropertyType::DateTime, Value::DateTime(s)) => is_date_time(s),
            (PropertyType::Tags, Value::Text(s)) => tag(s),
            (PropertyType::Tags, Value::List(items)) => items.iter().all(|t| tag(t)),
            _ => false,
        }
    }
}

/// A change to one key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyEdit {
    /// Gives `key` this value, adding it last if the note hasn't one (and
    /// front matter, if it has none).
    Set {
        key: String,
        value: Value,
    },
    Rename {
        from: String,
        to: String,
    },
    Remove {
        key: String,
    },
}

/// Why an edit was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyError {
    /// The key's YAML can't be edited safely here.
    Unsupported,
    /// No such key.
    Missing,
    /// Another key already has the name.
    Taken,
    /// A key needs a name.
    NoName,
}

/// The note's properties, in the order they're written.
pub fn properties(source: &str) -> Vec<Property> {
    let Some(front) = FrontMatter::of(source) else {
        return Vec::new();
    };
    entries(source, &front)
        .into_iter()
        .map(|e| Property {
            key: e.key,
            value: e.value,
            lines: e.span,
        })
        .collect()
}

/// The edit that makes `edit` to `source`, with `selection` kept on the
/// text it was on.
pub fn property_edit(
    source: &str,
    edit: &PropertyEdit,
    selection: Range<usize>,
) -> Result<TextEdit, PropertyError> {
    let front = FrontMatter::of(source);
    let entries = front
        .as_ref()
        .map(|f| entries(source, f))
        .unwrap_or_default();
    let find = |key: &str| entries.iter().find(|e| e.key == key);
    let editable = |key: &str| match find(key) {
        None => Err(PropertyError::Missing),
        Some(e) if e.value == Value::Unsupported => Err(PropertyError::Unsupported),
        Some(e) => Ok(e),
    };
    let newline = front
        .as_ref()
        .map_or_else(|| first_line_break(source), |f| f.newline);
    let (range, text) = match edit {
        PropertyEdit::Set { value, .. } if *value == Value::Unsupported => {
            return Err(PropertyError::Unsupported);
        }
        PropertyEdit::Set { key, .. } if key.trim().is_empty() => {
            return Err(PropertyError::NoName);
        }
        PropertyEdit::Set { key, value } => match editable(key) {
            Ok(e) => {
                let rendered = render(value, &e.style, newline);
                let comment = e.style.comment.as_deref().unwrap_or("");
                let key = &source[e.key_range.clone()];
                // A comment stays on the key's line.
                let text = if rendered.starts_with(newline) {
                    format!("{key}:{comment}{rendered}")
                } else {
                    format!("{key}:{rendered}{comment}")
                };
                (e.span.clone(), text)
            }
            Err(PropertyError::Missing) => {
                let line = format!(
                    "{}:{}",
                    key_text(key, None),
                    render(value, &Style::default(), newline)
                );
                match &front {
                    Some(f) => (f.close..f.close, format!("{line}{newline}")),
                    None => {
                        let at = crate::bom_len(source);
                        (at..at, format!("---{newline}{line}{newline}---{newline}"))
                    }
                }
            }
            Err(e) => return Err(e),
        },
        PropertyEdit::Remove { key } => {
            let e = editable(key)?;
            let end = e.span.end + line_break_at(source, e.span.end);
            (e.span.start..end, String::new())
        }
        PropertyEdit::Rename { to, .. } if to.trim().is_empty() => {
            return Err(PropertyError::NoName);
        }
        PropertyEdit::Rename { from, to } => {
            let e = editable(from)?;
            if from != to && find(to).is_some() {
                return Err(PropertyError::Taken);
            }
            let quote = source[e.key_range.clone()]
                .chars()
                .next()
                .filter(|c| matches!(c, '"' | '\''));
            (e.key_range.clone(), key_text(to, quote))
        }
    };
    let shift = |at: usize| {
        if at <= range.start {
            at
        } else if at >= range.end {
            at - range.len() + text.len()
        } else {
            range.start + text.len()
        }
    };
    let selection = shift(selection.start)..shift(selection.end);
    Ok(TextEdit {
        range,
        text,
        selection,
    })
}

/// Where a note's front matter is.
struct FrontMatter {
    /// Its lines between the `---` lines.
    body: Range<usize>,
    /// Where its closing line starts.
    close: usize,
    /// The line break it's written with.
    newline: &'static str,
}

impl FrontMatter {
    fn of(source: &str) -> Option<Self> {
        let len = crate::front_matter_len(source);
        if len == 0 {
            return None;
        }
        let start = crate::bom_len(source);
        let first_end = start + source[start..].find('\n')? + 1;
        let newline = if source[..first_end].ends_with("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let closing = &source[..len];
        let without_break = closing.strip_suffix('\n').unwrap_or(closing);
        let close = without_break
            .rfind('\n')
            .map_or(0, |n| n + 1)
            .max(first_end);
        Some(FrontMatter {
            body: first_end..close,
            close,
            newline,
        })
    }
}

/// How a value was written, to write a new one the same way.
#[derive(Default)]
struct Style {
    /// The quote it was written in.
    quote: Option<char>,
    /// A list written `[a, b]`.
    flow: bool,
    /// What a block list's items start with before their `-`.
    indent: Option<String>,
    /// A comment after the value on the key's line, with the space
    /// before it.
    comment: Option<String>,
}

/// A key as read, with what an edit needs.
struct Entry {
    key: String,
    /// The key as written, quotes and all.
    key_range: Range<usize>,
    span: Range<usize>,
    value: Value,
    style: Style,
}

/// A line of the source: where it starts, and its text without its line
/// break.
struct Line<'a> {
    start: usize,
    text: &'a str,
}

impl Line<'_> {
    fn end(&self) -> usize {
        self.start + self.text.len()
    }

    /// Blank or only a comment.
    fn is_soft(&self) -> bool {
        let t = self.text.trim_start();
        t.is_empty() || t.starts_with('#')
    }

    /// Part of the key above: indented, or a block list's item.
    fn continues(&self) -> bool {
        self.text.starts_with([' ', '\t', '-'])
    }
}

fn entries(source: &str, front: &FrontMatter) -> Vec<Entry> {
    let mut lines = Vec::new();
    let mut at = front.body.start;
    for raw in source[front.body.clone()].split_inclusive('\n') {
        let text = raw.trim_end_matches('\n').trim_end_matches('\r');
        lines.push(Line { start: at, text });
        at += raw.len();
    }
    // Each key's line with the lines that go on from it.
    let mut groups: Vec<Vec<Line>> = Vec::new();
    for line in lines {
        match groups.last_mut() {
            Some(group) if line.is_soft() || line.continues() => group.push(line),
            _ if line.is_soft() => {}
            _ => groups.push(vec![line]),
        }
    }
    let mut entries: Vec<Entry> = groups
        .into_iter()
        .map(|mut group| {
            // Blank lines and comments after a key aren't its own.
            while group.len() > 1 && group.last().is_some_and(Line::is_soft) {
                group.pop();
            }
            entry(&group)
        })
        .collect();
    // YAML forbids a key twice; which one counts is anyone's guess.
    for i in 0..entries.len() {
        if entries.iter().filter(|e| e.key == entries[i].key).count() > 1 {
            entries[i].value = Value::Unsupported;
        }
    }
    entries
}

/// A key's line and the lines that go on from it, read.
fn entry(group: &[Line]) -> Entry {
    let line = &group[0];
    let span = line.start..group.last().map_or(line.end(), Line::end);
    let Some((key, key_len, rest)) = split_key(line.text) else {
        // Not a key YAML this simple can read.
        return Entry {
            key: line.text.trim().to_string(),
            key_range: line.start..line.end(),
            span,
            value: Value::Unsupported,
            style: Style::default(),
        };
    };
    let (value, comment) = split_comment(rest);
    let value = value.trim();
    // A blank line or comment between a value's lines would be lost when
    // they're written again.
    if group[1..].iter().any(Line::is_soft) {
        return Entry {
            key,
            key_range: line.start..line.start + key_len,
            span,
            value: Value::Unsupported,
            style: Style::default(),
        };
    }
    let more: Vec<&str> = group[1..]
        .iter()
        .filter(|l| !l.is_soft())
        .map(|l| l.text)
        .collect();
    let style = Style {
        quote: value.chars().next().filter(|c| matches!(c, '"' | '\'')),
        flow: value.starts_with('['),
        indent: more.first().map(|l| {
            let indent = l.len() - l.trim_start().len();
            l[..indent].to_string()
        }),
        comment: comment.map(str::to_string),
    };
    Entry {
        key,
        key_range: line.start..line.start + key_len,
        span,
        value: read_value(value, &more),
        style,
    }
}

/// A key line's key (unquoted), how long it's written, and what follows
/// its `:`.
fn split_key(line: &str) -> Option<(String, usize, &str)> {
    let (key, len) = match line.chars().next()? {
        q @ ('"' | '\'') => quoted(line, q)?,
        c if "?&*!|>[]{}%@`,#-".contains(c) => return None,
        _ => {
            let colon = line
                .match_indices(':')
                .map(|(i, _)| i)
                .find(|&i| line[i + 1..].is_empty() || line[i + 1..].starts_with([' ', '\t']))?;
            let key = line[..colon].trim_end();
            (key.to_string(), key.len())
        }
    };
    let rest = line[len..]
        .trim_start_matches([' ', '\t'])
        .strip_prefix(':')?;
    let spaced = rest.is_empty() || rest.starts_with([' ', '\t']);
    (spaced && !key.is_empty()).then_some((key, len, rest))
}

/// A quoted scalar at the start of `text`, unquoted, and how long it's
/// written; `None` if it doesn't close or holds escapes this can't read.
fn quoted(text: &str, quote: char) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut chars = text.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            ('\'', '\'') if chars.peek().is_some_and(|&(_, n)| n == '\'') => {
                chars.next();
                out.push('\'');
            }
            ('"', '\\') => {
                let (_, escaped) = chars.next()?;
                out.push(match escaped {
                    '"' => '"',
                    '\\' => '\\',
                    'n' => '\n',
                    't' => '\t',
                    _ => return None,
                });
            }
            (q, c) if c == q => return Some((out, i + 1)),
            _ => out.push(c),
        }
    }
    None
}

/// A value and a comment after it, with the space before it.
fn split_comment(rest: &str) -> (&str, Option<&str>) {
    let mut quote = None;
    let mut prev = ' ';
    for (i, c) in rest.char_indices() {
        match (c, quote) {
            ('"' | '\'', None) if prev.is_whitespace() || matches!(prev, '[' | ',') => {
                quote = Some(c)
            }
            (c, Some(q)) if c == q => quote = None,
            ('#', None) if prev.is_whitespace() => {
                let start = rest[..i].trim_end_matches([' ', '\t']).len();
                return (&rest[..start], Some(&rest[start..]));
            }
            _ => {}
        }
        prev = c;
    }
    (rest, None)
}

/// A value written `value` on its key's line, with `more` lines after.
fn read_value(value: &str, more: &[&str]) -> Value {
    if !more.is_empty() {
        return if value.is_empty() {
            block_list(more)
        } else {
            Value::Unsupported
        };
    }
    if value.is_empty() {
        return Value::Text(String::new());
    }
    if value.starts_with(['&', '*', '!', '|', '>', '{', '?', '%', '@', '`']) {
        return Value::Unsupported;
    }
    if let Some(inner) = value.strip_prefix('[') {
        let Some(inner) = inner.strip_suffix(']') else {
            return Value::Unsupported;
        };
        if inner.trim().is_empty() {
            return Value::List(Vec::new());
        }
        let mut items: Vec<&str> = flow_items(inner).map(str::trim).collect();
        // `[a, b,]`: nothing after the last comma.
        if items.last() == Some(&"") {
            items.pop();
        }
        return items
            .into_iter()
            .map(|item| list_item(item).ok_or(()))
            .collect::<Result<_, _>>()
            .map_or(Value::Unsupported, Value::List);
    }
    if let Some(q) = value.chars().next().filter(|c| matches!(c, '"' | '\'')) {
        return match quoted(value, q) {
            Some((text, len)) if len == value.len() => guess_string(text),
            _ => Value::Unsupported,
        };
    }
    if value.contains(": ") || value.ends_with(':') {
        return Value::Unsupported;
    }
    match value {
        "true" | "True" | "TRUE" => Value::Checkbox(true),
        "false" | "False" | "FALSE" => Value::Checkbox(false),
        "null" | "Null" | "NULL" | "~" => Value::Text(String::new()),
        _ if is_number(value) => Value::Number(value.to_string()),
        _ => guess_string(value.to_string()),
    }
}

/// A block list's items, each `- item` at one indent.
fn block_list(lines: &[&str]) -> Value {
    let indent = lines[0].len() - lines[0].trim_start().len();
    let mut items = Vec::new();
    for line in lines {
        let (lead, rest) = line.split_at(line.len() - line.trim_start().len());
        if lead.len() != indent {
            return Value::Unsupported;
        }
        let item = match rest.strip_prefix('-') {
            Some(item) if item.is_empty() || item.starts_with(' ') => split_comment(item).0.trim(),
            _ => return Value::Unsupported,
        };
        if item.is_empty() {
            continue;
        }
        match list_item(item) {
            Some(item) => items.push(item),
            None => return Value::Unsupported,
        }
    }
    Value::List(items)
}

/// A list's item as a string: quoted or plain, but not a list or map.
fn list_item(item: &str) -> Option<String> {
    if let Some(q) = item.chars().next().filter(|c| matches!(c, '"' | '\'')) {
        return quoted(item, q)
            .filter(|(_, len)| *len == item.len())
            .map(|(text, _)| text);
    }
    let nested = item.starts_with(['[', '{', '&', '*', '!', '|', '>', '-', '?'])
        || item.contains(": ")
        || item.ends_with(':');
    (!nested).then(|| item.to_string())
}

/// A string's type by its shape: a date, a date-time, else text.
fn guess_string(text: String) -> Value {
    if date_shaped(&text) {
        Value::Date(text)
    } else if date_time_shaped(&text) {
        Value::DateTime(text)
    } else {
        Value::Text(text)
    }
}

/// Text YAML reads as a number (not `inf` or `nan`, which it doesn't).
fn is_number(text: &str) -> bool {
    text.parse::<f64>().is_ok() && !text.to_ascii_lowercase().contains(['i', 'n'])
}

/// `dddd-dd-dd`.
fn date_shaped(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// A date, `T`, then `dd:dd`, `dd:dd:dd` or with a fraction of a second.
fn date_time_shaped(text: &str) -> bool {
    let Some((date, time)) = text.split_once('T') else {
        return false;
    };
    let (hm, seconds) = match time.get(5..) {
        Some(rest) => (&time[..5], rest),
        None => return false,
    };
    let two_digits = |s: &str| s.len() == 2 && s.bytes().all(|c| c.is_ascii_digit());
    let seconds_ok = seconds.is_empty()
        || seconds.strip_prefix(':').is_some_and(|s| {
            let (whole, fraction) = s.split_once('.').unwrap_or((s, "0"));
            two_digits(whole)
                && !fraction.is_empty()
                && fraction.bytes().all(|c| c.is_ascii_digit())
        });
    date_shaped(date)
        && hm.as_bytes()[2] == b':'
        && two_digits(&hm[..2])
        && two_digits(&hm[3..])
        && seconds_ok
}

/// A real calendar date.
fn is_date(text: &str) -> bool {
    if !date_shaped(text) {
        return false;
    }
    let num = |r: Range<usize>| text[r].parse::<u32>().unwrap_or(0);
    let (year, month, day) = (num(0..4), num(5..7), num(8..10));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => 0,
    };
    (1..=days).contains(&day)
}

/// A real date and time of day.
fn is_date_time(text: &str) -> bool {
    let num = |r: Range<usize>| text.get(r).and_then(|s| s.parse::<u32>().ok());
    date_time_shaped(text)
        && is_date(&text[..10])
        && num(11..13).is_some_and(|h| h < 24)
        && num(14..16).is_some_and(|m| m < 60)
        && num(17..19).is_none_or(|s| s < 60)
}

/// What follows a key's `:` for `value`, written in `style`.
fn render(value: &Value, style: &Style, newline: &str) -> String {
    match value {
        Value::Text(s) if s.is_empty() => String::new(),
        Value::Text(s) => format!(" {}", scalar(s, style.quote)),
        Value::Number(s) if is_number(s) => format!(" {s}"),
        // A date keeps the quotes it was written in.
        Value::Date(s) | Value::DateTime(s) if style.quote.is_some() => {
            format!(" {}", scalar(s, style.quote))
        }
        Value::Date(s) | Value::DateTime(s) if !needs_quotes(s) => format!(" {s}"),
        Value::Number(s) | Value::Date(s) | Value::DateTime(s) => format!(" {}", scalar(s, None)),
        Value::Checkbox(b) => format!(" {b}"),
        Value::List(items) if items.is_empty() => " []".to_string(),
        Value::List(items) if style.flow => {
            let items: Vec<String> = items.iter().map(|i| flow_scalar(i)).collect();
            format!(" [{}]", items.join(", "))
        }
        Value::List(items) => {
            let indent = style.indent.as_deref().unwrap_or("  ");
            items
                .iter()
                .map(|item| format!("{newline}{indent}- {}", item_scalar(item)))
                .collect()
        }
        Value::Unsupported => String::new(),
    }
}

/// Text as a scalar that reads back as this text: in `quote` if it was
/// written in one, else quoted only if it must be.
fn scalar(text: &str, quote: Option<char>) -> String {
    let reads_otherwise = matches!(
        read_value(text, &[]),
        Value::Number(_) | Value::Checkbox(_) | Value::Unsupported
    ) || matches!(text, "null" | "Null" | "NULL" | "~");
    match quote {
        Some(q) => quote_with(text, q),
        None if needs_quotes(text) || reads_otherwise => quote_any(text),
        None => text.to_string(),
    }
}

/// A list item, quoted only if YAML wouldn't read it as this text.
fn item_scalar(text: &str) -> String {
    if needs_quotes(text) || list_item(text).as_deref() != Some(text) {
        quote_any(text)
    } else {
        text.to_string()
    }
}

/// A flow list's item, quoted too if it holds what ends one.
fn flow_scalar(text: &str) -> String {
    if text.contains([',', '[', ']', '{', '}']) {
        quote_any(text)
    } else {
        item_scalar(text)
    }
}

/// A key as written: plain, or in `quote` if it was, or in quotes it must
/// have.
fn key_text(key: &str, quote: Option<char>) -> String {
    let key = key.trim();
    match quote {
        Some(q) => quote_with(key, q),
        None if needs_quotes(key) || key.contains(':') => quote_any(key),
        None => key.to_string(),
    }
}

/// Text YAML wouldn't read plain as itself.
fn needs_quotes(text: &str) -> bool {
    text.starts_with([
        '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@',
        '`', ' ', '\t',
    ]) || text.ends_with([' ', '\t', ':'])
        || text.contains(": ")
        || text.contains(" #")
        || text.contains(['\n', '\t', '\r'])
}

/// In double quotes, unless it has some and no single ones.
fn quote_any(text: &str) -> String {
    if text.contains('"') && !text.contains(['\'', '\n', '\t', '\\']) {
        quote_with(text, '\'')
    } else {
        quote_with(text, '"')
    }
}

fn quote_with(text: &str, quote: char) -> String {
    if quote == '\'' && !text.contains(['\n', '\t']) {
        return format!("'{}'", text.replace('\'', "''"));
    }
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

/// The length of the line break at `at`, if one starts there.
fn line_break_at(source: &str, at: usize) -> usize {
    if source[at..].starts_with("\r\n") {
        2
    } else {
        usize::from(source[at..].starts_with('\n'))
    }
}

/// The line break the source's first line ends with: `\n` if it has none.
fn first_line_break(source: &str) -> &'static str {
    match source.find('\n') {
        Some(n) if source[..n].ends_with('\r') => "\r\n",
        _ => "\n",
    }
}
