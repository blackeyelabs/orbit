//! Links in a note's source, parsed as Obsidian parses them: wikilinks
//! (`[[target#heading|alias]]`), embeds (`![[…]]`, `![alt](…)`) and
//! markdown links to vault paths. Front matter, code and escaped brackets
//! hold no links; web addresses aren't vault links and are left out.

use std::ops::Range;

use crate::front_matter_len;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkStyle {
    /// `[[…]]`
    Wiki,
    /// `[text](…)`
    Markdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Where the whole link sits in the source, in bytes.
    pub span: Range<usize>,
    /// What it points at: a path or name, then any `#heading` (or `#^block`),
    /// percent-decoded for markdown links.
    pub target: String,
    /// What it shows: the alias, the link text, or else the target with
    /// headings as `Note > Heading`.
    pub display: String,
    /// `![[…]]` or `![…](…)`: shown inside the note rather than linked.
    pub embed: bool,
    pub style: LinkStyle,
}

impl Link {
    /// The note or file part of the target, without its `#heading`.
    pub fn path(&self) -> &str {
        split_target(&self.target).0
    }

    /// The heading (or `^block`) part of the target, after the first `#`.
    pub fn subpath(&self) -> Option<&str> {
        split_target(&self.target).1
    }
}

/// `Note#Heading` → (`Note`, `Some("Heading")`).
pub fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((path, subpath)) => (path, Some(subpath)),
        None => (target, None),
    }
}

/// Every link in `source`, in order.
pub fn links(source: &str) -> Vec<Link> {
    let mut found = Vec::new();
    let bytes = source.as_bytes();
    let mut i = front_matter_len(source);
    let mut line_start = true;
    let mut fences = Fences::default();
    while i < bytes.len() {
        if line_start {
            line_start = false;
            let line_end = source[i..].find('\n').map_or(source.len(), |n| i + n + 1);
            if fences.is_code(&source[i..line_end]) {
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
            b'\\' if bytes.get(i + 1) != Some(&b'\n') => i += 2,
            b'`' => i = skip_code_span(source, i),
            b'[' | b'!' => match link_at(source, i) {
                Some(link) => {
                    i = link.span.end;
                    found.push(link);
                }
                None => i += 1,
            },
            _ => i += 1,
        }
    }
    found
}

/// Follows fenced code blocks line by line.
#[derive(Default)]
struct Fences {
    /// The open fence's character and length.
    open: Option<(u8, usize)>,
}

impl Fences {
    /// Whether `line` is a fence or inside a fenced block.
    fn is_code(&mut self, line: &str) -> bool {
        match (fence_marker(line), self.open) {
            (Some(marker), None) => self.open = Some(marker),
            (Some((ch, len)), Some((open_ch, open_len)))
                if ch == open_ch && len >= open_len && is_closing(line) =>
            {
                self.open = None
            }
            (_, None) => return false,
            _ => {}
        }
        true
    }
}

/// A fence opening or closing line: its character and length.
fn fence_marker(line: &str) -> Option<(u8, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let ch = *trimmed.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = trimmed.bytes().take_while(|&b| b == ch).count();
    // A backtick fence's info string can't hold backticks.
    let rest = &trimmed[len..];
    (len >= 3 && !(ch == b'`' && rest.contains('`'))).then_some((ch, len))
}

fn is_closing(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.bytes().all(|b| b == trimmed.as_bytes()[0])
}

/// Past the code span starting at `start` (a run of backticks), or just
/// past the run if nothing closes it before a blank line.
fn skip_code_span(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let run = bytes[start..].iter().take_while(|&&b| b == b'`').count();
    let mut i = start + run;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"\n\n") || bytes[i..].starts_with(b"\n\r\n") {
            break;
        }
        if bytes[i] == b'`' {
            let len = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if len == run {
                return i + len;
            }
            i += len;
        } else {
            i += 1;
        }
    }
    start + run
}

/// The link starting at `i` (at its `[`, or the `!` of an embed).
fn link_at(source: &str, i: usize) -> Option<Link> {
    let (embed, open) = match source[i..].strip_prefix('!') {
        Some(rest) if rest.starts_with('[') => (true, i + 1),
        Some(_) => return None,
        None => (false, i),
    };
    let link = if source[open..].starts_with("[[") {
        wikilink(source, open)
    } else {
        markdown_link(source, open)
    }?;
    Some(Link {
        span: i..link.span.end,
        embed,
        ..link
    })
}

/// `[[target#heading|alias]]` at `open`, all on one line.
fn wikilink(source: &str, open: usize) -> Option<Link> {
    let inner_start = open + 2;
    let rest = &source[inner_start..];
    let line = &rest[..rest.find('\n').unwrap_or(rest.len())];
    let close = line.find("]]")?;
    let inner = &rest[..close];
    if inner.is_empty() || inner.contains("[[") {
        return None;
    }
    // In a table the pipe is written `\|`.
    let (target, alias) = match inner.split_once('|') {
        Some((target, alias)) => (target.strip_suffix('\\').unwrap_or(target), Some(alias)),
        None => (inner, None),
    };
    let display = match (alias, split_target(target)) {
        (Some(alias), _) => alias.to_string(),
        (None, ("", Some(subpath))) => subpath.replace('#', " > "),
        (None, (path, Some(subpath))) => format!("{path} > {}", subpath.replace('#', " > ")),
        (None, _) => target.to_string(),
    };
    Some(Link {
        span: open..inner_start + close + 2,
        target: target.to_string(),
        display,
        embed: false,
        style: LinkStyle::Wiki,
    })
}

/// `[text](destination "title")` at `open`, to a vault path.
fn markdown_link(source: &str, open: usize) -> Option<Link> {
    let bytes = source.as_bytes();
    // The text: brackets nest, and a blank line ends the search.
    let mut depth = 0;
    let mut i = open;
    let text_end = loop {
        match *bytes.get(i)? {
            b'\\' => i += 1,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break i;
                }
            }
            b'\n' if bytes.get(i + 1) == Some(&b'\n') => return None,
            _ => {}
        }
        i += 1;
    };
    if bytes.get(text_end + 1) != Some(&b'(') {
        return None;
    }
    let dest_start = text_end + 2;
    let (destination, after) = if bytes.get(dest_start) == Some(&b'<') {
        let close = source[dest_start..].find('>')? + dest_start;
        (&source[dest_start + 1..close], close + 1)
    } else {
        let mut depth = 0;
        let mut j = dest_start;
        while let Some(&b) = bytes.get(j) {
            match b {
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                b' ' | b'\t' | b'\n' => break,
                _ => {}
            }
            j += 1;
        }
        (&source[dest_start..j], j)
    };
    // An optional title, then the closing parenthesis.
    let rest = &source[after..];
    let trimmed = rest.trim_start_matches([' ', '\t']);
    let close = match trimmed.as_bytes().first()? {
        b')' => after + (rest.len() - trimmed.len()),
        b'"' | b'\'' => {
            let quote = trimmed.chars().next()?;
            let end = trimmed[1..].find(quote)? + 2;
            let tail = trimmed[end..].trim_start_matches([' ', '\t']);
            if !tail.starts_with(')') {
                return None;
            }
            after + (rest.len() - tail.len())
        }
        _ => return None,
    };
    if destination.is_empty() || has_scheme(destination) || destination.contains('\n') {
        return None;
    }
    Some(Link {
        span: open..close + 1,
        target: percent_decode(destination).unwrap_or_else(|| destination.to_string()),
        display: source[open + 1..text_end].to_string(),
        embed: false,
        style: LinkStyle::Markdown,
    })
}

/// `https:`, `mailto:` and the like: not a vault path.
pub fn has_scheme(destination: &str) -> bool {
    match destination.split_once(':') {
        Some((scheme, _)) => {
            scheme.len() > 1
                && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
        None => false,
    }
}

/// Decodes `%XX` escapes; `None` if they don't decode to UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The front matter's `aliases` (or `alias`): a flow list, a block list or
/// one value, each unquoted.
pub fn aliases(source: &str) -> Vec<String> {
    let len = front_matter_len(source);
    if len == 0 {
        return Vec::new();
    }
    let mut lines = source[..len].lines().skip(1);
    while let Some(line) = lines.next() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if !matches!(key, "aliases" | "alias") {
            continue;
        }
        let value = value.trim();
        if let Some(list) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            return list.split(',').filter_map(unquote).collect();
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

fn unquote(value: &str) -> Option<String> {
    let value = value.trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|&q| value.strip_prefix(q).and_then(|v| v.strip_suffix(q)))
        .unwrap_or(value);
    (!unquoted.is_empty()).then(|| unquoted.to_string())
}

/// A heading in a note's source, outside code: `## Text`, or text
/// underlined with `===` or `---`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// 1 to 6.
    pub level: u8,
    /// Its text as written, without the `#`s around it; an underlined
    /// heading's lines joined with spaces.
    pub text: String,
    /// Where its (first) line starts in the source, in bytes.
    pub offset: usize,
}

/// The block a line is in, as far as underlined headings care.
enum Block {
    /// Between blocks.
    None,
    /// A paragraph, which an underline makes a heading: where it starts,
    /// and its lines so far.
    Paragraph(usize, Vec<String>),
    /// A list item, quote or indented code, which can't be underlined.
    Other,
}

/// The source's headings in order. One without text isn't listed.
pub fn headings(source: &str) -> Vec<Heading> {
    let mut found = Vec::new();
    let mut offset = front_matter_len(source);
    let mut fences = Fences::default();
    let mut block = Block::None;
    for line in source[offset..].split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if fences.is_code(line) {
            block = Block::None;
            continue;
        }
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        let content = trimmed.trim_end();
        if content.is_empty() {
            block = Block::None;
            continue;
        }
        if let Some((level, text)) = atx_heading(trimmed).filter(|_| indent <= 3) {
            block = Block::None;
            if !text.is_empty() {
                found.push(Heading {
                    level,
                    text: text.to_string(),
                    offset: start,
                });
            }
            continue;
        }
        block = match block {
            Block::Paragraph(first, lines) if indent <= 3 && is_underline(content) => {
                let level = if content.starts_with('=') { 1 } else { 2 };
                found.push(Heading {
                    level,
                    text: lines.join(" "),
                    offset: first,
                });
                Block::None
            }
            Block::Paragraph(first, mut lines) => {
                lines.push(content.to_string());
                Block::Paragraph(first, lines)
            }
            Block::Other => Block::Other,
            Block::None if indent > 3 || starts_other_block(content) => Block::Other,
            Block::None => Block::Paragraph(start, vec![content.to_string()]),
        };
    }
    found
}

/// `## Text ##` → (2, `Text`).
fn atx_heading(line: &str) -> Option<(u8, &str)> {
    let level = line.bytes().take_while(|&b| b == b'#').count();
    let text = &line[level..];
    if !(1..=6).contains(&level) || !(text.starts_with([' ', '\t']) || text.trim().is_empty()) {
        return None;
    }
    Some((level as u8, text.trim().trim_end_matches('#').trim()))
}

/// `===` or `---`, all one character.
fn is_underline(line: &str) -> bool {
    let first = line.as_bytes()[0];
    (first == b'=' || first == b'-') && line.bytes().all(|b| b == first)
}

/// A list item, quote, rule or table row: never a heading's text.
fn starts_other_block(line: &str) -> bool {
    let ordered = line.split_once(['.', ')']).is_some_and(|(n, rest)| {
        !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
            && (rest.is_empty() || rest.starts_with(' '))
    });
    let bullet = matches!(
        line.as_bytes(),
        [b'-' | b'*' | b'+'] | [b'-' | b'*' | b'+', b' ' | b'\t', ..]
    );
    let marks: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    let rule = marks.len() >= 3
        && matches!(marks[0], '*' | '_' | '-')
        && marks.iter().all(|&c| c == marks[0]);
    ordered || bullet || rule || line.starts_with(['>', '|'])
}

/// Where the heading a link's subpath names starts. Matching ignores case
/// and surrounding space; for nested subpaths (`A#B`) the last one counts.
pub fn heading_offset(source: &str, subpath: &str) -> Option<usize> {
    let wanted = subpath.rsplit('#').next()?.trim().to_lowercase();
    headings(source)
        .into_iter()
        .find(|h| h.text.to_lowercase() == wanted)
        .map(|h| h.offset)
}
