//! The spike's document model, std only: a note split into blocks, each
//! block's editable text as one or more *leaves*, and the caret stops
//! between them. A leaf is a source range that Kit renders as one keyed
//! text leaf (a paragraph, a heading, a list item's text, a code block's
//! code, a table cell). Source outside every leaf (list markers, fences,
//! table pipes, the blank line between blocks) is never under the caret.

use std::ops::Range;

use spike1_conceal::{Layout, layout};
pub use spike1_conceal::{Run, Selection, Style};
use unicode_segmentation::UnicodeSegmentation as _;

/// A selection no span can touch, so a leaf lays out fully concealed.
const NOTHING_REVEALED: Selection = Selection {
    anchor: usize::MAX,
    head: usize::MAX,
};

/// What a block renders as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Paragraph,
    /// An ATX heading of this level; its leaf is the whole line, and the
    /// conceal model hides the `#`s.
    Heading(u8),
    /// One leaf per item, the text after the marker (and after `[ ] `).
    List {
        ordered: bool,
        /// Per item: `None` for a plain item, else whether it's ticked.
        checked: Vec<Option<bool>>,
    },
    /// The leaf is the code between the fences, never concealed.
    Code {
        lang: Option<String>,
    },
    /// One leaf per cell, row by row; the delimiter row has none. A
    /// short row has fewer cells than `columns`: the rest render empty
    /// and the caret can't stand in them.
    Table {
        columns: usize,
        /// How many cells (leaves) each row has.
        rows: Vec<usize>,
    },
    /// A blank line the caret can stand on: an empty paragraph.
    Empty,
    /// A thematic break (`---`): no leaf, the caret passes over it.
    Rule,
}

/// One block and the leaves it owns (indices into [`Doc::leaves`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    pub leaves: Range<usize>,
}

/// A keyed text leaf: where its text is in the source, and whether inline
/// syntax in it is concealed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub source: Range<usize>,
    pub conceals: bool,
}

impl Leaf {
    /// Prose: inline syntax in it is concealed.
    fn prose(source: Range<usize>) -> Leaf {
        Leaf {
            source,
            conceals: true,
        }
    }

    /// Code: shown verbatim.
    fn code(source: Range<usize>) -> Leaf {
        Leaf {
            source,
            conceals: false,
        }
    }
}

/// A note split into blocks and leaves. Rebuilt from the source after
/// every edit (the spike doesn't reparse incrementally).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Doc {
    pub blocks: Vec<Block>,
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

/// A list item's marker, as [`list_marker`] reads it.
struct Marker {
    /// Bytes from the line's start to the item's text: indent, marker,
    /// space and any task box.
    len: usize,
    ordered: bool,
    /// `None` for a plain item, else whether the task is ticked.
    checked: Option<bool>,
}

/// The list marker a line starts with, if any.
fn list_marker(text: &str) -> Option<Marker> {
    let indent = text.len() - text.trim_start().len();
    let rest = &text[indent..];
    let (marker, ordered) = if ["- ", "* ", "+ "].iter().any(|m| rest.starts_with(m)) {
        (2, false)
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || !rest[digits..].starts_with(". ") {
            return None;
        }
        (digits + 2, true)
    };
    let after = &rest[marker..];
    let (task, checked) = if after.starts_with("[ ] ") {
        (4, Some(false))
    } else if after.starts_with("[x] ") || after.starts_with("[X] ") {
        (4, Some(true))
    } else {
        (0, None)
    };
    Some(Marker {
        len: indent + marker + task,
        ordered,
        checked,
    })
}

/// A line that ends a paragraph by starting another block.
fn starts_block(text: &str) -> bool {
    heading_level(text).is_some() || is_fence(text) || is_rule(text) || list_marker(text).is_some()
}

/// A thematic break: three or more `-`, `*` or `_`, spaces allowed between.
/// (Right under a paragraph line, `---` would make it a setext heading in
/// CommonMark; the spike reads it as a rule.)
fn is_rule(text: &str) -> bool {
    let marks: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|&m| marks.chars().all(|c| c == m))
}

/// The cells of a table row: each cell's text, inside one padding space on
/// each side. An empty cell is an empty range just inside its padding, so
/// the caret can still stand in it.
fn cells(line: &Line) -> Vec<Range<usize>> {
    let text = line.text;
    let pipes: Vec<usize> = text.match_indices('|').map(|(i, _)| i).collect();
    pipes
        .windows(2)
        .map(|w| {
            let inner = &text[w[0] + 1..w[1]];
            // One padding space each side belongs to the pipes; the rest,
            // a space just typed at a cell's end included, is the cell's.
            let lead = usize::from(inner.starts_with(' '));
            let trail = usize::from(inner.len() > lead && inner.ends_with(' '));
            let start = line.start + w[0] + 1 + lead;
            start..start + inner.len() - lead - trail
        })
        .collect()
}

impl Doc {
    /// Splits `source` into blocks and leaves.
    pub fn parse(source: &str) -> Doc {
        let lines = lines(source);
        let mut doc = Doc::default();
        let mut i = 0;
        // A blank line right after a block only separates it from the next.
        let mut after_block = false;
        while i < lines.len() {
            let line = &lines[i];
            if is_blank(line.text) {
                if !after_block {
                    doc.push(Kind::Empty, [Leaf::prose(line.start..line.start)]);
                }
                after_block = false;
                i += 1;
                continue;
            }
            after_block = true;
            if let Some(level) = heading_level(line.text) {
                doc.push(Kind::Heading(level), [Leaf::prose(line.start..line.end())]);
                i += 1;
            } else if is_fence(line.text) {
                let lang = line.text.trim_start()[3..].trim();
                let lang = (!lang.is_empty()).then(|| lang.to_string());
                let close = (i + 1..lines.len()).find(|&j| is_fence(lines[j].text));
                let inner = i + 1..close.unwrap_or(lines.len());
                let code = if inner.is_empty() {
                    let at = lines.get(inner.start).map_or(source.len(), |l| l.start);
                    at..at
                } else {
                    lines[inner.start].start..lines[inner.end - 1].end()
                };
                doc.push(Kind::Code { lang }, [Leaf::code(code)]);
                i = close.map_or(lines.len(), |c| c + 1);
            } else if is_rule(line.text) {
                doc.push(Kind::Rule, []);
                i += 1;
            } else if is_table_row(line.text)
                && lines.get(i + 1).is_some_and(|l| is_delimiter_row(l.text))
            {
                let end = (i..lines.len())
                    .find(|&j| !is_table_row(lines[j].text))
                    .unwrap_or(lines.len());
                // A table is a header row, then a delimiter row, then rows.
                let rows: Vec<&Line> = lines[i..end]
                    .iter()
                    .enumerate()
                    .filter(|(n, _)| *n != 1)
                    .map(|(_, l)| l)
                    .collect();
                let columns = cells(rows[0]).len();
                // A long row is cut to the header's width; a short one
                // keeps only the cells it has.
                let cells: Vec<Vec<Range<usize>>> = rows
                    .iter()
                    .map(|row| cells(row).into_iter().take(columns).collect())
                    .collect();
                let counts = cells.iter().map(Vec::len).collect();
                doc.push(
                    Kind::Table {
                        columns,
                        rows: counts,
                    },
                    cells.into_iter().flatten().map(Leaf::prose),
                );
                i = end;
            } else if let Some(Marker { ordered, .. }) = list_marker(line.text) {
                let end = (i..lines.len())
                    .find(|&j| list_marker(lines[j].text).is_none())
                    .unwrap_or(lines.len());
                let items: Vec<_> = lines[i..end]
                    .iter()
                    .map(|l| {
                        let marker = list_marker(l.text).expect("a list line");
                        (l.start + marker.len..l.end(), marker.checked)
                    })
                    .collect();
                let checked = items.iter().map(|(_, c)| *c).collect();
                doc.push(
                    Kind::List { ordered, checked },
                    items.into_iter().map(|(r, _)| Leaf::prose(r)),
                );
                i = end;
            } else {
                let end = (i + 1..lines.len())
                    .find(|&j| is_blank(lines[j].text) || starts_block(lines[j].text))
                    .unwrap_or(lines.len());
                doc.push(
                    Kind::Paragraph,
                    [Leaf::prose(line.start..lines[end - 1].end())],
                );
                i = end;
            }
        }
        // The note's end is always somewhere to stand, so the caret can get
        // past a code block or table that ends it (an empty last line).
        if doc.leaf_at(source.len()).is_none() {
            doc.push(Kind::Empty, [Leaf::prose(source.len()..source.len())]);
        }
        doc
    }

    /// Adds a block and its leaves.
    fn push(&mut self, kind: Kind, leaves: impl IntoIterator<Item = Leaf>) {
        let first = self.leaves.len();
        self.leaves.extend(leaves);
        self.blocks.push(Block {
            kind,
            leaves: first..self.leaves.len(),
        });
    }

    /// Text typed at `offset` into an empty code block or an empty line goes
    /// on a line of its own: after a fence typed just before it (or the end
    /// of a block that ends the note), and before a closing fence right
    /// after it. Returns the line breaks to add before and after.
    pub fn line_breaks_for_insert(
        &self,
        source: &str,
        offset: usize,
    ) -> (&'static str, &'static str) {
        let Some(leaf) = self
            .leaf_at(offset)
            .filter(|&l| self.leaves[l].source.is_empty())
        else {
            return ("", "");
        };
        let block = self.blocks.iter().find(|b| b.leaves.contains(&leaf));
        if !block.is_some_and(|b| matches!(b.kind, Kind::Code { .. } | Kind::Empty)) {
            return ("", "");
        }
        let before = if offset > 0 && !source[..offset].ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let after = if source[offset..].starts_with("```") {
            "\n"
        } else {
            ""
        };
        (before, after)
    }

    /// The leaf the caret at `offset` is in: the first whose range holds it
    /// (ends included).
    pub fn leaf_at(&self, offset: usize) -> Option<usize> {
        self.leaves
            .iter()
            .position(|l| l.source.start <= offset && offset <= l.source.end)
    }

    /// Where a caret at `offset` may stand: inside a leaf as is, else the
    /// nearest leaf boundary (the end of the leaf before when `back`).
    pub fn snap(&self, offset: usize, back: bool) -> usize {
        if self.leaf_at(offset).is_some() || self.leaves.is_empty() {
            return offset;
        }
        let next = self.leaves.iter().position(|l| l.source.start > offset);
        match (back, next) {
            (_, Some(0)) => self.leaves[0].source.start,
            (true, Some(n)) => self.leaves[n - 1].source.end,
            (false, Some(n)) => self.leaves[n].source.start,
            (_, None) => self.leaves.last().expect("not empty").source.end,
        }
    }

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

    /// Each leaf's view for the given selection. A leaf the selection
    /// doesn't reach is laid out with nothing revealed; code is verbatim.
    pub fn views(&self, source: &str, selection: Selection) -> Vec<View> {
        let sel = selection.anchor.min(selection.head)..selection.anchor.max(selection.head);
        self.leaves
            .iter()
            .map(|leaf| {
                let text = &source[leaf.source.clone()];
                if !leaf.conceals {
                    return View::Verbatim(text.to_string());
                }
                let touches = sel.start <= leaf.source.end && leaf.source.start <= sel.end;
                let local = if touches {
                    let rel =
                        |o: usize| o.clamp(leaf.source.start, leaf.source.end) - leaf.source.start;
                    Selection {
                        anchor: rel(selection.anchor),
                        head: rel(selection.head),
                    }
                } else {
                    NOTHING_REVEALED
                };
                View::Concealed(layout(text, local))
            })
            .collect()
    }
}

/// What one leaf shows: its text with syntax concealed, or verbatim (code).
/// Offsets are relative to the leaf.
#[derive(Debug)]
pub enum View {
    Concealed(Layout),
    Verbatim(String),
}

impl View {
    /// The text the leaf shows.
    pub fn display_text(&self) -> &str {
        match self {
            View::Concealed(l) => l.display_text(),
            View::Verbatim(t) => t,
        }
    }

    /// Styled runs over the display text.
    pub fn runs(&self) -> &[Run] {
        match self {
            View::Concealed(l) => l.runs(),
            View::Verbatim(_) => &[],
        }
    }

    /// The display offset for a leaf-relative source offset.
    pub fn to_display(&self, source: usize) -> usize {
        match self {
            View::Concealed(l) => l.to_display(source),
            View::Verbatim(t) => source.min(t.len()),
        }
    }

    /// The leaf-relative source offset for a display offset.
    pub fn to_source(&self, display: usize) -> usize {
        match self {
            View::Concealed(l) => l.to_source(display),
            View::Verbatim(t) => display.min(t.len()),
        }
    }
}

/// The grapheme boundary after `offset`.
fn next_grapheme(source: &str, offset: usize) -> usize {
    source[offset..]
        .grapheme_indices(true)
        .nth(1)
        .map_or(source.len(), |(i, _)| offset + i)
}

/// The grapheme boundary before `offset`.
fn prev_grapheme(source: &str, offset: usize) -> usize {
    source[..offset]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}
