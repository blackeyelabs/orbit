//! The conceal model, moved from Spike 1 (`spikes/spike1-conceal`).
//!
//! Given one leaf's source text and the selection, [`layout`] decides which
//! markdown syntax is hidden (syntax shows only where the caret or
//! selection touches its span), maps offsets between source and display
//! text, and reports styled runs and links over the display text.
//!
//! Inline syntax, as Obsidian's Live Preview conceals it: emphasis
//! (nested), strike, highlights, code spans, escapes, wikilinks (aliases
//! and headings), markdown links, autolinks and bare web addresses.
//! Nothing inside code is concealed. Tags show as typed. Embeds, images
//! and math are left as text for their own tickets. Emphasis doesn't
//! cross a line break. A heading's style run can overlap the inline runs
//! inside it; the renderer merges them.

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

/// The caret or selection, as byte offsets into the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Where the selection started; it stays put as the selection grows.
    pub anchor: usize,
    /// The moving end, where the caret is drawn.
    pub head: usize,
}

impl Selection {
    /// A caret at `offset`: nothing selected.
    pub fn caret(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    /// The selected range, start first.
    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    /// Selects `range`, the caret at its end.
    pub fn spanning(range: Range<usize>) -> Self {
        Self {
            anchor: range.start,
            head: range.end,
        }
    }

    /// The reveal rule: touching a span's boundary counts.
    pub fn touches(&self, span: &Range<usize>) -> bool {
        let sel = self.range();
        sel.start <= span.end && span.start <= sel.end
    }
}

/// How a run of display text is styled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// `**bold**` or `__bold__`.
    Bold,
    /// `*italic*` or `_italic_`.
    Italic,
    /// `~~struck~~`.
    Strike,
    /// `==highlighted==`.
    Highlight,
    /// `` `code` ``.
    Code,
    /// A markdown link's or wikilink's text, or a web address.
    Link,
    /// A heading of this level, its whole line.
    Heading(u8),
    /// Markdown syntax that is showing because the caret revealed it.
    Syntax,
}

/// A styled range of the display text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// Display offsets.
    pub range: Range<usize>,
    /// How the run is styled.
    pub style: Style,
}

/// Where a link goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A note or attachment in the vault, maybe with a `#heading`: a wikilink's
    /// target, or a markdown link's destination, percent-decoded.
    Note(String),
    /// A web address (anything with a scheme, `mailto:` too).
    Url(String),
}

/// A link in the display text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Its text, in display offsets.
    pub range: Range<usize>,
    /// Where it goes.
    pub target: Target,
    /// A click follows it: its syntax is hidden (or it has none, as a bare
    /// web address). A revealed link is being edited, so a click there
    /// places the caret.
    pub followable: bool,
}

/// Source syntax hidden from the display, which shows `shown` in its place
/// (usually nothing; ` > ` for the `#` in `[[Note#Heading]]`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hide {
    range: Range<usize>,
    shown: Cow<'static, str>,
}

impl Hide {
    /// Hidden, with nothing in its place.
    fn new(range: Range<usize>) -> Self {
        Hide {
            range,
            shown: Cow::Borrowed(""),
        }
    }

    /// How much longer the source is than the display, across it.
    fn shrink(&self) -> isize {
        self.range.len() as isize - self.shown.len() as isize
    }
}

/// The character an image stands as in the display text: an object, as Kit
/// draws an inline image.
pub const OBJECT: &str = "\u{FFFC}";

/// An image in the display text: `![[pic.png]]` or `![alt](pic.png)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Where its [`OBJECT`] is, in display offsets.
    pub at: usize,
    /// The image: an attachment in the vault, or a web address.
    pub target: Target,
    /// What it shows as text: a markdown image's text, or an embed's
    /// alias when that isn't a size.
    pub alt: String,
    /// Its width in pixels, from an embed's `|300` or `|300x200`.
    pub width: Option<u32>,
    /// Its height in pixels, from an embed's `|300x200`.
    pub height: Option<u32>,
}

/// Math in the display text, to typeset: inline `$…$`, or a `$$…$$` block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Math {
    /// Where its [`OBJECT`] is, in display offsets.
    pub at: usize,
    /// Its LaTeX, without the dollars.
    pub latex: String,
    /// It's a block (`$$`), set on its own line; else it sits in a line.
    pub display: bool,
}

/// What the editor shows for one layout pass.
#[derive(Debug)]
pub struct Layout {
    display: String,
    source_len: usize,
    /// Hidden source, sorted and non-overlapping.
    hidden: Vec<Hide>,
    runs: Vec<Run>,
    links: Vec<Link>,
    images: Vec<Image>,
    maths: Vec<Math>,
}

impl Layout {
    /// The text shown: the source with concealed syntax left out.
    pub fn display_text(&self) -> &str {
        &self.display
    }

    /// Styled runs over the display text, sorted by start.
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// The links in the text, in order.
    pub fn links(&self) -> &[Link] {
        &self.links
    }

    /// The images in the text, in order.
    pub fn images(&self) -> &[Image] {
        &self.images
    }

    /// The math in the text, in order.
    pub fn maths(&self) -> &[Math] {
        &self.maths
    }

    /// Display offset for a source offset. An offset inside hidden syntax
    /// collapses to the display position where that syntax would be.
    pub fn to_display(&self, source: usize) -> usize {
        let mut shift = 0;
        for h in &self.hidden {
            if source <= h.range.start {
                break;
            }
            if source < h.range.end {
                return (h.range.start as isize - shift) as usize;
            }
            shift += h.shrink();
        }
        (source as isize - shift) as usize
    }

    /// Source offset for a display offset: the start of the next visible
    /// source character (or the end of the source). Inside text shown in
    /// place of hidden syntax, that's the end of the syntax.
    pub fn to_source(&self, display: usize) -> usize {
        let display = display as isize;
        let mut shift = 0;
        for h in &self.hidden {
            let start = h.range.start as isize - shift;
            if display < start {
                break;
            }
            if display < start + h.shown.len() as isize {
                return h.range.end;
            }
            shift += h.shrink();
        }
        ((display + shift).max(0) as usize).min(self.source_len)
    }
}

/// Lays out `source` for `selection`: what's hidden, and how the rest is
/// styled.
pub fn layout(source: &str, selection: Selection) -> Layout {
    layout_line(source, 0, selection)
}

/// Lays out a code block's leaf: its code as typed, nothing parsed, and its
/// first `fence` bytes (the opening fence's line) hidden unless the
/// selection touches the leaf.
pub fn layout_code(source: &str, fence: usize, selection: Selection) -> Layout {
    let shown_from = if selection.touches(&(0..source.len())) {
        0
    } else {
        fence
    };
    let hidden = (shown_from > 0)
        .then(|| Hide::new(0..fence))
        .into_iter()
        .collect();
    Layout {
        display: source[shown_from..].to_string(),
        source_len: source.len(),
        hidden,
        runs: Vec::new(),
        links: Vec::new(),
        images: Vec::new(),
        maths: Vec::new(),
    }
}

/// Lays out a list item's or quote's line, whose first `marker` bytes are
/// its marker: hidden, like a heading's `#`s, unless the selection touches
/// the line.
pub fn layout_line(source: &str, marker: usize, selection: Selection) -> Layout {
    layout_titled(source, marker, None, selection)
}

/// Lays out a callout's title line, whose first `marker` bytes (its
/// quote marker and `[!type]`) are hidden unless the selection touches the
/// line, `title` shown in their place if it has none of its own.
pub fn layout_titled(
    source: &str,
    marker: usize,
    title: Option<&str>,
    selection: Selection,
) -> Layout {
    let spans: Vec<(Span, bool)> = parse(source, marker, title)
        .into_iter()
        .map(|s| {
            let revealed = selection.touches(&s.range);
            (s, revealed)
        })
        .collect();

    let mut hidden: Vec<Hide> = spans
        .iter()
        .filter(|(_, revealed)| !revealed)
        .flat_map(|(s, _)| s.hidden.iter().cloned())
        .collect();
    // Headings, outer and nested spans are found apart.
    hidden.sort_by_key(|h| h.range.start);

    let mut display = String::with_capacity(source.len());
    let mut at = 0;
    for h in &hidden {
        display.push_str(&source[at..h.range.start]);
        display.push_str(&h.shown);
        at = h.range.end;
    }
    display.push_str(&source[at..]);

    let mut layout = Layout {
        display,
        source_len: source.len(),
        hidden,
        runs: Vec::new(),
        links: Vec::new(),
        images: Vec::new(),
        maths: Vec::new(),
    };
    let to_display =
        |layout: &Layout, r: &Range<usize>| layout.to_display(r.start)..layout.to_display(r.end);
    let mut runs = Vec::new();
    let mut links = Vec::new();
    let mut images = Vec::new();
    let mut maths = Vec::new();
    for (span, revealed) in &spans {
        if let (Some(latex), false) = (&span.math, *revealed) {
            maths.push(Math {
                at: layout.to_display(span.range.start),
                latex: latex.clone(),
                display: false,
            });
        }
        if let (Some(image), false) = (&span.image, *revealed) {
            images.push(Image {
                at: layout.to_display(span.range.start),
                ..image.clone()
            });
        }
        if *revealed {
            runs.extend(span.hidden.iter().map(|h| Run {
                range: to_display(&layout, &h.range),
                style: Style::Syntax,
            }));
        }
        if let Some(style) = span.style {
            runs.push(Run {
                range: to_display(&layout, &span.content),
                style,
            });
        }
        if let Some(target) = &span.target {
            links.push(Link {
                range: to_display(&layout, &span.content),
                target: target.clone(),
                followable: !revealed || span.hidden.is_empty(),
            });
        }
    }
    runs.retain(|r| !r.range.is_empty());
    runs.sort_by_key(|r| r.range.start);
    links.sort_by_key(|l| l.range.start);
    layout.runs = runs;
    layout.links = links;
    layout.images = images;
    layout.maths = maths;
    layout
}

/// Lays out a `$$` math block's leaf: one [`OBJECT`] for its math unless
/// the selection touches it, when its source shows as typed.
pub fn layout_math(source: &str, selection: Selection) -> Layout {
    let mut layout = layout_code(source, 0, selection);
    if selection.touches(&(0..source.len())) {
        return layout;
    }
    let latex = source.trim().strip_prefix("$$").unwrap_or(source.trim());
    let latex = latex.strip_suffix("$$").unwrap_or(latex).trim();
    layout.display = OBJECT.to_string();
    layout.hidden = vec![Hide {
        range: 0..source.len(),
        shown: OBJECT.into(),
    }];
    layout.maths = vec![Math {
        at: 0,
        latex: latex.to_string(),
        display: true,
    }];
    layout
}

/// A piece of markdown syntax: its whole extent, the parts of it that are
/// pure syntax (hidden unless revealed), its visible content, and how
/// that's styled.
#[derive(Clone, Debug)]
struct Span {
    range: Range<usize>,
    hidden: Vec<Hide>,
    content: Range<usize>,
    style: Option<Style>,
    target: Option<Target>,
    /// It's an image, drawn in place of its source (its `at` unset).
    image: Option<Image>,
    /// It's inline math, this LaTeX, typeset in place of its source.
    math: Option<String>,
    /// Inline syntax inside the content is parsed too (emphasis, a
    /// markdown link's text); code and wikilinks are taken as written.
    nests: bool,
}

impl Span {
    /// A span of `range` showing `content` styled `style`, its syntax
    /// `hidden`: nothing inside it is parsed, and it goes nowhere.
    fn new(
        range: Range<usize>,
        hidden: Vec<Hide>,
        content: Range<usize>,
        style: Option<Style>,
    ) -> Self {
        Span {
            range,
            hidden,
            content,
            style,
            target: None,
            image: None,
            math: None,
            nests: false,
        }
    }

    /// A span of `range` whose syntax is `open` bytes at its start and
    /// `close` at its end, styled `style`.
    fn paired(range: Range<usize>, open: usize, close: usize, style: Style) -> Self {
        let content = range.start + open..range.end - close;
        let hidden = vec![
            Hide::new(range.start..content.start),
            Hide::new(content.end..range.end),
        ];
        Span::new(range, hidden, content, Some(style))
    }

    /// Source shown as typed, nothing inside it parsed: an embed or image
    /// (until their own ticket), or a run of backticks nothing closes.
    fn literal(range: Range<usize>) -> Self {
        Span::new(range.clone(), Vec::new(), range, None)
    }

    /// Its content is parsed for more inline syntax.
    fn nesting(self) -> Self {
        Span {
            nests: true,
            ..self
        }
    }

    /// It's a link to `target`.
    fn to(self, target: Target) -> Self {
        Span {
            target: Some(target),
            ..self
        }
    }
}

/// Emphasis delimiters, longer before their prefixes (`**` before `*`).
const PAIRS: &[(&str, Style)] = &[
    ("**", Style::Bold),
    ("__", Style::Bold),
    ("~~", Style::Strike),
    ("==", Style::Highlight),
    ("*", Style::Italic),
    ("_", Style::Italic),
];

/// Every span of syntax in `source`: headings (or the line's `marker`),
/// then inline spans after it left to right, each followed by the spans
/// nested in it.
fn parse(source: &str, marker: usize, title: Option<&str>) -> Vec<Span> {
    let mut spans = if marker == 0 {
        headings(source)
    } else {
        let line = 0..source.len();
        let hide = Hide {
            range: 0..marker,
            shown: title.map_or(Cow::Borrowed(""), |t| Cow::Owned(t.to_string())),
        };
        vec![Span::new(line, vec![hide], marker..source.len(), None)]
    };
    let parser = Parser {
        source,
        found: RefCell::new(vec![None; source.len() + 1]),
        depth: Cell::new(0),
    };
    parser.inline(marker..source.len(), &mut spans);
    spans
}

/// How deep spans are looked for inside the content of others: a search
/// for a close past it steps over characters, as a run of openers would
/// otherwise nest as deep as it's long.
const MAX_DEPTH: usize = 16;

/// The inline parser over one leaf. The span starting at each offset is
/// worked out once: a search for a closing delimiter skips the spans
/// inside, which would otherwise be parsed again and again.
struct Parser<'a> {
    source: &'a str,
    /// By offset: not looked for yet, or the span there (if any).
    found: RefCell<Vec<Option<Option<Rc<Span>>>>>,
    /// How many searches for a close are under way. A span worked out at
    /// the limit is cached as found there, which only tells in nesting
    /// deeper than anyone writes.
    depth: Cell<usize>,
}

impl Parser<'_> {
    /// The spans in `range`, and those nested in them, in order.
    fn inline(&self, range: Range<usize>, out: &mut Vec<Span>) {
        let mut i = range.start;
        while i < range.end {
            match self.span_at(i).filter(|s| s.range.end <= range.end) {
                Some(span) => {
                    i = span.range.end;
                    let nested = span.nests.then(|| span.content.clone());
                    out.push(Span::clone(&span));
                    if let Some(content) = nested {
                        self.inline(content, out);
                    }
                }
                None => i += self.char_len(i),
            }
        }
    }

    /// The span that starts at `at`, if one does.
    fn span_at(&self, at: usize) -> Option<Rc<Span>> {
        if let Some(span) = &self.found.borrow()[at] {
            return span.clone();
        }
        self.depth.set(self.depth.get() + 1);
        let span = self.parse_at(at).map(Rc::new);
        self.depth.set(self.depth.get() - 1);
        self.found.borrow_mut()[at] = Some(span.clone());
        span
    }

    /// Past the span at `at` inside another's content, or past its
    /// character once spans are nested too deep to look for.
    fn skip(&self, at: usize) -> usize {
        let inner = (self.depth.get() < MAX_DEPTH)
            .then(|| self.span_at(at))
            .flatten();
        inner.map_or(at + self.char_len(at), |s| s.range.end)
    }

    /// Works out the span that starts at `at`; see [`Self::span_at`].
    fn parse_at(&self, at: usize) -> Option<Span> {
        let s = self.source;
        let rest = &s[at..];
        match rest.as_bytes().first()? {
            b'\\' => escape(s, at),
            b'`' => code(s, at),
            b'!' if rest[1..].starts_with('[') => match wikilink(s, at + 1) {
                Some(link) => Some(embed(s, at, link)),
                None => Some(markdown_image(s, at, self.link(at + 1, true)?)),
            },
            b'[' => wikilink(s, at).or_else(|| self.link(at, false)),
            b'<' => autolink(s, at),
            b'h' => bare_url(s, at),
            b'$' => inline_math(s, at),
            _ => PAIRS
                .iter()
                .find_map(|&(delim, style)| self.emphasis(at, delim, style)),
        }
    }

    /// The length of the character at `at`.
    fn char_len(&self, at: usize) -> usize {
        self.source[at..].chars().next().map_or(1, char::len_utf8)
    }

    /// `delim content delim` from `at`: the content starts and ends with
    /// something other than whitespace, stays on one line, and spans
    /// inside it are skipped over while looking for the close.
    fn emphasis(&self, at: usize, delim: &str, style: Style) -> Option<Span> {
        let s = self.source;
        let start = at + delim.len();
        let first = s[at..].strip_prefix(delim)?.chars().next()?;
        if first.is_whitespace() {
            return None;
        }
        // `_` only delimits at word edges, so snake_case_names stay literal.
        let underscore = delim.starts_with('_');
        if underscore && is_word_char_before(s, at) {
            return None;
        }
        let mark = delim.chars().next()?;
        let mut j = start;
        // Whether `j` was reached past a span nested inside.
        let mut after_span = false;
        while j < s.len() && s.as_bytes()[j] != b'\n' {
            // A close follows text, or the end of a nested span: never
            // whitespace, nor the middle of a run of its own marker.
            let prev = s[..j].chars().next_back();
            let follows_text = after_span || prev.is_some_and(|c| !c.is_whitespace() && c != mark);
            let closes = j > start
                && follows_text
                && s[j..].starts_with(delim)
                && !(underscore && is_word_char_at(s, j + delim.len()));
            if closes {
                let range = at..j + delim.len();
                return Some(Span::paired(range, delim.len(), delim.len(), style).nesting());
            }
            let next = self.skip(j);
            after_span = next > j + self.char_len(j);
            j = next;
        }
        None
    }

    /// `[text](destination)` from `at`: shows only the text, which may hold
    /// more inline syntax.
    fn link(&self, at: usize, empty_text: bool) -> Option<Span> {
        let s = self.source;
        let bytes = s.as_bytes();
        // The text: brackets nest, code inside is skipped, one line.
        let mut depth = 0;
        let mut i = at;
        let text_end = loop {
            match *bytes.get(i)? {
                b'\n' => return None,
                b'\\' => i += 1,
                b'`' => i = code(s, i).map_or(i, |c| c.range.end - 1),
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        break i;
                    }
                }
                _ => {}
            }
            i += 1;
        };
        if (text_end == at + 1 && !empty_text) || bytes.get(text_end + 1) != Some(&b'(') {
            return None;
        }
        // The destination: parentheses nest, one line.
        let dest_start = text_end + 2;
        let mut depth = 0;
        let mut j = dest_start;
        let close = loop {
            match *bytes.get(j)? {
                b'\n' => return None,
                b'(' => depth += 1,
                b')' if depth == 0 => break j,
                b')' => depth -= 1,
                _ => {}
            }
            j += 1;
        };
        let destination = destination(&s[dest_start..close])?;
        let target = if md_engine::has_scheme(destination) {
            Target::Url(destination.to_string())
        } else {
            Target::Note(
                md_engine::percent_decode(destination).unwrap_or_else(|| destination.to_string()),
            )
        };
        let range = at..close + 1;
        Some(
            Span::paired(range, 1, close + 1 - text_end, Style::Link)
                .nesting()
                .to(target),
        )
    }
}

/// A link's destination, without the `<>` around it or a `"title"` (or
/// `'title'`) after it; anything else after it isn't a link.
fn destination(inside: &str) -> Option<&str> {
    let inside = inside.trim();
    let (dest, rest) = match inside.strip_prefix('<') {
        Some(rest) => rest.split_at(rest.find('>')?),
        None => inside.split_at(inside.find([' ', '\t']).unwrap_or(inside.len())),
    };
    let rest = rest.strip_prefix('>').unwrap_or(rest).trim();
    let titled = rest.is_empty()
        || ['"', '\'']
            .iter()
            .any(|&q| rest.len() > 1 && rest.starts_with(q) && rest.ends_with(q));
    (!dest.is_empty() && titled).then_some(dest)
}

/// `\` and an ASCII punctuation character: the character shows as typed,
/// the backslash is hidden.
fn escape(source: &str, at: usize) -> Option<Span> {
    let c = *source.as_bytes().get(at + 1)?;
    c.is_ascii_punctuation().then(|| {
        Span::new(
            at..at + 2,
            vec![Hide::new(at..at + 1)],
            at + 1..at + 2,
            None,
        )
    })
}

/// A code span: a run of backticks, up to the next run as long. Nothing
/// inside it is parsed. A run nothing closes is text, all of it.
fn code(source: &str, at: usize) -> Option<Span> {
    let bytes = source.as_bytes();
    let run = bytes[at..].iter().take_while(|&&b| b == b'`').count();
    let mut j = at + run;
    while j < bytes.len() {
        if bytes[j] == b'`' {
            let len = bytes[j..].iter().take_while(|&&b| b == b'`').count();
            if len == run {
                return Some(Span::paired(at..j + len, run, run, Style::Code));
            }
            j += len;
        } else {
            j += 1;
        }
    }
    Some(Span::literal(at..at + run))
}

/// `[[target]]` shows the target, a heading in it as `Note > Heading`;
/// `[[target|alias]]` shows the alias. An empty alias (`[[target|]]`)
/// shows the target. All on one line.
fn wikilink(source: &str, at: usize) -> Option<Span> {
    let inner = source[at..].strip_prefix("[[")?;
    let len = inner
        .find("]]")
        .filter(|&n| n > 0 && !inner[..n].contains(['\n', '[']))?;
    let body = at + 2..at + 2 + len;
    let close = body.end..body.end + 2;
    let text = &inner[..len];
    let (target, alias) = match text.split_once('|') {
        // In a table the pipe is written `\|`.
        Some((target, alias)) => (target.strip_suffix('\\').unwrap_or(target), Some(alias)),
        None => (text, None),
    };
    let mut hidden = Vec::new();
    let content = match alias {
        Some(alias) if !alias.is_empty() => {
            // Hide "target|" along with the opening brackets.
            let content = body.end - alias.len()..body.end;
            hidden.push(Hide::new(at..content.start));
            content
        }
        _ => {
            // The target, its headings joined by ` > `.
            let mut open = at..body.start;
            if target.starts_with('#') {
                // `[[#Heading]]`: just the heading.
                open.end += 1;
            }
            let content = open.end..body.start + target.len();
            hidden.push(Hide::new(open));
            for (i, _) in source[content.clone()].match_indices('#') {
                let range = content.start + i..content.start + i + 1;
                // A `#` with no heading after it is just hidden.
                let shown = if range.end == content.end { "" } else { " > " };
                hidden.push(Hide {
                    range,
                    shown: shown.into(),
                });
            }
            content
        }
    };
    // The closing brackets, and an empty alias's stray `|` with them.
    hidden.push(Hide::new(content.end..close.end));
    let target = Target::Note(target.to_string());
    Some(Span::new(at..close.end, hidden, content, Some(Style::Link)).to(target))
}

/// The image extensions drawn as images.
const IMAGE_EXTENSIONS: [&str; 9] = [
    "png", "jpg", "jpeg", "webp", "gif", "svg", "bmp", "tif", "tiff",
];

/// The image extension a link's target ends with, lower case: `None` if
/// it doesn't name an image.
pub fn image_extension(target: &str) -> Option<String> {
    let path = target.split(['#', '?']).next()?;
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    IMAGE_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// Whether a link's target names an image, by its extension.
fn is_image(target: &str) -> bool {
    image_extension(target).is_some()
}

/// A span of `range` drawn as `image`: all of it hidden, an [`OBJECT`] in
/// its place.
fn image_span(range: Range<usize>, image: Image) -> Span {
    let hide = Hide {
        range: range.clone(),
        shown: OBJECT.into(),
    };
    Span {
        image: Some(image),
        ..Span::new(range.clone(), vec![hide], range.end..range.end, None)
    }
}

/// `![[…]]` from `at`, given the wikilink after its `!`: an image if it
/// names one (its alias a size or alt text), else a link to the note it
/// embeds, shown as the wikilink is.
fn embed(source: &str, at: usize, link: Span) -> Span {
    let inner = &source[link.range.start + 2..link.range.end - 2];
    let (target, alias) = match inner.split_once('|') {
        Some((target, alias)) => (target.strip_suffix('\\').unwrap_or(target), alias),
        None => (inner, ""),
    };
    if !is_image(target) {
        // The `!` is hidden with the opening brackets.
        let mut link = link;
        link.range.start = at;
        link.hidden[0].range.start = at;
        return link;
    }
    let size = |text: &str| -> Option<(u32, Option<u32>)> {
        match text.split_once('x') {
            Some((w, h)) => Some((w.parse().ok()?, Some(h.parse().ok()?))),
            None => Some((text.parse().ok()?, None)),
        }
    };
    let (width, height, alt) = match size(alias) {
        Some((w, h)) => (Some(w), h, String::new()),
        None => (None, None, alias.to_string()),
    };
    let image = Image {
        at: 0,
        target: Target::Note(target.to_string()),
        alt,
        width,
        height,
    };
    image_span(at..link.range.end, image)
}

/// `![alt](src)` from `at`, given the markdown link after its `!`: an
/// image, unless it names a vault file that isn't one (a note), which is
/// a link to it, shown as the markdown link is.
fn markdown_image(source: &str, at: usize, link: Span) -> Span {
    if let Some(Target::Note(target)) = &link.target
        && !is_image(target)
    {
        let mut link = link;
        link.range.start = at;
        link.hidden[0].range.start = at;
        return link;
    }
    let image = Image {
        at: 0,
        target: link.target.clone().unwrap_or(Target::Url(String::new())),
        alt: source[link.content.clone()].to_string(),
        width: None,
        height: None,
    };
    image_span(at..link.range.end, image)
}

/// `$…$` from `at`, on one line: the content starts and ends with
/// something other than a space, and no digit follows the closing `$` (so
/// prices stay text). `$$` isn't inline math.
fn inline_math(source: &str, at: usize) -> Option<Span> {
    let rest = &source[at + 1..];
    let first = rest.chars().next()?;
    if first.is_whitespace() || first == '$' {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b'\n' {
        match bytes[i] {
            b'\\' => i += 1,
            b'$' if i > 0 && !bytes[i - 1].is_ascii_whitespace() => {
                if bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
                let range = at..at + i + 2;
                let hide = Hide {
                    range: range.clone(),
                    shown: OBJECT.into(),
                };
                return Some(Span {
                    math: Some(rest[..i].to_string()),
                    ..Span::new(range.clone(), vec![hide], range.end..range.end, None)
                });
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `<https://…>`: the address, without its angle brackets.
fn autolink(source: &str, at: usize) -> Option<Span> {
    let inner = &source[at + 1..];
    let len = inner.find('>')?;
    let url = &inner[..len];
    if !md_engine::has_scheme(url) || url.contains(char::is_whitespace) {
        return None;
    }
    let target = Target::Url(url.to_string());
    Some(Span::paired(at..at + len + 2, 1, 1, Style::Link).to(target))
}

/// A bare `http://` or `https://` address, at a word's start, up to
/// whitespace, without the punctuation or emphasis markers that end it
/// (a `)` stays while it closes a `(` in the address).
fn bare_url(source: &str, at: usize) -> Option<Span> {
    let rest = &source[at..];
    if !(rest.starts_with("https://") || rest.starts_with("http://"))
        || is_word_char_before(source, at)
    {
        return None;
    }
    let mut url = &rest[..rest.find(char::is_whitespace).unwrap_or(rest.len())];
    while let Some(last) = url.chars().next_back() {
        let unbalanced = last == ')' && url.matches(')').count() > url.matches('(').count();
        if !(unbalanced || ".,;:!?\"'*_~=".contains(last)) {
            break;
        }
        url = &url[..url.len() - 1];
    }
    if url.ends_with("//") {
        return None;
    }
    let range = at..at + url.len();
    let target = Target::Url(url.to_string());
    Some(Span::new(range.clone(), Vec::new(), range, Some(Style::Link)).to(target))
}

/// ATX headings: 1–6 `#` and a space at the start of a line. The span is the
/// whole line, so a caret anywhere on it reveals the markers.
fn headings(source: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut start = 0;
    for line in source.split('\n') {
        let hashes = line.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            let end = start + line.len();
            let marker = start..start + hashes + 1;
            let content = marker.end..end;
            let style = Some(Style::Heading(hashes as u8));
            spans.push(Span::new(
                start..end,
                vec![Hide::new(marker)],
                content,
                style,
            ));
        }
        start += line.len() + 1;
    }
    spans
}

/// Whether the character before `at` is a letter or digit.
fn is_word_char_before(source: &str, at: usize) -> bool {
    source[..at]
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
}

/// Whether the character at `at` is a letter or digit.
fn is_word_char_at(source: &str, at: usize) -> bool {
    source[at..]
        .chars()
        .next()
        .is_some_and(char::is_alphanumeric)
}
