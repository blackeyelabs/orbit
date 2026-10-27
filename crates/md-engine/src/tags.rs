//! Inline `#tags` in a note's source, found as Obsidian finds them
//! (checked against Obsidian 1.14.4 in orbit-core's tests): a `#` at the
//! start of a line, after whitespace or straight after another tag, then
//! the tag's characters. Front matter, code, math, comments, wikilinks and
//! escaped `\#` hold no tags.

use crate::front_matter_len;
use crate::links::{Fences, skip_code_span, wikilink};

/// Whether `c` can be part of a tag: anything but whitespace and
/// punctuation, though `_`, `-` and `/` are allowed.
fn is_tag_char(c: char) -> bool {
    !(c.is_whitespace()
        || ('\u{2000}'..='\u{206F}').contains(&c)
        || ('\u{2E00}'..='\u{2E7F}').contains(&c)
        || "'!\"#$%&()*+,.:;<=>?@^`{|}~[]\\".contains(c))
}

/// Whether `name` (without `#`) is a whole tag: tag characters, not all
/// of them digits.
pub(crate) fn is_tag(name: &str) -> bool {
    name.chars().all(is_tag_char) && !name.chars().all(|c| c.is_ascii_digit())
}

/// Every inline tag in `source`, without its `#`, as written, in order.
pub fn inline_tags(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let bytes = source.as_bytes();
    let mut i = front_matter_len(source);
    let mut lines = Lines::default();
    let mut line_start = true;
    // Where the last tag ended: another can start right there.
    let mut tag_end = None;
    while i < bytes.len() {
        if line_start {
            line_start = false;
            let line_end = source[i..].find('\n').map_or(source.len(), |n| i + n + 1);
            if lines.is_code(&source[i..line_end]) {
                i = line_end;
                line_start = true;
                continue;
            }
        }
        match bytes[i] {
            b'\n' => {
                line_start = true;
                i += 1;
            }
            // An escaped character, but never past a line end.
            b'\\' if bytes.get(i + 1) != Some(&b'\n') => {
                i += 1 + source[i + 1..].chars().next().map_or(0, char::len_utf8)
            }
            b'`' => i = skip_code_span(source, i),
            b'$' => i = skip_math(source, i),
            b'<' if source[i..].starts_with("<!--") => i = skip_past(source, i + 4, "-->"),
            b'%' if source[i..].starts_with("%%") => i = skip_past(source, i + 2, "%%"),
            b'!' | b'[' => {
                let open = if bytes[i] == b'!' { i + 1 } else { i };
                match source[open..]
                    .starts_with("[[")
                    .then(|| wikilink(source, open))
                    .flatten()
                {
                    Some(link) => i = link.span.end,
                    None => i += 1,
                }
            }
            b'#' if tag_end == Some(i) || source[..i].ends_with(char::is_whitespace) || i == 0 => {
                let name_len = source[i + 1..]
                    .find(|c| !is_tag_char(c))
                    .unwrap_or(source.len() - i - 1);
                let name = &source[i + 1..i + 1 + name_len];
                i += 1 + name_len;
                if !name.is_empty() && is_tag(name) {
                    found.push(name.to_string());
                    tag_end = Some(i);
                }
            }
            _ => i += source[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    found
}

/// Follows code blocks line by line: fenced, or indented where a list or
/// paragraph doesn't continue.
#[derive(Default)]
struct Lines {
    fences: Fences,
    /// The last line was blank, a code line or a fence, or there was none.
    after_break: bool,
    /// Inside a list item, whose indented lines are its own.
    in_list: bool,
    started: bool,
}

impl Lines {
    fn is_code(&mut self, line: &str) -> bool {
        let first = !std::mem::replace(&mut self.started, true);
        let after_break = first || self.after_break;
        if self.fences.is_code(line) {
            self.after_break = true;
            self.in_list = false;
            return true;
        }
        let content = line.trim_start_matches([' ', '\t']);
        if content.trim().is_empty() {
            self.after_break = true;
            return false;
        }
        let indented =
            line.starts_with('\t') || line.len() - line.trim_start_matches(' ').len() >= 4;
        if indented && after_break && !self.in_list {
            self.after_break = true;
            return true;
        }
        if !indented {
            self.in_list = is_list_item(content);
        }
        self.after_break = false;
        false
    }
}

/// `- item`, `* item`, `+ item`, `1. item` or `1) item`.
fn is_list_item(line: &str) -> bool {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let rest = match digits {
        0 => line.strip_prefix(['-', '*', '+']),
        _ => line[digits..].strip_prefix(['.', ')']),
    };
    rest.is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '\n', '\r']))
}

/// Past `$math$` or `$$math$$` at `start`, or just past the `$` if it
/// isn't math.
fn skip_math(source: &str, start: usize) -> usize {
    if source[start..].starts_with("$$") {
        return skip_past(source, start + 2, "$$");
    }
    let rest = &source[start + 1..];
    let line = &rest[..rest.find('\n').unwrap_or(rest.len())];
    if line.starts_with(char::is_whitespace) {
        return start + 1;
    }
    match line.find('$') {
        Some(end) if end > 0 && !line[..end].ends_with(char::is_whitespace) => start + 1 + end + 1,
        _ => start + 1,
    }
}

/// Past the next `close` from `from`, or the end if nothing closes.
fn skip_past(source: &str, from: usize, close: &str) -> usize {
    source[from..]
        .find(close)
        .map_or(source.len(), |n| from + n + close.len())
}
