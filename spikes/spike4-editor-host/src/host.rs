//! The GPUI half: an editable Live Preview whose blocks Kit renders through
//! `editor_host`. The source text is the model. Each frame the [`Doc`]'s
//! leaves are laid out with the conceal model and handed to Kit as keyed
//! text leaves; the geometry Kit reports back (one `LeafGeometry` per leaf)
//! places the caret and maps clicks, Up/Down and IME positions.
//!
//! Adapted from Spike 1's `ConcealInput`: the editing and IME callbacks are
//! the same, but positions go through leaves instead of one shaped text.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui_kit::base::text::editor_host::{
    BlockNode, CodeBlock, ColumnAlign, FlowPosition, InlineNode, LeafGeometry, Paragraph,
    RenderContext, Span, Table, TextMark, collect_leaf_geometry,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

use crate::model::{Doc, Kind, Selection, Style, View};

actions!(
    spike4,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Newline,
        Paste,
        Copy,
    ]
);

/// The key context the editor's bindings live in.
pub const KEY_CONTEXT: &str = "Spike4Editor";

/// Binds the editing keys.
pub fn bind_keys(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("ctrl-a", SelectAll, context),
        KeyBinding::new("enter", Newline, context),
        KeyBinding::new("ctrl-v", Paste, context),
        KeyBinding::new("ctrl-c", Copy, context),
    ]);
}

/// Revealed syntax can't be dimmed: `TextMark` has no foreground colour.
/// The workaround is a faint highlight behind it.
const SYNTAX_TINT: Hsla = Hsla {
    h: 0.,
    s: 0.,
    l: 0.5,
    a: 0.14,
};

/// Background ranges to paint behind each leaf's text, by leaf key.
type Decorations = HashMap<u64, Vec<(Range<usize>, Hsla)>>;

/// The geometry the last prepaint collected, by leaf key.
type Geometry = Rc<RefCell<HashMap<u64, LeafGeometry>>>;

/// Time spent in frames, for the 200 KB check.
#[derive(Clone, Debug, Default)]
pub struct Timings {
    pub frames: u32,
    /// Building and rendering the block nodes.
    pub render: Duration,
    /// Kit's prepaint inside `collect_leaf_geometry`.
    pub prepaint: Duration,
    pub slowest_prepaint: Duration,
    /// The whole frame from the editor's render to the end of its paint,
    /// layout included.
    pub frame: Duration,
    pub slowest_frame: Duration,
}

impl Timings {
    /// Takes what was counted so far, starting over.
    pub fn take(&mut self) -> Timings {
        std::mem::take(self)
    }
}

/// An editable note in Live Preview.
pub struct Editor {
    focus_handle: FocusHandle,
    source: String,
    doc: Doc,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// The leaf views the current frame renders, for the offset maps.
    views: Vec<View>,
    geometry: Geometry,
    /// The virtualized list's rows, and its state: only the rows in view
    /// are laid out.
    rows: Vec<Row>,
    list: ListState,
    is_selecting: bool,
    /// The x Up and Down aim for, kept across consecutive vertical moves.
    goal_x: Option<Pixels>,
    /// Scroll the caret into view on the next paint.
    reveal_caret: bool,
    /// The caret sits at a soft wrap and belongs to the start of the lower
    /// line (see `source_for_point`).
    wrap_next: bool,
    timings: Rc<RefCell<Timings>>,
    /// When this frame's render started, for the whole-frame timing.
    frame_started: Cell<Option<Instant>>,
}

/// The most list items one row of the virtualized list holds. A long
/// list is split into runs of items, each rendered as a list of its own,
/// so only the runs in view are laid out.
const LIST_CHUNK: usize = 20;

/// One item of the virtualized list: a block, or a run of a long list's
/// items (`leaves`, a part of the block's).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    block: usize,
    leaves: Range<usize>,
}

/// The rows of the virtualized list for a note.
fn rows(doc: &Doc) -> Vec<Row> {
    doc.blocks
        .iter()
        .enumerate()
        .flat_map(|(block, b)| {
            let chunk = if matches!(b.kind, Kind::List { .. }) {
                LIST_CHUNK
            } else {
                b.leaves.len().max(1)
            };
            let starts: Vec<usize> = b.leaves.clone().step_by(chunk).collect();
            let starts = if starts.is_empty() {
                vec![b.leaves.start]
            } else {
                starts
            };
            starts
                .into_iter()
                .map(move |start| Row {
                    block,
                    leaves: start..(start + chunk).min(b.leaves.end),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Whether two rows, each in its own note, read the same: the same kind of
/// block, the same source, and for a list the same task states.
fn same_row(a: (&Doc, &str, &Row), b: (&Doc, &str, &Row)) -> bool {
    let kind = |(doc, _, row): (&Doc, &str, &Row)| match &doc.blocks[row.block].kind {
        Kind::List { ordered, checked } => {
            let first = doc.blocks[row.block].leaves.start;
            let items = row.leaves.start - first..row.leaves.end - first;
            (Some((*ordered, checked[items].to_vec())), None)
        }
        other => (None, Some(other.clone())),
    };
    kind(a) == kind(b) && row_text(a.0, a.1, a.2) == row_text(b.0, b.1, b.2)
}

/// The source a row's leaves span.
fn row_text<'a>(doc: &Doc, source: &'a str, row: &Row) -> &'a str {
    match (
        doc.leaves.get(row.leaves.start),
        row.leaves
            .end
            .checked_sub(1)
            .and_then(|l| doc.leaves.get(l)),
    ) {
        (Some(first), Some(last)) if !row.leaves.is_empty() => {
            &source[first.source.start..last.source.end]
        }
        _ => "",
    }
}

/// A leaf's key in Kit: its index plus one.
fn leaf_key(leaf: usize) -> u64 {
    leaf as u64 + 1
}

impl Editor {
    /// An editor over `source`, with the caret at the start of the first
    /// leaf.
    pub fn new(source: &str, cx: &mut Context<Self>) -> Self {
        let doc = Doc::parse(source);
        let rows = rows(&doc);
        let start = doc.leaves.first().map_or(0, |l| l.source.start);
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            source: source.to_string(),
            doc,
            selected_range: start..start,
            selection_reversed: false,
            marked_range: None,
            views: Vec::new(),
            geometry: Geometry::default(),
            list: ListState::new(rows.len(), ListAlignment::Top, px(300.)),
            rows,
            is_selecting: false,
            goal_x: None,
            reveal_caret: false,
            wrap_next: false,
            timings: Rc::default(),
            frame_started: Cell::default(),
        };
        this.relayout();
        this
    }

    /// The handle that focuses the editor for typing.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// The note's markdown source.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The frame timings, shared with the element that records them.
    pub fn timings(&self) -> Rc<RefCell<Timings>> {
        self.timings.clone()
    }

    /// Puts the caret at `offset` (snapped into a leaf).
    pub fn set_caret(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.doc.snap(offset, false);
        self.move_to(offset, cx);
    }

    /// The note split into blocks and leaves.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// The selection in source offsets; empty for a caret.
    pub fn selected_range(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    /// The caret: the selection's moving end.
    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// The text the current frame shows for each leaf.
    pub fn display_texts(&self) -> Vec<String> {
        self.views
            .iter()
            .map(|v| v.display_text().to_string())
            .collect()
    }

    /// The geometry of the last frame, by leaf index.
    pub fn leaf_geometry(&self, leaf: usize) -> Option<LeafGeometry> {
        self.geometry.borrow().get(&leaf_key(leaf)).cloned()
    }

    /// Where the last frame drew the caret: its top, in window pixels.
    pub fn caret_point(&self) -> Option<Point<Pixels>> {
        self.point_for_source(self.cursor_offset())
    }

    /// The selection as anchor and head, for the conceal model.
    fn selection(&self) -> Selection {
        let r = &self.selected_range;
        if self.selection_reversed {
            Selection {
                anchor: r.end,
                head: r.start,
            }
        } else {
            Selection {
                anchor: r.start,
                head: r.end,
            }
        }
    }

    /// Re-lays out every leaf for the current selection, and has the list
    /// remeasure the blocks whose text changed (syntax revealed or hidden).
    fn relayout(&mut self) {
        let views = self.doc.views(&self.source, self.selection());
        let changed: Vec<usize> = (0..views.len())
            .filter(|&leaf| {
                self.views
                    .get(leaf)
                    .is_none_or(|old| old.display_text() != views[leaf].display_text())
            })
            .collect();
        self.views = views;
        for leaf in changed {
            if let Some(row) = self.row_of(leaf) {
                self.list.remeasure_items(row..row + 1);
            }
        }
    }

    /// The row of the virtualized list a leaf is in.
    fn row_of(&self, leaf: usize) -> Option<usize> {
        self.rows.iter().position(|r| r.leaves.contains(&leaf))
    }

    /// The leaf and display index of a source offset.
    fn locate(&self, source: usize) -> Option<(usize, usize)> {
        let leaf = self.doc.leaf_at(source)?;
        let local = source - self.doc.leaves[leaf].source.start;
        Some((leaf, self.views.get(leaf)?.to_display(local)))
    }

    /// The top of a caret at `source`, from the last frame's geometry.
    fn point_for_source(&self, source: usize) -> Option<Point<Pixels>> {
        let (leaf, display) = self.locate(source)?;
        let geometry = self.geometry.borrow();
        let g = geometry.get(&leaf_key(leaf))?;
        let drawn = g.position_for_index(display);
        if self.wrap_next && source == self.cursor_offset() {
            // The start of the next visual line: where the next character's
            // line is, at the leaf's left edge.
            let text = self.views[leaf].display_text();
            let next = text[display..]
                .chars()
                .next()
                .map_or(display, |c| display + c.len_utf8());
            let below = g.position_for_index(next);
            if below.y > drawn.y {
                return Some(point(g.bounds().left(), below.y));
            }
        }
        Some(drawn)
    }

    /// The line height of the leaf `source` is in.
    fn line_height_at(&self, source: usize) -> Pixels {
        self.locate(source)
            .and_then(|(leaf, _)| {
                self.geometry
                    .borrow()
                    .get(&leaf_key(leaf))
                    .map(LeafGeometry::line_height)
            })
            .unwrap_or(px(20.))
    }

    /// The source offset nearest a window point: the nearest leaf (by
    /// vertical, then horizontal distance), then the nearest caret in it.
    /// `only` limits which leaves count.
    /// Also says whether the point is on the visual line *after* the one
    /// Kit draws that index on: at a soft wrap, the end of one line and the
    /// start of the next are the same index, and `LeafGeometry` has no
    /// affinity, so it always draws it at the end of the upper line.
    fn source_for_point(
        &self,
        at: Point<Pixels>,
        only: impl Fn(&Bounds<Pixels>) -> bool,
    ) -> Option<(usize, bool)> {
        let geometry = self.geometry.borrow();
        // How far `v` is outside `lo..hi`.
        let outside = |v: Pixels, lo: Pixels, hi: Pixels| (lo - v).max(v - hi).max(px(0.));
        let distance = |b: &Bounds<Pixels>| {
            (
                outside(at.y, b.top(), b.bottom()),
                outside(at.x, b.left(), b.right()),
            )
        };
        let (leaf, g) = (0..self.doc.leaves.len())
            .filter_map(|leaf| Some((leaf, geometry.get(&leaf_key(leaf))?)))
            .filter(|(_, g)| only(&g.bounds()))
            .min_by(|(_, a), (_, b)| {
                distance(&a.bounds())
                    .partial_cmp(&distance(&b.bounds()))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })?;
        let b = g.bounds();
        let inside = point(
            at.x,
            at.y.clamp(b.top() + px(1.), (b.bottom() - px(1.)).max(b.top())),
        );
        let display = g.index_for_position(inside);
        let drawn = g.position_for_index(display);
        let next_line = drawn.y + g.line_height() <= inside.y;
        let local = self.views.get(leaf)?.to_source(display);
        Some((self.doc.leaves[leaf].source.start + local, next_line))
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.doc.prev_stop(&self.source, self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.doc.next_stop(&self.source, self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    /// The caret one visual line up or down, at `x`: within a
    /// wrapped leaf, or into the nearest leaf above or below (a table cell
    /// in the same column, the next block).
    fn vertical_target(&self, down: bool, x: Pixels) -> Option<(usize, bool)> {
        let caret = self.cursor_offset();
        let at = self.point_for_source(caret)?;
        let line = self.line_height_at(caret);
        let target = if down {
            point(x, at.y + line * 1.5)
        } else {
            point(x, at.y - line * 0.5)
        };
        let here = at.y + line / 2.;
        let found = self.source_for_point(target, |b| {
            if down {
                b.bottom() > here + line / 2.
            } else {
                b.top() < here - line / 2.
            }
        });
        // Nothing laid out below (or above): the next (or previous) leaf
        // if it's off screen, else the end (or start) of the caret's leaf.
        let edge = || {
            let leaf = self.doc.leaf_at(caret)?;
            let neighbour = if down {
                leaf.checked_add(1)
            } else {
                leaf.checked_sub(1)
            }
            .filter(|&n| n < self.doc.leaves.len() && self.leaf_geometry(n).is_none());
            Some(match neighbour {
                Some(n) if down => self.doc.leaves[n].source.start,
                Some(n) => self.doc.leaves[n].source.end,
                None if down => self.doc.leaves[leaf].source.end,
                None => self.doc.leaves[leaf].source.start,
            })
        };
        found
            .or_else(|| edge().map(|o| (o, false)))
            .map(|(o, next_line)| (self.doc.snap(o, !down), next_line))
    }

    /// Moves (or extends the selection) one visual line up or down. The
    /// goal x is kept across consecutive vertical moves, which `move_to`
    /// would otherwise clear.
    fn vertical(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let Some(x) = self
            .goal_x
            .or_else(|| self.point_for_source(self.cursor_offset()).map(|p| p.x))
        else {
            return;
        };
        if let Some((target, next_line)) = self.vertical_target(down, x) {
            if select {
                self.select_to(target, cx);
            } else {
                self.move_to(target, cx);
            }
            self.wrap_next = next_line;
        }
        self.goal_x = Some(x);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(false, false, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(true, false, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(false, true, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(true, true, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.doc.prev_stop(&self.source, self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.doc.next_stop(&self.source, self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let first = self.doc.snap(0, false);
        let last = self.doc.snap(self.source.len(), true);
        self.move_to(first, cx);
        self.select_to(last, cx)
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.doc.prev_stop(&self.source, self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.doc.next_stop(&self.source, self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.source[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let Some((ix, next_line)) = self.source_for_point(event.position, |_| true) else {
            return;
        };
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(ix, cx)
        } else {
            self.move_to(ix, cx)
        }
        self.wrap_next = next_line;
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting
            && let Some((ix, next_line)) = self.source_for_point(event.position, |_| true)
        {
            self.select_to(ix, cx);
            self.wrap_next = next_line;
        }
    }

    /// Puts the caret at `offset`, dropping any selection.
    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.after_move(cx);
    }

    /// Moves the selection's head to `offset`.
    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.after_move(cx);
    }

    /// After the caret moved: forget the goal x and the wrap side, reveal
    /// the caret and re-lay out the leaves it now touches.
    fn after_move(&mut self, cx: &mut Context<Self>) {
        self.goal_x = None;
        self.wrap_next = false;
        self.reveal_caret = true;
        self.relayout();
        cx.notify()
    }

    /// The source range an IME or edit callback targets: the given UTF-16
    /// range, else the marked (preedit) text, else the selection.
    fn edit_range(&self, range_utf16: Option<&Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone())
    }

    /// A source byte offset in UTF-16 units, as platform input counts.
    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.source[..offset.min(self.source.len())]
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    /// A source byte range in UTF-16 units.
    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    /// A UTF-16 range as source bytes.
    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        utf16_to_utf8(&self.source, range.start)..utf16_to_utf8(&self.source, range.end)
    }

    /// Replaces `range` with `text` and reparses the note.
    /// The list is told which rows changed: those between the rows that
    /// read the same before and after, from either end.
    fn edit(&mut self, range: Range<usize>, text: &str) {
        let old_source = self.source.clone();
        let old = std::mem::take(&mut self.doc);
        let old_rows = std::mem::take(&mut self.rows);
        self.source.replace_range(range, text);
        self.doc = Doc::parse(&self.source);
        self.rows = rows(&self.doc);
        let same = |a: usize, b: usize| {
            same_row(
                (&old, &old_source, &old_rows[a]),
                (&self.doc, &self.source, &self.rows[b]),
            )
        };
        let (n_old, n_new) = (old_rows.len(), self.rows.len());
        let prefix = (0..n_old.min(n_new)).take_while(|&i| same(i, i)).count();
        let suffix = (0..(n_old - prefix).min(n_new - prefix))
            .take_while(|&i| same(n_old - 1 - i, n_new - 1 - i))
            .count();
        self.list
            .splice(prefix..n_old - suffix, n_new - prefix - suffix);
    }

    /// Where to draw the caret, when there's no selection.
    pub fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        if !self.selected_range.is_empty() {
            return None;
        }
        let caret = self.cursor_offset();
        let at = self.point_for_source(caret)?;
        let (leaf, _) = self.locate(caret)?;
        let g = self.leaf_geometry(leaf)?;
        let height = g.caret_height();
        // Centred in the line box, whose leading Kit's position leaves out.
        let top = at.y + (g.line_height() - height) / 2.;
        Some(Bounds::new(point(at.x, top), size(px(2.), height)))
    }

    /// After the caret moved, scrolls so it's in view. Says whether it
    /// scrolled (the frame must be redrawn).
    fn scroll_caret_into_view(&mut self) -> bool {
        if !std::mem::take(&mut self.reveal_caret) {
            return false;
        }
        let caret = self.cursor_offset();
        let at = self.point_for_source(caret);
        let view = self.list.viewport_bounds();
        let line = self.line_height_at(caret);
        let row = self.doc.leaf_at(caret).and_then(|leaf| self.row_of(leaf));
        let Some(row) = row else {
            return false;
        };
        // Off screen (not laid out, so no geometry) or past an edge.
        let hidden = at.is_none_or(|at| at.y < view.top() || at.y + line > view.bottom());
        if hidden {
            self.list.scroll_to_reveal_item(row);
        }
        hidden
    }

    /// The part of `range` (source) inside `leaf`, as a display range in
    /// that leaf, if it isn't empty.
    fn display_range(&self, leaf: usize, range: &Range<usize>) -> Option<Range<usize>> {
        let source = &self.doc.leaves[leaf].source;
        let start = range.start.max(source.start);
        let end = range.end.min(source.end);
        let view = self.views.get(leaf)?;
        (start < end)
            .then(|| view.to_display(start - source.start)..view.to_display(end - source.start))
    }

    /// The selection as display ranges, by leaf: what the frame highlights.
    pub fn selection_highlights(&self) -> Vec<(usize, Range<usize>)> {
        (0..self.doc.leaves.len())
            .filter_map(|leaf| Some((leaf, self.display_range(leaf, &self.selected_range)?)))
            .collect()
    }

    /// The Kit block node for each row of the current frame, and the
    /// decorations (selection) per leaf key.
    fn blocks(&self, cx: &App) -> (Vec<BlockNode>, Decorations) {
        let selection_color = cx.theme().colors.selection.opacity(0.5);
        let paragraph = |leaf: usize| -> Paragraph {
            let view = &self.views[leaf];
            let marked = self
                .marked_range
                .as_ref()
                .and_then(|m| self.display_range(leaf, m));
            let mut p = Paragraph::default();
            p.push(InlineNode::new(view.display_text().to_string()).marks(marks(view, marked)));
            p.with_leaf_key(leaf_key(leaf))
        };
        let decorations = self
            .selection_highlights()
            .into_iter()
            .map(|(leaf, range)| (leaf_key(leaf), vec![(range, selection_color)]))
            .collect();
        let blocks = self
            .rows
            .iter()
            .map(|row| {
                let block = &self.doc.blocks[row.block];
                let first = block.leaves.start;
                match &block.kind {
                    Kind::Paragraph | Kind::Empty => BlockNode::Paragraph(paragraph(first)),
                    Kind::Rule => BlockNode::HorizontalRule { span: None },
                    Kind::Heading(level) => BlockNode::Heading {
                        level: *level,
                        children: paragraph(first),
                        span: None,
                    },
                    Kind::List { ordered, checked } => BlockNode::List {
                        children: row
                            .leaves
                            .clone()
                            .zip(&checked[row.leaves.start - first..])
                            .map(|(leaf, checked)| BlockNode::ListItem {
                                children: vec![BlockNode::Paragraph(paragraph(leaf))],
                                spread: false,
                                checked: *checked,
                                span: None,
                            })
                            .collect(),
                        ordered: *ordered,
                        // A later run of a numbered list continues its count.
                        start: Some((row.leaves.start - first + 1) as u32),
                        span: None,
                    },
                    Kind::Code { lang } => BlockNode::CodeBlock(
                        CodeBlock::new(
                            self.views[first].display_text().to_string().into(),
                            lang.clone().map(SharedString::from),
                            None::<Span>,
                        )
                        .with_leaf_key(leaf_key(first)),
                    ),
                    Kind::Table { columns, rows } => {
                        let mut leaves = block.leaves.clone();
                        // A short row's missing cells render empty, unkeyed.
                        let rows = rows
                            .iter()
                            .map(|&cells| {
                                let mut row: Vec<Paragraph> =
                                    leaves.by_ref().take(cells).map(paragraph).collect();
                                row.resize_with(*columns, Paragraph::default);
                                row
                            })
                            .collect();
                        BlockNode::Table(Table::new(rows, vec![ColumnAlign::None; *columns]))
                    }
                }
            })
            .collect();
        (blocks, decorations)
    }
}

/// Kit marks for a leaf: the conceal runs flattened into non-overlapping
/// ranges, plus an underline under IME preedit text.
fn marks(view: &View, marked: Option<Range<usize>>) -> Vec<(Range<usize>, TextMark)> {
    let len = view.display_text().len();
    let runs = view.runs();
    let mut cuts = vec![0, len];
    for r in runs {
        cuts.extend([r.range.start, r.range.end]);
    }
    if let Some(m) = &marked {
        cuts.extend([m.start, m.end]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .filter(|w| w[0] < w[1])
        .filter_map(|w| {
            let seg = w[0]..w[1];
            let mut mark = TextMark::default();
            for r in runs
                .iter()
                .filter(|r| r.range.start <= seg.start && seg.end <= r.range.end)
            {
                match r.style {
                    Style::Bold => mark.bold = true,
                    Style::Italic => mark.italic = true,
                    Style::Strike => mark.strikethrough = true,
                    Style::Code => mark.code = true,
                    // A real link mark would make the text a link Kit
                    // follows on click; the spike only underlines.
                    Style::Link => mark.underline = true,
                    // Kit's heading block sets the size and weight.
                    Style::Heading(_) => {}
                    Style::Syntax => mark.highlight = Some(SYNTAX_TINT),
                }
            }
            if marked
                .as_ref()
                .is_some_and(|m| m.start <= seg.start && seg.end <= m.end)
            {
                mark.underline = true;
            }
            (mark != TextMark::default()).then_some((seg, mark))
        })
        .collect()
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.source[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.edit_range(range_utf16.as_ref());
        let (before, after) = if range.is_empty() && !new_text.is_empty() {
            self.doc.line_breaks_for_insert(&self.source, range.start)
        } else {
            ("", "")
        };
        self.edit(range.clone(), &format!("{before}{new_text}{after}"));
        let caret = self
            .doc
            .snap(range.start + before.len() + new_text.len(), false);
        self.marked_range = None;
        self.move_to(caret, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.edit_range(range_utf16.as_ref());
        self.edit(range.clone(), new_text);
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        // The new selection is relative to the inserted text, in its UTF-16.
        self.selected_range = new_selected_range_utf16
            .map(|sel| {
                range.start + utf16_to_utf8(new_text, sel.start)
                    ..range.start + utf16_to_utf8(new_text, sel.end)
            })
            .unwrap_or_else(|| {
                let caret = range.start + new_text.len();
                caret..caret
            });
        self.selection_reversed = false;
        self.after_move(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let start = self.point_for_source(range.start)?;
        let end = self.point_for_source(range.end).unwrap_or(start);
        let line = self.line_height_at(range.start);
        let end_x = if end.y == start.y { end.x } else { start.x };
        Some(Bounds::from_corners(
            start,
            point(end_x.max(start.x + px(1.)), start.y + line),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let (ix, _) = self.source_for_point(point, |_| true)?;
        Some(self.offset_to_utf16(ix))
    }
}

/// The UTF-8 byte offset of UTF-16 offset `n` within `text`.
fn utf16_to_utf8(text: &str, n: usize) -> usize {
    let mut utf16 = 0;
    for (ix, ch) in text.char_indices() {
        if utf16 >= n {
            return ix;
        }
        utf16 += ch.len_utf16();
    }
    text.len()
}

/// The editor's root element: prepaints the rendered blocks inside a
/// `collect_leaf_geometry` scope, keeps the geometry for the editor, then
/// paints the caret over the text and registers the input handler.
struct HostElement {
    editor: Entity<Editor>,
    child: Option<AnyElement>,
    decorations: Decorations,
}

impl IntoElement for HostElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for HostElement {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let mut child = self.child.take().expect("a child");
        let style = gpui_kit::Style {
            size: size(relative(1.).into(), relative(1.).into()),
            ..Default::default()
        };
        let child_id = child.request_layout(window, cx);
        (window.request_layout(style, [child_id], cx), child)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        let decorations = std::mem::take(&mut self.decorations);
        let (_, geometry) = collect_leaf_geometry(decorations, || child.prepaint(window, cx));
        let editor = self.editor.read(cx);
        *editor.geometry.borrow_mut() = geometry;
        let took = started.elapsed();
        let mut t = editor.timings.borrow_mut();
        t.prepaint += took;
        t.slowest_prepaint = t.slowest_prepaint.max(took);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
        let editor = self.editor.read(cx);
        let focus_handle = editor.focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        if focus_handle.is_focused(window)
            && let Some(caret) = editor.caret_bounds()
        {
            window.paint_quad(fill(caret, cx.theme().colors.caret));
        }
        let frame_started = editor.frame_started.take();
        let scrolled = self
            .editor
            .update(cx, |editor, _| editor.scroll_caret_into_view());
        if scrolled {
            window.refresh();
        }
        let editor = self.editor.read(cx);
        if let Some(started) = frame_started {
            let mut t = editor.timings.borrow_mut();
            t.frame += started.elapsed();
            t.slowest_frame = t.slowest_frame.max(started.elapsed());
        }
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = Instant::now();
        self.frame_started.set(Some(started));
        let rcx = RenderContext::from_app(cx).with_body_font_size(px(16.));
        let (blocks, decorations) = self.blocks(cx);
        let flows: Vec<FlowPosition> = {
            let refs: Vec<&BlockNode> = blocks.iter().collect();
            let rows = &self.rows;
            (0..refs.len())
                .map(|ix| {
                    let flow = FlowPosition::at(&refs, ix, ix + 1 == refs.len());
                    // The runs of one list sit together, with no gap between.
                    let same_block =
                        |other: Option<&Row>| other.is_some_and(|o| o.block == rows[ix].block);
                    let flow = if same_block(ix.checked_sub(1).and_then(|p| rows.get(p))) {
                        flow.first(true)
                    } else {
                        flow
                    };
                    if same_block(rows.get(ix + 1)) {
                        flow.last(true)
                    } else {
                        flow
                    }
                })
                .collect()
        };
        let blocks = Rc::new(blocks);
        // Only the blocks the list lays out (those in view) render.
        let content = list(self.list.clone(), move |ix, window, cx| {
            div()
                .px_4()
                .child(blocks[ix].render_with_id(("block", ix), flows[ix], &rcx, window, cx))
                .into_any_element()
        })
        .size_full()
        .into_any_element();
        {
            let mut t = self.timings.borrow_mut();
            t.frames += 1;
            t.render += started.elapsed();
        }
        div()
            .size_full()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(HostElement {
                editor: cx.entity(),
                child: Some(content),
                decorations,
            })
    }
}
