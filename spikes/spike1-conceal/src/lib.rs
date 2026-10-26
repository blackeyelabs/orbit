//! Spike 1: the Live Preview conceal model.
//!
//! Given a note's source text and the selection, `layout` decides which
//! markdown syntax is hidden (syntax shows only where the caret or selection
//! touches its span), maps offsets between source and display text, and
//! reports styled runs over the display text.
//!
//! Known limits (spike scope): no nesting of the same construct, no escapes,
//! lists, quotes or setext headings. A heading's style run can overlap the
//! inline runs inside it; the renderer merges them.

use std::ops::Range;

/// The caret or selection, as byte offsets into the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    /// The reveal rule: touching a span's boundary counts.
    fn touches(&self, span: &Range<usize>) -> bool {
        let sel = self.range();
        sel.start <= span.end && span.start <= sel.end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Bold,
    Italic,
    Strike,
    Code,
    Link,
    Heading(u8),
    /// Markdown syntax that is showing because the caret revealed it.
    Syntax,
}

/// A styled range of the display text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub range: Range<usize>,
    pub style: Style,
}

/// What the editor shows for one layout pass.
#[derive(Debug)]
pub struct Layout {
    display: String,
    source_len: usize,
    /// Hidden source ranges, sorted and non-overlapping.
    hidden: Vec<Range<usize>>,
    runs: Vec<Run>,
}

impl Layout {
    pub fn display_text(&self) -> &str {
        &self.display
    }

    /// Styled runs over the display text, sorted by start.
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// Display offset for a source offset. An offset inside hidden syntax
    /// collapses to the display position where that syntax would be.
    pub fn to_display(&self, source: usize) -> usize {
        let mut removed = 0;
        for r in &self.hidden {
            if source <= r.start {
                break;
            }
            removed += source.min(r.end) - r.start;
        }
        source - removed
    }

    /// Source offset for a display offset: the start of the next visible
    /// source character (or the end of the source).
    pub fn to_source(&self, display: usize) -> usize {
        let mut source = display;
        for r in &self.hidden {
            if source < r.start {
                break;
            }
            source += r.len();
        }
        source.min(self.source_len)
    }
}

pub fn layout(source: &str, selection: Selection) -> Layout {
    let spans: Vec<(Span, bool)> = parse(source)
        .into_iter()
        .map(|s| {
            let revealed = selection.touches(&s.range);
            (s, revealed)
        })
        .collect();

    let mut hidden: Vec<Range<usize>> = spans
        .iter()
        .filter(|(_, revealed)| !revealed)
        .flat_map(|(s, _)| s.syntax.iter().cloned())
        .collect();
    // Heading and inline spans are found in separate passes.
    hidden.sort_by_key(|r| r.start);

    let mut display = String::with_capacity(source.len());
    let mut at = 0;
    for r in &hidden {
        display.push_str(&source[at..r.start]);
        at = r.end;
    }
    display.push_str(&source[at..]);

    let mut layout = Layout {
        display,
        source_len: source.len(),
        hidden,
        runs: Vec::new(),
    };
    let mut runs = Vec::new();
    for (span, revealed) in &spans {
        if *revealed {
            runs.extend(span.syntax.iter().map(|r| Run {
                range: r.clone(),
                style: Style::Syntax,
            }));
        }
        runs.push(Run {
            range: span.content(),
            style: span.style,
        });
    }
    layout.runs = runs
        .into_iter()
        .map(|r| Run {
            range: layout.to_display(r.range.start)..layout.to_display(r.range.end),
            ..r
        })
        .filter(|r| !r.range.is_empty())
        .collect();
    layout.runs.sort_by_key(|r| r.range.start);
    layout
}

/// A piece of markdown syntax: its whole extent, the parts of it that are pure
/// syntax (hidden unless revealed), and how its content is styled.
#[derive(Debug)]
struct Span {
    range: Range<usize>,
    syntax: Vec<Range<usize>>,
    style: Style,
}

impl Span {
    /// The visible content: after the opening syntax, before the closing one.
    fn content(&self) -> Range<usize> {
        let start = self.syntax[0].end;
        let end = match self.syntax.as_slice() {
            [_] => self.range.end,
            [.., close] => close.start,
            [] => unreachable!("every span has syntax"),
        };
        start..end
    }
}

/// Paired inline delimiters. Code comes first so its content is never
/// parsed; longer delimiters come before their prefixes (`**` before `*`).
const PAIRS: &[(&str, Style)] = &[
    ("`", Style::Code),
    ("**", Style::Bold),
    ("~~", Style::Strike),
    ("*", Style::Italic),
    ("_", Style::Italic),
];

fn parse(source: &str) -> Vec<Span> {
    let mut spans = headings(source);
    let mut i = 0;
    while i < source.len() {
        let span = wikilink(source, i).or_else(|| link(source, i)).or_else(|| {
            PAIRS
                .iter()
                .find_map(|&(d, style)| paired(source, i, d, style))
        });
        if let Some(span) = span {
            i = span.range.end;
            spans.push(span);
        } else {
            i += source[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    spans
}

/// `delim content delim` starting at `at`, with non-empty content on one line.
fn paired(source: &str, at: usize, delim: &str, style: Style) -> Option<Span> {
    let rest = source[at..].strip_prefix(delim)?;
    let len = rest.find(delim)?;
    if len == 0 || rest[..len].contains('\n') {
        return None;
    }
    let close = at + delim.len() + len;
    // `_` only delimits at word edges, so snake_case_names stay literal.
    if delim == "_" && (is_word_char_before(source, at) || is_word_char_at(source, close + 1)) {
        return None;
    }
    Some(Span {
        range: at..close + delim.len(),
        syntax: vec![at..at + delim.len(), close..close + delim.len()],
        style,
    })
}

/// `[[target]]` shows the target; `[[target|alias]]` shows the alias. An
/// empty alias (`[[target|]]`) shows the target.
fn wikilink(source: &str, at: usize) -> Option<Span> {
    let inner = source[at..].strip_prefix("[[")?;
    let len = inner
        .find("]]")
        .filter(|&n| n > 0 && !inner[..n].contains('\n'))?;
    let body = at + 2..at + 2 + len;
    let mut open = at..body.start;
    let mut close = body.end..body.end + 2;
    if let Some(bar) = inner[..len].find('|').map(|b| body.start + b) {
        if bar + 1 < body.end {
            // Hide "target|" along with the opening brackets.
            open.end = bar + 1;
        } else {
            // Empty alias: hide the stray "|" with the closing brackets.
            close.start = bar;
        }
    }
    Some(Span {
        range: at..close.end,
        syntax: vec![open, close],
        style: Style::Link,
    })
}

/// `[text](url)` shows only the text.
fn link(source: &str, at: usize) -> Option<Span> {
    let inner = source[at..].strip_prefix('[')?;
    let text_len = inner
        .find(']')
        .filter(|&n| n > 0 && !inner[..n].contains('\n'))?;
    let url = inner[text_len + "]".len()..].strip_prefix('(')?;
    let url_len = url.find(')').filter(|&n| !url[..n].contains('\n'))?;
    let text_end = at + "[".len() + text_len;
    let end = text_end + "](".len() + url_len + ")".len();
    Some(Span {
        range: at..end,
        syntax: vec![at..at + 1, text_end..end],
        style: Style::Link,
    })
}

/// ATX headings: 1–6 `#` and a space at the start of a line. The span is the
/// whole line, so a caret anywhere on it reveals the markers.
fn headings(source: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut start = 0;
    for line in source.split('\n') {
        let hashes = line.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            let marker = start..start + hashes + 1;
            spans.push(Span {
                range: start..start + line.len(),
                syntax: vec![marker],
                style: Style::Heading(hashes as u8),
            });
        }
        start += line.len() + 1;
    }
    spans
}

fn is_word_char_before(source: &str, at: usize) -> bool {
    source[..at]
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
}

fn is_word_char_at(source: &str, at: usize) -> bool {
    source[at..]
        .chars()
        .next()
        .is_some_and(char::is_alphanumeric)
}
