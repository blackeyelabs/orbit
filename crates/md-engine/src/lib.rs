//! Markdown parsing, block cache and the span/decoration model.

mod callout;
mod formatting;
mod front_matter;
mod links;
mod tags;

pub use callout::{Callout, CalloutKind, callout};
pub use formatting::{Format, TextEdit, format_edit};
pub use front_matter::{aliases, front_matter_tags};
pub use links::{
    Heading, Link, LinkStyle, has_scheme, heading_offset, headings, links, percent_decode,
    split_target, typed_link,
};
pub use tags::inline_tags;

use unicode_segmentation::UnicodeSegmentation as _;

/// Words in a note's source outside its front matter, split at Unicode word
/// boundaries (UAX #29): `don't` is one word, and each CJK character is one.
/// Markdown punctuation isn't a word; link targets and other syntax that
/// reads as words still count.
pub fn word_count(source: &str) -> usize {
    source[front_matter_len(source)..].unicode_words().count()
}

/// A note's text after its front matter.
pub fn without_front_matter(source: &str) -> &str {
    &source[front_matter_len(source)..]
}

/// The byte length of a leading YAML front matter block (with any BOM), or 0.
fn front_matter_len(source: &str) -> usize {
    let start = if source.starts_with('\u{feff}') { 3 } else { 0 };
    let mut lines = source[start..].split_inclusive('\n');
    let Some(first) = lines.next().filter(|l| l.trim_end() == "---") else {
        return 0;
    };
    let mut len = start + first.len();
    for line in lines {
        len += line.len();
        if matches!(line.trim_end(), "---" | "...") {
            return len;
        }
    }
    0
}
