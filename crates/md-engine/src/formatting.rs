//! The source editor's formatting shortcuts, as plain text edits.

use std::ops::Range;

/// A formatting shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Ctrl+B: `**bold**`.
    Bold,
    /// Ctrl+I: `*italic*`.
    Italic,
    /// Ctrl+K: `[text](url)`.
    Link,
}

/// Replace `range` of the old text with `text`; then select `selection`,
/// in the new text's byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

/// The edit a formatting shortcut makes to `source` with `selection`
/// (byte offsets) selected.
pub fn format_edit(source: &str, selection: Range<usize>, kind: Format) -> TextEdit {
    match kind {
        Format::Bold => toggle_emphasis(source, selection, 2),
        Format::Italic => toggle_emphasis(source, selection, 1),
        Format::Link => link(source, selection),
    }
}

/// `[selection]()` with the caret in the parentheses; `[](url)` with the
/// caret in the brackets when a URL is selected; `[]()` with the caret in the
/// brackets when nothing is.
fn link(source: &str, sel: Range<usize>) -> TextEdit {
    let sel = trim(source, sel);
    let selected = &source[sel.clone()];
    let is_url = ["https://", "http://", "mailto:"]
        .iter()
        .any(|scheme| selected.starts_with(scheme))
        && !selected.contains(char::is_whitespace);
    let (text, caret) = if selected.is_empty() {
        ("[]()".to_string(), sel.start + 1)
    } else if is_url {
        (format!("[]({selected})"), sel.start + 1)
    } else {
        (format!("[{selected}]()"), sel.end + 3)
    };
    TextEdit {
        range: sel,
        text,
        selection: caret..caret,
    }
}

/// Wraps the selection in `stars` asterisks, or removes them if it's
/// already wrapped, with the markers inside or just outside the selection.
fn toggle_emphasis(source: &str, sel: Range<usize>, stars: usize) -> TextEdit {
    // `** it **` isn't emphasis, so spaces stay outside the markers.
    let sel = trim(source, sel);
    let selected = &source[sel.clone()];
    // The markers are selected too: `‹**it**›`.
    let inside = leading_stars(selected).min(trailing_stars(selected));
    let inner = selected
        .get(stars..selected.len().saturating_sub(stars))
        .unwrap_or("");
    // `‹*a* and *b*›` is two emphases, not one around `a* and *b`.
    let one_pair = !star_runs(inner).any(|run| wraps(run, stars));
    if wraps(inside, stars) && selected.len() > 2 * stars && one_pair {
        return TextEdit {
            range: sel.clone(),
            text: inner.to_string(),
            selection: sel.start..sel.start + inner.len(),
        };
    }
    // The markers are just outside: `**‹it›**`.
    let outside = trailing_stars(&source[..sel.start]).min(leading_stars(&source[sel.end..]));
    if wraps(outside, stars) {
        return TextEdit {
            range: sel.start - stars..sel.end + stars,
            text: selected.to_string(),
            selection: sel.start - stars..sel.end - stars,
        };
    }
    let marker = "*".repeat(stars);
    TextEdit {
        range: sel.clone(),
        text: format!("{marker}{selected}{marker}"),
        selection: sel.start + stars..sel.end + stars,
    }
}

/// Whether a run of `run` asterisks on both sides includes emphasis of
/// `stars` asterisks: `***` is both bold (2) and italic (1).
fn wraps(run: usize, stars: usize) -> bool {
    match stars {
        2 => run >= 2,
        _ => run % 2 == 1,
    }
}

fn trim(source: &str, sel: Range<usize>) -> Range<usize> {
    let selected = &source[sel.clone()];
    let start = sel.start + (selected.len() - selected.trim_start().len());
    let end = sel.end - (selected.len() - selected.trim_end().len());
    start..end.max(start)
}

/// The lengths of the runs of asterisks in `s`.
fn star_runs(s: &str) -> impl Iterator<Item = usize> + '_ {
    s.split(|c| c != '*').map(str::len).filter(|&n| n > 0)
}

fn leading_stars(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b'*').count()
}

fn trailing_stars(s: &str) -> usize {
    s.bytes().rev().take_while(|&b| b == b'*').count()
}
