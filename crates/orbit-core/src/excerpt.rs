//! A note's line cut down to the words around something in it: a link, for
//! backlinks, or the words a search found.

use std::ops::Range;

/// Characters kept on either side of what's shown.
const CONTEXT_CHARS: usize = 60;

/// Text from a note, with parts of it to mark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    /// A line, cut at words to about 60 characters either side of what it
    /// shows, with `…` where cut.
    pub text: String,
    /// The parts to mark, in bytes, in order.
    pub highlights: Vec<Range<usize>>,
}

/// `focus` in its line of `source`, with `marks` (those in what's kept).
/// A focus that runs over a line end, as a markdown link's text can, keeps
/// it, as a space.
pub(crate) fn excerpt(source: &str, focus: Range<usize>, marks: &[Range<usize>]) -> Snippet {
    let line_start = source[..focus.start].rfind('\n').map_or(0, |n| n + 1);
    let line_end = source[focus.end..]
        .find(['\n', '\r'])
        .map_or(source.len(), |n| focus.end + n);
    let before = source[line_start..focus.start].trim_start();
    let after = source[focus.end..line_end].trim_end();
    let (before, cut_before) = match before.char_indices().rev().nth(CONTEXT_CHARS - 1) {
        // Start at the next word, if there's one before the focus.
        Some((cut, _)) => {
            let rest = &before[cut..];
            let word = rest.find(char::is_whitespace).map_or(0, |n| n + 1);
            (rest[word..].trim_start(), true)
        }
        None => (before, false),
    };
    let (after, cut_after) = match after.char_indices().nth(CONTEXT_CHARS) {
        Some((cut, _)) => {
            let kept = &after[..cut];
            let word = kept.rfind(char::is_whitespace).unwrap_or(cut);
            (kept[..word].trim_end(), true)
        }
        None => (after, false),
    };
    let start = focus.start - before.len();
    let end = focus.end + after.len();
    let prefix = if cut_before { "…" } else { "" };
    let mut text = format!("{prefix}{}", source[start..end].replace(['\n', '\r'], " "));
    if cut_after {
        text.push('…');
    }
    let highlights = marks
        .iter()
        .filter(|mark| mark.start >= start && mark.end <= end)
        .map(|mark| mark.start - start + prefix.len()..mark.end - start + prefix.len())
        .collect();
    Snippet { text, highlights }
}
