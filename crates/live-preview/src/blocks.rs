//! A note split into blocks, and each block's editable text as one or more
//! *leaves*, grown from Spike 4's model. A leaf is a source range the
//! editor renders as one keyed text leaf. Source outside every leaf (the
//! blank line between blocks) is never under the caret.
//!
//! Paragraphs, headings, list items, quotes and code blocks render; every
//! other block (tables, rules, front matter) shows its source as typed,
//! nothing concealed, until its own Live Preview ticket gives it a widget.
//! Each list item and quote line is a block of its own, its leaf starting
//! with the line's marker (`-`, `1.`, `- [ ]`, `>`); a code block's leaf
//! starts with its opening fence, the closing fence left out (see
//! [`Shows`]).

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
    /// One line of a list: its leaf starts at the marker, the indentation
    /// before it left out (drawn as indent, never as text).
    ListItem(Item),
    /// One line of a quote, nested this deep (`> >` is 2).
    Quote(usize),
    /// One line of a callout: a quote whose first line is `[!type]`. A
    /// quote nested in it (`> >`) is drawn as one of its lines, its depth
    /// not shown.
    Callout(CalloutLine),
    /// A fenced code block.
    Code(Code),
    /// A `$$` math block: its leaf all of it, typeset unless the caret is in
    /// it.
    Math,
    /// A table: a leaf per cell, row by row; its delimiter row has none.
    Table(Table),
    /// A block shown as its source, verbatim: the leaf is all of it.
    Source,
}

/// A list item's shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// How deep it nests in its list: 0 at the list's left edge.
    pub depth: usize,
    /// An ordered item's number, as written.
    pub number: Option<u32>,
    /// A task's checkbox: whether it's ticked.
    pub task: Option<bool>,
}

/// A callout line's shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalloutLine {
    /// How the callout looks.
    pub kind: md_engine::CalloutKind,
    /// It's the callout's first line, its title.
    pub title: bool,
}

/// A fenced code block's shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Code {
    /// The language named after the opening fence, if one is.
    pub lang: Option<String>,
    /// Its closing fence's line, as written (trimmed); without one it runs
    /// to the note's end.
    pub close: Option<String>,
}

/// A table's shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// Each column's alignment: as many as the header row has cells.
    pub align: Vec<Align>,
    /// How many cells (leaves) each row has, the header first. A long row
    /// is cut to the header's width; a short one has fewer, the rest
    /// drawn empty, where the caret can't stand.
    pub rows: Vec<usize>,
}

/// How a table column's text is aligned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// `---`: as the renderer has it, left.
    None,
    /// `:--`.
    Left,
    /// `:-:`.
    Center,
    /// `--:`.
    Right,
}

impl Table {
    /// Each row's cells, as leaf indices, header first, for a table whose
    /// cells are the leaves `leaves`.
    pub fn row_leaves(&self, leaves: Range<usize>) -> Vec<Range<usize>> {
        let mut at = leaves.start;
        self.rows
            .iter()
            .map(|&count| {
                at += count;
                at - count..at
            })
            .collect()
    }
}

/// A list level open while the items below are parsed: where its last
/// item's marker and text start, in columns.
struct Level {
    indent: usize,
    content: usize,
}

impl Kind {
    /// Whether a block of this kind runs straight on into a `next` one, with
    /// no gap: a list's items, a quote's lines, a callout's.
    pub fn runs_into(&self, next: &Kind) -> bool {
        matches!(
            (self, next),
            (Kind::ListItem(_), Kind::ListItem(_))
                | (Kind::Quote(_), Kind::Quote(_))
                | (
                    Kind::Callout(_),
                    Kind::Callout(CalloutLine { title: false, .. })
                )
        )
    }
}

/// One block and the leaves it owns (indices into [`Blocks::leaves`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// What it renders as.
    pub kind: Kind,
    /// Its leaves.
    pub leaves: Range<usize>,
}

/// A keyed text leaf: where its text is in the source, and how it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    /// The leaf's text, as a source range.
    pub source: Range<usize>,
    /// What of it is concealed.
    pub shows: Shows,
}

/// How a leaf shows: what of it is concealed while the caret isn't in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shows {
    /// Prose: its inline syntax, and its first `marker` bytes, a list
    /// item's or quote's line marker (0 for other prose).
    Prose { marker: usize },
    /// A callout's title line: its inline syntax, and its first `marker`
    /// bytes (its `>` and `[!type]`), `title` shown in their place when it
    /// has no title of its own.
    Title {
        marker: usize,
        title: Option<String>,
    },
    /// Code, as typed: only its first `fence` bytes, the opening fence's
    /// line.
    Code { fence: usize },
    /// A math block: all of it, as one object to typeset.
    Math,
    /// Nothing: a [`Kind::Source`] block.
    Source,
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

/// A code block's opening fence, parsed.
pub(crate) struct Fence {
    /// The fence's mark, `` ` `` or `~`.
    mark: u8,
    /// How many marks it has: a closing fence has at least as many.
    len: usize,
    /// Where the language is (or would go) on the line: the first word
    /// after the marks, empty if there's none.
    pub(crate) lang: Range<usize>,
}

/// An opening fence: up to 3 spaces, then three or more `` ` `` or `~`,
/// then the info string (no backtick in it after backticks), as in
/// CommonMark.
pub(crate) fn opening_fence(text: &str) -> Option<Fence> {
    let indent = text.bytes().take_while(|&b| b == b' ').count();
    let mark = *text
        .as_bytes()
        .get(indent)
        .filter(|&&b| b == b'`' || b == b'~')?;
    let len = text[indent..].bytes().take_while(|&b| b == mark).count();
    let info = &text[indent + len..];
    if indent > 3 || len < 3 || (mark == b'`' && info.contains('`')) {
        return None;
    }
    let start = indent + len + (info.len() - info.trim_start().len());
    let word = text[start..]
        .find(char::is_whitespace)
        .unwrap_or(text.len() - start);
    Some(Fence {
        mark,
        len,
        lang: start..start + word,
    })
}

impl Fence {
    /// Whether `text` closes this fence: up to 3 spaces, as many of its
    /// marks or more, then nothing but spaces.
    fn closed_by(&self, text: &str) -> bool {
        let indent = text.bytes().take_while(|&b| b == b' ').count();
        let marks = text[indent..]
            .bytes()
            .take_while(|&b| b == self.mark)
            .count();
        indent <= 3 && marks >= self.len && text[indent + marks..].trim().is_empty()
    }
}

/// A code fence's opening line.
fn is_fence(text: &str) -> bool {
    opening_fence(text).is_some()
}

/// The `$$` math block starting at line `i`, if one does: the index of the
/// line after its last. It starts with `$$` (indented 3 spaces at most) and
/// ends on a line that ends with `$$`, its first or one before a blank
/// line; with no such line it's prose.
fn math_block_end(lines: &[Line], i: usize) -> Option<usize> {
    let first = lines[i].text;
    let indent = first.bytes().take_while(|&b| b == b' ').count();
    let text = first.trim();
    if indent > 3 || !text.starts_with("$$") {
        return None;
    }
    if text.len() >= 4 && text.ends_with("$$") {
        return Some(i + 1);
    }
    (i + 1..lines.len())
        .take_while(|&j| !is_blank(lines[j].text))
        .find(|&j| lines[j].text.trim().ends_with("$$"))
        .map(|j| j + 1)
}

/// A table row: a line that starts with a pipe.
pub(crate) fn is_table_row(text: &str) -> bool {
    text.trim_start().starts_with('|')
}

/// A table's delimiter row, such as `|---|:--:|`.
fn is_delimiter_row(text: &str) -> bool {
    text.contains('-')
        && text
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// The cells of a table row, as ranges of `text`: each cell's text inside
/// one padding space each side (any more space, one just typed at a cell's
/// end, is the cell's). An empty cell is an empty range just inside its
/// padding, so the caret can stand in it. An escaped `\|` is in a cell; a
/// row needn't end with a pipe.
pub(crate) fn cells(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    // A pipe after an odd run of backslashes is escaped.
    let escaped = |i: usize| bytes[..i].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1;
    // Where each cell starts and ends: just past a pipe and at the next,
    // a row with no pipe first or last having one there all the same.
    let pipes: Vec<usize> = (0..bytes.len())
        .filter(|&i| bytes[i] == b'|' && !escaped(i))
        .collect();
    let first = text.len() - text.trim_start().len();
    let end = text.trim_end().len();
    let mut starts: Vec<usize> = pipes.iter().map(|&p| p + 1).collect();
    let mut ends = pipes.clone();
    if pipes.first() != Some(&first) {
        starts.insert(0, first);
    } else {
        ends.remove(0);
    }
    if pipes.last().is_none_or(|&p| p + 1 != end) {
        ends.push(end);
    } else {
        starts.pop();
    }
    starts
        .into_iter()
        .zip(ends)
        .filter(|(start, end)| start <= end)
        .map(|(start, end)| {
            let inner = &text[start..end];
            let lead = usize::from(inner.starts_with(' '));
            let trail = usize::from(inner.len() > lead && inner.ends_with(' '));
            start + lead..end - trail
        })
        .collect()
}

/// How many cells a table row has.
fn cell_count(text: &str) -> usize {
    cells(text).len()
}

/// A delimiter row's column alignments: `:--` left, `--:` right, `:-:`
/// centre.
fn alignments(text: &str) -> Vec<Align> {
    cells(text)
        .into_iter()
        .map(|cell| {
            let cell = text[cell].trim();
            match (cell.starts_with(':'), cell.ends_with(':')) {
                (true, true) => Align::Center,
                (true, false) => Align::Left,
                (false, true) => Align::Right,
                (false, false) => Align::None,
            }
        })
        .collect()
}

/// A table starts here: a row, then a delimiter row with a cell per
/// header cell (as in GFM, so a half-typed `|-` isn't one yet).
fn starts_table(line: &str, next: Option<&str>) -> bool {
    is_table_row(line)
        && cell_count(line) > 0
        && next.is_some_and(|n| is_delimiter_row(n) && cell_count(n) == cell_count(line))
}

/// A list item's line, parsed.
struct ListLine {
    /// The bytes of indentation before the marker.
    indent: usize,
    /// The indentation's width, a tab counting 4.
    columns: usize,
    /// The column the item's text starts at, past the bullet or number and
    /// its space: an item indented this far nests under it.
    content: usize,
    /// The marker's bytes, from the bullet or number to the item's text:
    /// `- `, `12. `, `- [x] `.
    marker: usize,
    /// An ordered item's number.
    number: Option<u32>,
    /// A task's checkbox: whether it's ticked.
    task: Option<bool>,
}

/// The most digits an ordered item's number has, as in CommonMark.
const MAX_DIGITS: usize = 9;

/// A list item's line: a bullet (`-`, `*`, `+`) or a number and `.` or
/// `)`, then a space; then maybe a task's `[ ]` or `[x]`.
fn list_line(text: &str) -> Option<ListLine> {
    let rest = text.trim_start_matches([' ', '\t']);
    let indent = text.len() - rest.len();
    let columns = text[..indent]
        .chars()
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum();
    let (number, after) = match ["- ", "* ", "+ "].iter().find_map(|m| rest.strip_prefix(m)) {
        Some(after) => (None, after),
        None => {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if !(1..=MAX_DIGITS).contains(&digits) {
                return None;
            }
            let after = [". ", ") "]
                .iter()
                .find_map(|m| rest[digits..].strip_prefix(m))?;
            (Some(rest[..digits].parse().ok()?), after)
        }
    };
    let content = columns + (rest.len() - after.len());
    let (task, after) = match after.get(..3) {
        Some(b @ ("[ ]" | "[x]" | "[X]")) if after.len() == 3 || after[3..].starts_with(' ') => {
            (Some(b != "[ ]"), &after[(after.len()).min(4)..])
        }
        _ => (None, after),
    };
    Some(ListLine {
        indent,
        columns,
        content,
        marker: rest.len() - after.len(),
        number,
        task,
    })
}

/// A list item's line.
fn is_list_item(text: &str) -> bool {
    list_line(text).is_some()
}

/// A quote's line, parsed.
pub(crate) struct QuoteLine {
    /// The marker's bytes: up to 3 spaces, then each `>` and the space
    /// after it.
    pub(crate) marker: usize,
    /// How deep it nests: `> >` is 2.
    depth: usize,
}

/// A quote's line.
pub(crate) fn quote_line(text: &str) -> Option<QuoteLine> {
    let spaces = text.bytes().take_while(|&b| b == b' ').count();
    if spaces > 3 || !text[spaces..].starts_with('>') {
        return None;
    }
    let (mut at, mut depth) = (spaces, 0);
    while text[at..].starts_with('>') {
        at += 1;
        depth += 1;
        if text[at..].starts_with(' ') {
            at += 1;
        }
    }
    Some(QuoteLine { marker: at, depth })
}

/// A callout's title line, parsed.
struct CalloutTitle {
    /// How it looks.
    kind: md_engine::CalloutKind,
    /// The marker's bytes: the `>`, `[!type]`, a fold sign and the spaces
    /// before the title.
    marker: usize,
    /// What it shows if it has no title: its type's name.
    default: Option<String>,
}

/// A callout's first line: `> [!type]`, maybe `-` or `+`, then its title.
fn callout_title(text: &str) -> Option<CalloutTitle> {
    let quote = quote_line(text).filter(|q| q.depth == 1)?;
    let callout = md_engine::callout(text)?;
    let rest = &text[quote.marker..];
    let close = rest.find(']')? + 1;
    let after = &rest[close..];
    let after = after.strip_prefix(['-', '+']).unwrap_or(after);
    let spaces = after.len() - after.trim_start().len();
    let marker = text.len() - after.len() + spaces;
    let untitled = text[marker..].trim().is_empty();
    Some(CalloutTitle {
        kind: callout.kind,
        marker,
        default: untitled.then_some(callout.title),
    })
}

/// A quote's line.
fn is_quote(text: &str) -> bool {
    quote_line(text).is_some()
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
        // Front matter, from a `---` first line to the next `---`, isn't
        // shown: the properties panel edits it. With no leaf, no caret
        // stands in it.
        let front = md_engine::front_matter(source).len();
        if front > 0 {
            i = lines
                .iter()
                .position(|l| l.start >= front)
                .unwrap_or(lines.len());
        }
        // A blank line right after a block only separates it from the next.
        let mut after_block = i > 0;
        // The list levels open: blank lines between items keep them, any
        // other block closes them.
        let mut levels = Vec::new();
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
                    doc.push(Kind::Empty, line.start..line.start);
                }
                after_block = false;
                i += run;
                continue;
            }
            after_block = true;
            if !is_list_item(line.text) {
                levels.clear();
            }
            let next = lines.get(i + 1).map(|l| l.text);
            // Where a block that runs while `more` holds ends.
            let run_while = |more: &dyn Fn(&str) -> bool| {
                (i..lines.len())
                    .find(|&j| !more(lines[j].text))
                    .unwrap_or(lines.len())
            };
            if let Some(level) = heading_level(line.text) {
                doc.push(Kind::Heading(level), line.start..line.end());
                i += 1;
            } else if let Some(end) = math_block_end(&lines, i) {
                let source = line.start..lines[end - 1].end();
                doc.push_leaf(
                    Kind::Math,
                    Leaf {
                        source,
                        shows: Shows::Math,
                    },
                );
                i = end;
            } else if let Some(fence) = opening_fence(line.text) {
                let close_ix = (i + 1..lines.len()).find(|&j| fence.closed_by(lines[j].text));
                let close = close_ix.map(|c| lines[c].text.trim().to_string());
                doc.push_code(&lines[i..close_ix.unwrap_or(lines.len())], &fence, close);
                i = close_ix.map_or(lines.len(), |c| c + 1);
            } else if is_rule(line.text) {
                doc.push_source(&lines[i..i + 1]);
                i += 1;
            } else if starts_table(line.text, next) {
                // Its header and delimiter rows, then the rows after.
                let end = (i + 2..lines.len())
                    .find(|&j| !is_table_row(lines[j].text))
                    .unwrap_or(lines.len());
                doc.push_table(&lines[i..end]);
                i = end;
            } else if is_list_item(line.text) {
                let end = run_while(&is_list_item);
                doc.push_list(&lines[i..end], &mut levels);
                i = end;
            } else if let Some(callout) =
                callout_title(line.text).filter(|_| i == 0 || !is_quote(lines[i - 1].text))
            {
                let end = run_while(&is_quote);
                doc.push_callout(&lines[i..end], callout);
                i = end;
            } else if let Some(quote) = quote_line(line.text) {
                let kind = Kind::Quote(quote.depth);
                doc.push_line(kind, line.start..line.end(), quote.marker);
                i += 1;
            } else {
                let end = (i + 1..lines.len())
                    .find(|&j| {
                        let next = lines.get(j + 1).map(|l| l.text);
                        is_blank(lines[j].text)
                            || starts_block(lines[j].text)
                            || math_block_end(&lines, j).is_some()
                            || starts_table(lines[j].text, next)
                    })
                    .unwrap_or(lines.len());
                doc.push(Kind::Paragraph, line.start..lines[end - 1].end());
                i = end;
            }
        }
        // The note's end is always somewhere to stand.
        if doc.leaf_at(source.len()).is_none() {
            doc.push(Kind::Empty, source.len()..source.len());
        }
        doc
    }

    /// Adds a block of prose with one leaf.
    fn push(&mut self, kind: Kind, source: Range<usize>) {
        let shows = Shows::Prose { marker: 0 };
        self.push_leaf(kind, Leaf { source, shows });
    }

    /// Adds a block of one line whose first `marker` bytes are its list or
    /// quote marker.
    fn push_line(&mut self, kind: Kind, source: Range<usize>, marker: usize) {
        let shows = Shows::Prose { marker };
        self.push_leaf(kind, Leaf { source, shows });
    }

    /// Adds a code block: its opening fence's line and the code after it
    /// (`lines`), a leaf; its closing fence's line, `close` if it has one,
    /// is left out, so the caret never stands on it.
    fn push_code(&mut self, lines: &[Line], fence: &Fence, close: Option<String>) {
        let (Some(open), Some(last)) = (lines.first(), lines.last()) else {
            return;
        };
        let lang = &open.text[fence.lang.clone()];
        let kind = Kind::Code(Code {
            lang: (!lang.is_empty()).then(|| lang.to_string()),
            close,
        });
        // The fence's line break goes with it, when code follows.
        let fence = open.text.len() + usize::from(lines.len() > 1);
        let shows = Shows::Code { fence };
        self.push_leaf(
            kind,
            Leaf {
                source: open.start..last.end(),
                shows,
            },
        );
    }

    /// Adds a block of one leaf.
    fn push_leaf(&mut self, kind: Kind, leaf: Leaf) {
        let first = self.leaves.len();
        self.leaves.push(leaf);
        self.blocks.push(Block {
            kind,
            leaves: first..first + 1,
        });
    }

    /// Adds a list's lines, an item each, under the `levels` open. An item
    /// indented as far as the text of the one above nests under it; one
    /// indented less goes back out to the level it's within.
    fn push_list(&mut self, lines: &[Line], levels: &mut Vec<Level>) {
        for (line, item) in lines.iter().filter_map(|l| Some((l, list_line(l.text)?))) {
            // Out of each level the item doesn't reach the text of; a
            // level it starts within is its own, and its last item's.
            while let Some(level) = levels.last()
                && item.columns < level.content
            {
                let within = item.columns >= level.indent;
                levels.pop();
                if within {
                    break;
                }
            }
            levels.push(Level {
                indent: item.columns,
                content: item.content,
            });
            let kind = Kind::ListItem(Item {
                depth: levels.len() - 1,
                number: item.number,
                task: item.task,
            });
            let start = line.start + item.indent;
            self.push_line(kind, start..line.end(), item.marker);
        }
    }

    /// Adds a callout: its title line and the quote lines after it, a
    /// block each.
    fn push_callout(&mut self, lines: &[Line], title: CalloutTitle) {
        let Some((first, body)) = lines.split_first() else {
            return;
        };
        let kind = title.kind;
        self.push_leaf(
            Kind::Callout(CalloutLine { kind, title: true }),
            Leaf {
                source: first.start..first.end(),
                shows: Shows::Title {
                    marker: title.marker,
                    title: title.default,
                },
            },
        );
        for line in body {
            let marker = quote_line(line.text).map_or(0, |q| q.marker);
            let kind = Kind::Callout(CalloutLine { kind, title: false });
            self.push_line(kind, line.start..line.end(), marker);
        }
    }

    /// The source each leaf's syntax shows for a selection touching:
    /// itself, or a callout's line the whole callout.
    pub fn reveal_ranges(&self) -> Vec<Range<usize>> {
        let mut ranges: Vec<Range<usize>> = self.leaves.iter().map(|l| l.source.clone()).collect();
        let mut b = 0;
        while b < self.blocks.len() {
            let Kind::Callout(CalloutLine { title: true, .. }) = self.blocks[b].kind else {
                b += 1;
                continue;
            };
            let end = (b + 1..self.blocks.len())
                .find(|&e| {
                    !matches!(
                        self.blocks[e].kind,
                        Kind::Callout(CalloutLine { title: false, .. })
                    )
                })
                .unwrap_or(self.blocks.len());
            let first = self.blocks[b].leaves.start;
            let last = self.blocks[end - 1].leaves.end - 1;
            let span = self.leaves[first].source.start..self.leaves[last].source.end;
            for range in &mut ranges[first..=last] {
                *range = span.clone();
            }
            b = end;
        }
        ranges
    }

    /// Adds a table: its header row, delimiter row, then rows (`lines`).
    fn push_table(&mut self, lines: &[Line]) {
        let Some(delimiter) = lines.get(1) else {
            return;
        };
        let align = alignments(delimiter.text);
        let columns = align.len();
        let rows: Vec<&Line> = lines.iter().take(1).chain(lines.iter().skip(2)).collect();
        let first = self.leaves.len();
        let mut counts = Vec::with_capacity(rows.len());
        for row in rows {
            let cells: Vec<Range<usize>> = cells(row.text).into_iter().take(columns).collect();
            counts.push(cells.len());
            self.leaves.extend(cells.into_iter().map(|cell| Leaf {
                source: row.start + cell.start..row.start + cell.end,
                shows: Shows::Prose { marker: 0 },
            }));
        }
        self.blocks.push(Block {
            kind: Kind::Table(Table {
                align,
                rows: counts,
            }),
            leaves: first..self.leaves.len(),
        });
    }

    /// Adds a block shown as its source: these lines, verbatim.
    fn push_source(&mut self, lines: &[Line]) {
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
            return;
        };
        let shows = Shows::Source;
        self.push_leaf(
            Kind::Source,
            Leaf {
                source: first.start..last.end(),
                shows,
            },
        );
    }

    /// The source offset block `ix` starts at: its first leaf's start.
    /// (Every block has a leaf.)
    pub fn block_start(&self, ix: usize) -> Option<usize> {
        let block = self.blocks.get(ix)?;
        Some(self.leaves[block.leaves.start].source.start)
    }

    /// The block holding source offset `offset`: the last to start at or
    /// before it.
    pub fn block_at(&self, offset: usize) -> usize {
        let after = self
            .blocks
            .partition_point(|b| self.leaves[b.leaves.start].source.start <= offset);
        after.saturating_sub(1)
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
