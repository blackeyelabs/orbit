//! A note split into blocks, and each block's editable text as one or more
//! *leaves*, grown from Spike 4's model. A leaf is a source range the
//! editor renders as one keyed text leaf. Source outside every leaf (the
//! blank line between blocks) is never under the caret.
//!
//! Paragraphs and headings render; every other block (lists, quotes, code,
//! tables, rules, front matter) shows its source as typed, nothing
//! concealed, until its own Live Preview ticket gives it a widget.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation as _;

/// What a block renders as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Prose, its inline syntax concealed.
    Paragraph,
    /// An ATX heading of this level: its leaf is the whole line, and the
    /// conceal model hides the `#`s.
    Heading(u8),
    /// A blank line the caret can stand on: an empty paragraph.
    Empty,
    /// A block shown as its source, verbatim: the leaf is all of it.
    Source,
}

/// One block and the leaves it owns (indices into [`Blocks::leaves`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// What it renders as.
    pub kind: Kind,
    /// Its leaves.
    pub leaves: Range<usize>,
}

/// A keyed text leaf: where its text is in the source, and whether inline
/// syntax in it is concealed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    /// The leaf's text, as a source range.
    pub source: Range<usize>,
    /// Prose conceals its syntax; a [`Kind::Source`] block doesn't.
    pub conceals: bool,
}

/// A note split into blocks and leaves. Rebuilt from the source after
/// every edit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blocks {
    /// Top to bottom.
    pub blocks: Vec<Block>,
    /// Every block's leaves, in source order.
    pub leaves: Vec<Leaf>,
}

/// One line of the source, without its `\n`.
struct Line<'a> {
    start: usize,
    text: &'a str,
}

impl Line<'_> {
    /// Where the line ends, before its `\n`.
    fn end(&self) -> usize {
        self.start + self.text.len()
    }
}

/// The source's lines, with their offsets.
fn lines(source: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    for text in source.split('\n') {
        out.push(Line { start, text });
        start += text.len() + 1;
    }
    out
}

/// A line with nothing but whitespace.
fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// An ATX heading's level: one to six `#`s and a space.
fn heading_level(text: &str) -> Option<u8> {
    let hashes = text.bytes().take_while(|&b| b == b'#').count();
    let rest = &text[hashes..];
    ((1..=6).contains(&hashes) && rest.starts_with(' ')).then_some(hashes as u8)
}

/// A code fence's opening or closing line.
fn is_fence(text: &str) -> bool {
    text.trim_start().starts_with("```")
}

/// A table row: a line that starts with a pipe.
fn is_table_row(text: &str) -> bool {
    text.trim_start().starts_with('|')
}

/// A table's delimiter row, such as `|---|:--:|`.
fn is_delimiter_row(text: &str) -> bool {
    text.contains('-')
        && text
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// How many cells a table row has: the gaps between its pipes.
fn cell_count(text: &str) -> usize {
    text.matches('|').count().saturating_sub(1)
}

/// A table starts here: a row, then a delimiter row with a cell per
/// header cell (as in GFM, so a half-typed `|-` isn't one yet).
fn starts_table(line: &str, next: Option<&str>) -> bool {
    is_table_row(line)
        && next.is_some_and(|n| is_delimiter_row(n) && cell_count(n) == cell_count(line))
}

/// A list item's line: a bullet (`-`, `*`, `+`) or a number and `.`, then
/// a space.
fn is_list_item(text: &str) -> bool {
    let rest = text.trim_start();
    if ["- ", "* ", "+ "].iter().any(|m| rest.starts_with(m)) {
        return true;
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && rest[digits..].starts_with(". ")
}

/// A quote's line.
fn is_quote(text: &str) -> bool {
    text.trim_start().starts_with('>')
}

/// A thematic break: three or more `-`, `*` or `_`, spaces allowed between.
fn is_rule(text: &str) -> bool {
    let marks: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|&m| marks.chars().all(|c| c == m))
}

/// A line that ends a paragraph by starting another block.
fn starts_block(text: &str) -> bool {
    heading_level(text).is_some()
        || is_fence(text)
        || is_rule(text)
        || is_list_item(text)
        || is_quote(text)
}

impl Blocks {
    /// Splits `source` into blocks and leaves.
    pub fn parse(source: &str) -> Blocks {
        let lines = lines(source);
        let mut doc = Blocks::default();
        let mut i = 0;
        // Front matter, from a `---` first line to the next `---`.
        let front = source.len() - md_engine::without_front_matter(source).len();
        if front > 0 {
            let end = lines
                .iter()
                .position(|l| l.start >= front)
                .unwrap_or(lines.len());
            doc.push_source(&lines[0..end]);
            i = end;
        }
        // A blank line right after a block only separates it from the next.
        let mut after_block = i > 0;
        while i < lines.len() {
            let line = &lines[i];
            if is_blank(line.text) {
                // After a block, a run of blank lines keeps its *last* one
                // as the separator before the next block, and every other
                // is an empty line to stand on. So Enter at a block's end
                // puts the caret on the very next line.
                let run = (i..lines.len())
                    .take_while(|&j| is_blank(lines[j].text))
                    .count();
                let separators = usize::from(after_block);
                for line in &lines[i..i + run - separators.min(run)] {
                    doc.push(Kind::Empty, line.start..line.start, true);
                }
                after_block = false;
                i += run;
                continue;
            }
            after_block = true;
            let next = lines.get(i + 1).map(|l| l.text);
            // Where a block that runs while `more` holds ends.
            let run_while = |more: &dyn Fn(&str) -> bool| {
                (i..lines.len())
                    .find(|&j| !more(lines[j].text))
                    .unwrap_or(lines.len())
            };
            if let Some(level) = heading_level(line.text) {
                doc.push(Kind::Heading(level), line.start..line.end(), true);
                i += 1;
            } else if is_fence(line.text) {
                let close = (i + 1..lines.len()).find(|&j| is_fence(lines[j].text));
                let end = close.map_or(lines.len(), |c| c + 1);
                doc.push_source(&lines[i..end]);
                i = end;
            } else if is_rule(line.text) {
                doc.push_source(&lines[i..i + 1]);
                i += 1;
            } else if starts_table(line.text, next) {
                let end = run_while(&is_table_row);
                doc.push_source(&lines[i..end]);
                i = end;
            } else if is_list_item(line.text) {
                let end = run_while(&is_list_item);
                doc.push_source(&lines[i..end]);
                i = end;
            } else if is_quote(line.text) {
                let end = run_while(&is_quote);
                doc.push_source(&lines[i..end]);
                i = end;
            } else {
                let end = (i + 1..lines.len())
                    .find(|&j| {
                        let next = lines.get(j + 1).map(|l| l.text);
                        is_blank(lines[j].text)
                            || starts_block(lines[j].text)
                            || starts_table(lines[j].text, next)
                    })
                    .unwrap_or(lines.len());
                doc.push(Kind::Paragraph, line.start..lines[end - 1].end(), true);
                i = end;
            }
        }
        // The note's end is always somewhere to stand.
        if doc.leaf_at(source.len()).is_none() {
            doc.push(Kind::Empty, source.len()..source.len(), true);
        }
        doc
    }

    /// Adds a block with one leaf.
    fn push(&mut self, kind: Kind, source: Range<usize>, conceals: bool) {
        let first = self.leaves.len();
        self.leaves.push(Leaf { source, conceals });
        self.blocks.push(Block {
            kind,
            leaves: first..first + 1,
        });
    }

    /// Adds a block shown as its source: these lines, verbatim.
    fn push_source(&mut self, lines: &[Line]) {
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
            return;
        };
        self.push(Kind::Source, first.start..last.end(), false);
    }

    /// The block a leaf belongs to.
    pub fn block_of(&self, leaf: usize) -> Option<&Block> {
        self.blocks.get(self.block_index_of(leaf)?)
    }

    /// The index of the block a leaf belongs to.
    pub fn block_index_of(&self, leaf: usize) -> Option<usize> {
        self.blocks.iter().position(|b| b.leaves.contains(&leaf))
    }

    /// The leaf the caret at `offset` is in: the first whose range holds it
    /// (ends included).
    pub fn leaf_at(&self, offset: usize) -> Option<usize> {
        let after = self.leaves.partition_point(|l| l.source.end < offset);
        (after < self.leaves.len() && self.leaves[after].source.start <= offset).then_some(after)
    }

    /// Where a caret at `offset` may stand: inside a leaf as is, else the
    /// nearest leaf boundary (the end of the leaf before when `back`).
    pub fn snap(&self, offset: usize, back: bool) -> usize {
        if self.leaf_at(offset).is_some() || self.leaves.is_empty() {
            return offset;
        }
        let next = self.leaves.partition_point(|l| l.source.start <= offset);
        match (back, next) {
            (_, 0) => self.leaves[0].source.start,
            (_, n) if n == self.leaves.len() => self.leaves[n - 1].source.end,
            (true, n) => self.leaves[n - 1].source.end,
            (false, n) => self.leaves[n].source.start,
        }
    }
}

impl Blocks {
    /// The caret stop after `offset`: the next grapheme within its leaf,
    /// else the start of the next leaf.
    pub fn next_stop(&self, source: &str, offset: usize) -> usize {
        let Some(leaf) = self.leaf_at(offset) else {
            return self.snap(offset, false);
        };
        let range = &self.leaves[leaf].source;
        if offset < range.end {
            return next_grapheme(source, offset).min(range.end);
        }
        self.leaves
            .get(leaf + 1)
            .map_or(offset, |next| next.source.start)
    }

    /// The caret stop before `offset`: the previous grapheme within its
    /// leaf, else the end of the previous leaf.
    pub fn prev_stop(&self, source: &str, offset: usize) -> usize {
        let Some(leaf) = self.leaf_at(offset) else {
            return self.snap(offset, true);
        };
        let range = &self.leaves[leaf].source;
        if offset > range.start {
            return prev_grapheme(source, offset).max(range.start);
        }
        leaf.checked_sub(1)
            .map_or(offset, |prev| self.leaves[prev].source.end)
    }
}

/// The grapheme boundary after `offset`.
pub(crate) fn next_grapheme(source: &str, offset: usize) -> usize {
    source[offset..]
        .grapheme_indices(true)
        .nth(1)
        .map_or(source.len(), |(i, _)| offset + i)
}

/// The grapheme boundary before `offset`.
pub(crate) fn prev_grapheme(source: &str, offset: usize) -> usize {
    source[..offset]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}
