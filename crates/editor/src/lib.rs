//! The Live Preview editor element: draws a [`live_preview::Document`]
//! through Kit's `editor_host` (ADR 0011) and turns keys, the mouse and
//! platform input (IME) into the document's edits and moves.
//!
//! Each frame the document's leaves are laid out by the conceal model and
//! handed to Kit as keyed text leaves; the geometry Kit reports back (one
//! `LeafGeometry` per leaf) places the caret and maps clicks, Up/Down and
//! IME positions. Grown from Spike 4's host (`spikes/spike4-editor-host`).

use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc};

use gpui_kit::base::input::{
    Backspace, Copy, Cut, Delete, DeleteToNextWordEnd, DeleteToPreviousWordStart, MoveDown,
    MoveEnd, MoveHome, MoveLeft, MoveRight, MoveToEnd, MoveToNextWord, MoveToPreviousWord,
    MoveToStart, MoveUp, Paste, Redo, SelectAll, SelectToEnd, SelectToEndOfLine,
    SelectToNextWordEnd, SelectToPreviousWordStart, SelectToStart, SelectToStartOfLine, Undo,
};
use gpui_kit::base::text::editor_host::{
    BlockNode, FlowPosition, InlineNode, LeafGeometry, LinkMark, Paragraph, RenderContext,
    TextMark, collect_leaf_geometry,
};
use gpui_kit::base::v_flex;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{Replace, Search};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use live_preview::blocks::{Block, Kind};
use live_preview::conceal::{Style, Target};
use live_preview::{Document, View};

mod find;
mod suggest;

pub use suggest::{Links, Suggestion, Suggestions};

actions!(
    live_preview,
    [
        /// Shift+Left.
        SelectLeft,
        /// Shift+Right.
        SelectRight,
        /// Shift+Up.
        SelectUp,
        /// Shift+Down.
        SelectDown,
        /// Enter.
        Newline,
        /// Tab: accepts a suggestion, else types a tab.
        InsertTab,
        /// Escape: closes the suggestions.
        Dismiss
    ]
);

/// The key context the editor's bindings live in.
pub const KEY_CONTEXT: &str = "LivePreview";

/// Binds the editing keys, in the editor's own key context. Copy, Cut and
/// Paste are Kit's own actions, so whatever intercepts them for Kit's
/// editor (the protected-note guard) intercepts them here too.
pub fn init(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("shift-backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("ctrl-backspace", DeleteToPreviousWordStart, context),
        KeyBinding::new("ctrl-delete", DeleteToNextWordEnd, context),
        KeyBinding::new("left", MoveLeft, context),
        KeyBinding::new("right", MoveRight, context),
        KeyBinding::new("up", MoveUp, context),
        KeyBinding::new("down", MoveDown, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("ctrl-left", MoveToPreviousWord, context),
        KeyBinding::new("ctrl-right", MoveToNextWord, context),
        KeyBinding::new("ctrl-shift-left", SelectToPreviousWordStart, context),
        KeyBinding::new("ctrl-shift-right", SelectToNextWordEnd, context),
        KeyBinding::new("home", MoveHome, context),
        KeyBinding::new("end", MoveEnd, context),
        KeyBinding::new("shift-home", SelectToStartOfLine, context),
        KeyBinding::new("shift-end", SelectToEndOfLine, context),
        KeyBinding::new("ctrl-home", MoveToStart, context),
        KeyBinding::new("ctrl-end", MoveToEnd, context),
        KeyBinding::new("ctrl-shift-home", SelectToStart, context),
        KeyBinding::new("ctrl-shift-end", SelectToEnd, context),
        KeyBinding::new("ctrl-a", SelectAll, context),
        KeyBinding::new("enter", Newline, context),
        KeyBinding::new("ctrl-z", Undo, context),
        KeyBinding::new("ctrl-shift-z", Redo, context),
        KeyBinding::new("ctrl-y", Redo, context),
        KeyBinding::new("ctrl-c", Copy, context),
        KeyBinding::new("ctrl-x", Cut, context),
        KeyBinding::new("ctrl-v", Paste, context),
        KeyBinding::new("tab", InsertTab, context),
        KeyBinding::new("escape", Dismiss, context),
        KeyBinding::new("ctrl-f", Search, context),
        KeyBinding::new("ctrl-h", Replace, context),
    ]);
    find::init(cx);
}

/// Revealed syntax can't be dimmed: `TextMark` has no text colour
/// (ADR 0011). The workaround is a faint highlight behind it.
const SYNTAX_TINT: Hsla = Hsla {
    h: 0.,
    s: 0.,
    l: 0.5,
    a: 0.14,
};

/// Behind `==highlighted==` text: a marker-pen yellow.
const HIGHLIGHT: Hsla = Hsla {
    h: 50. / 360.,
    s: 1.,
    l: 0.5,
    a: 0.35,
};

/// The line height assumed for a leaf not laid out yet.
const FALLBACK_LINE_HEIGHT: Pixels = px(20.);

/// Background ranges to paint behind each leaf's text, by leaf key.
type Decorations = HashMap<u64, Vec<(Range<usize>, Hsla)>>;

/// The geometry the last prepaint collected, by leaf key.
type Geometry = Rc<RefCell<HashMap<u64, LeafGeometry>>>;

/// A leaf's key in Kit: its index plus one.
fn leaf_key(leaf: usize) -> u64 {
    leaf as u64 + 1
}

/// What the editor tells its owner.
#[derive(Clone, Debug)]
pub enum LivePreviewEvent {
    /// The person edited the text (typing, deleting, undo, paste). Loading
    /// text isn't an edit.
    Edited,
    /// A link to a note was clicked: its target, `Note#Heading`. A click on
    /// a web address opens it instead.
    FollowLink(String),
}

/// A note being edited in Live Preview.
pub struct LivePreview {
    focus_handle: FocusHandle,
    doc: Document,
    /// IME preedit text, as a source range.
    marked_range: Option<Range<usize>>,
    /// The leaf views the current frame renders, for the offset maps.
    views: Vec<View>,
    geometry: Geometry,
    /// The virtualized list, one item per block: only the blocks in view
    /// are laid out.
    list: ListState,
    is_selecting: bool,
    /// The x Up and Down aim for, kept across consecutive vertical moves.
    goal_x: Option<Pixels>,
    /// Scroll the caret into view on the next paint.
    reveal_caret: bool,
    /// The caret sits at a soft wrap and belongs to the start of the lower
    /// line: Kit's `LeafGeometry` has no affinity, so it would draw it at
    /// the end of the upper one (see `source_for_point`).
    wrap_next: bool,
    /// Which link targets resolve, and what to suggest in a link being
    /// typed.
    links: Rc<dyn Links>,
    /// The find bar, while it's open.
    find: Option<find::Find>,
    /// The suggestions pop-up, while it's open.
    popup: Option<suggest::Popup>,
}

impl EventEmitter<LivePreviewEvent> for LivePreview {}

impl LivePreview {
    /// An editor over a note's `text`, the caret at its start; `links`
    /// says which links resolve (the others are drawn as unresolved) and
    /// what to suggest as one is typed.
    pub fn new(text: &str, links: Rc<dyn Links>, cx: &mut Context<Self>) -> Self {
        let doc = Document::new(text);
        let blocks = doc.blocks().blocks.len();
        let mut this = LivePreview {
            focus_handle: cx.focus_handle(),
            doc,
            marked_range: None,
            views: Vec::new(),
            geometry: Geometry::default(),
            list: ListState::new(blocks, ListAlignment::Top, px(300.)),
            is_selecting: false,
            goal_x: None,
            reveal_caret: false,
            wrap_next: false,
            links,
            find: None,
            popup: None,
        };
        this.relayout();
        this
    }

    /// The handle that has focus while the note is being edited.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// The note's source as it is now.
    pub fn text(&self) -> &str {
        self.doc.text()
    }

    /// The caret's offset.
    pub fn caret(&self) -> usize {
        self.doc.caret()
    }

    /// The selection, start first; empty for a caret.
    pub fn selection(&self) -> Range<usize> {
        self.doc.selection()
    }

    /// The caret's line and column in the source, from zero.
    pub fn line_column(&self) -> (u32, u32) {
        self.doc.line_column()
    }

    /// Replaces the whole text with `text` from disk: not an edit, and
    /// nothing to undo before it.
    pub fn load(&mut self, text: &str, cx: &mut Context<Self>) {
        self.doc.load(text);
        self.popup = None;
        self.reset_list();
        self.refresh_find(cx);
        self.after_move(cx);
    }

    /// Takes the text as edited in Source mode: the part that differs is
    /// one undo step. Not reported as an edit; the owner has seen it.
    pub fn take_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text != self.doc.text() {
            self.doc.take_text(text);
            self.popup = None;
            self.reset_list();
            self.refresh_find(cx);
            self.after_move(cx);
        }
    }

    /// Replaces `range` with `text`, then selects `selection` (a format
    /// command): one undo step.
    pub fn replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        selection: Range<usize>,
        cx: &mut Context<Self>,
    ) {
        self.edited(cx, |doc| doc.replace(range, text, selection));
    }

    /// Where the view is scrolled: the source offset the block at its top
    /// starts at, and how far into that block. (Blocks out of view aren't
    /// measured, so pixels from the top wouldn't come back right.)
    pub fn scroll_top(&self) -> (usize, Pixels) {
        let top = self.list.logical_scroll_top();
        let start = self.doc.blocks().block_start(top.item_ix);
        (start.unwrap_or(self.doc.text().len()), top.offset_in_item)
    }

    /// Scrolls the block holding source offset `top` to the top of the
    /// view, `within` pixels into it.
    pub fn scroll_to_top(&mut self, top: usize, within: Pixels, cx: &mut Context<Self>) {
        self.list.scroll_to(ListOffset {
            item_ix: self.doc.blocks().block_at(top),
            offset_in_item: within,
        });
        cx.notify();
    }

    /// Puts the caret at the start of `line` (from zero), scrolled into
    /// view.
    pub fn go_to_line(&mut self, line: u32, cx: &mut Context<Self>) {
        let at = self.doc.line_start(line);
        self.doc.set_caret(at);
        self.after_move(cx);
    }

    /// Forgets the undo history (the note's protected folder is locking).
    pub fn forget_history(&mut self) {
        self.doc.forget_history();
    }

    /// Makes an edit, then re-lays out what changed and tells the owner.
    fn edited(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Document)) {
        // Typing opens it again (see `update_suggestions`).
        self.popup = None;
        let before: Vec<(Kind, String)> = self.block_texts();
        let text_before = self.doc.text().len();
        edit(&mut self.doc);
        let after = self.block_texts();
        if before != after || text_before != self.doc.text().len() {
            // Only the blocks between the ones that read the same before
            // and after, from either end, are remeasured.
            let prefix = before
                .iter()
                .zip(&after)
                .take_while(|(a, b)| a == b)
                .count();
            let suffix = before[prefix..]
                .iter()
                .rev()
                .zip(after[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            self.list
                .splice(prefix..before.len() - suffix, after.len() - prefix - suffix);
            cx.emit(LivePreviewEvent::Edited);
            self.refresh_find(cx);
        }
        self.after_move(cx);
    }

    /// Each block's kind and source, to compare before and after an edit.
    fn block_texts(&self) -> Vec<(Kind, String)> {
        let blocks = self.doc.blocks();
        blocks
            .blocks
            .iter()
            .map(|b| (b.kind.clone(), self.block_source(b).to_string()))
            .collect()
    }

    /// The source a block's leaves span.
    fn block_source(&self, block: &Block) -> &str {
        let leaves = &self.doc.blocks().leaves;
        match (
            leaves.get(block.leaves.start),
            block.leaves.end.checked_sub(1).and_then(|l| leaves.get(l)),
        ) {
            (Some(first), Some(last)) => &self.doc.text()[first.source.start..last.source.end],
            _ => "",
        }
    }

    /// Starts the list over: the blocks were all replaced.
    fn reset_list(&mut self) {
        let blocks = self.doc.blocks().blocks.len();
        let old = self.list.item_count();
        self.list.splice(0..old, blocks);
    }

    /// Re-lays out every leaf for the current selection, and has the list
    /// remeasure the blocks whose text changed (syntax revealed or hidden).
    fn relayout(&mut self) {
        let views = self.doc.views();
        let changed: Vec<usize> = (0..views.len())
            .filter(|&leaf| {
                self.views
                    .get(leaf)
                    .is_none_or(|old| old.display_text() != views[leaf].display_text())
            })
            .collect();
        self.views = views;
        for leaf in changed {
            if let Some(block) = self.doc.blocks().block_index_of(leaf) {
                self.list.remeasure_items(block..block + 1);
            }
        }
    }

    /// The leaf and display index of a source offset.
    fn locate(&self, source: usize) -> Option<(usize, usize)> {
        let blocks = self.doc.blocks();
        let leaf = blocks.leaf_at(source)?;
        let local = source - blocks.leaves[leaf].source.start;
        Some((leaf, self.views.get(leaf)?.to_display(local)))
    }

    /// The geometry the last frame laid a leaf out with.
    fn leaf_geometry(&self, leaf: usize) -> Option<LeafGeometry> {
        self.geometry.borrow().get(&leaf_key(leaf)).cloned()
    }

    /// The top of a caret at `source`, from the last frame's geometry.
    fn point_for_source(&self, source: usize) -> Option<Point<Pixels>> {
        let (leaf, display) = self.locate(source)?;
        let g = self.leaf_geometry(leaf)?;
        let drawn = g.position_for_index(display);
        if self.wrap_next && source == self.doc.caret() {
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
            .and_then(|(leaf, _)| self.leaf_geometry(leaf))
            .map_or(FALLBACK_LINE_HEIGHT, |g| g.line_height())
    }

    /// The source offset nearest a window point: the nearest leaf laid out
    /// (by vertical, then horizontal distance), then the nearest caret in
    /// it. `only` limits which leaves count. Also says whether the point is
    /// on the visual line *after* the one Kit draws that index on: at a
    /// soft wrap, the end of one line and the start of the next are the
    /// same index.
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
        let leaves = &self.doc.blocks().leaves;
        let (leaf, g) = (0..leaves.len())
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
        Some((leaves[leaf].source.start + local, next_line))
    }

    /// The link a click at `at` follows: one on the text of a link whose
    /// syntax is hidden, not beside it.
    fn link_at_point(&self, at: Point<Pixels>) -> Option<Target> {
        let geometry = self.geometry.borrow();
        let (leaf, g) = (0..self.views.len())
            .filter_map(|leaf| Some((leaf, geometry.get(&leaf_key(leaf))?)))
            .find(|(_, g)| g.bounds().contains(&at))?;
        let index = g.index_for_position(at);
        let line = |i: usize| {
            let p = g.position_for_index(i);
            p.y <= at.y && at.y < p.y + g.line_height()
        };
        // Past the link's first character's left edge and its last one's
        // right edge, on their lines.
        let on = |range: &Range<usize>| {
            let start = g.position_for_index(range.start);
            let end = g.position_for_index(range.end);
            range.start < index && index < range.end
                || index == range.start && line(index) && at.x >= start.x
                || index == range.end && line(index) && at.x <= end.x
        };
        // A link inside another's text is the one clicked.
        self.views[leaf]
            .links()
            .iter()
            .filter(|link| link.followable && !link.range.is_empty() && on(&link.range))
            .min_by_key(|link| link.range.len())
            .map(|link| link.target.clone())
    }

    /// The caret one visual line up or down, at `x`: within a wrapped
    /// leaf, or into the nearest leaf above or below.
    fn vertical_target(&self, down: bool, x: Pixels) -> Option<(usize, bool)> {
        let caret = self.doc.caret();
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
        let leaves = &self.doc.blocks().leaves;
        let edge = || {
            let leaf = self.doc.blocks().leaf_at(caret)?;
            let neighbour = if down {
                leaf.checked_add(1)
            } else {
                leaf.checked_sub(1)
            }
            .filter(|&n| n < leaves.len() && self.leaf_geometry(n).is_none());
            Some(match neighbour {
                Some(n) if down => leaves[n].source.start,
                Some(n) => leaves[n].source.end,
                None if down => leaves[leaf].source.end,
                None => leaves[leaf].source.start,
            })
        };
        found
            .or_else(|| edge().map(|o| (o, false)))
            .map(|(o, next_line)| (self.doc.blocks().snap(o, !down), next_line))
    }

    /// Moves (or extends the selection) one visual line up or down. The
    /// goal x is kept across consecutive vertical moves.
    fn vertical(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        self.popup = None;
        let Some(x) = self
            .goal_x
            .or_else(|| self.point_for_source(self.doc.caret()).map(|p| p.x))
        else {
            return;
        };
        if let Some((target, next_line)) = self.vertical_target(down, x) {
            self.doc.move_to(target, extend);
            self.after_move(cx);
            self.wrap_next = next_line;
        }
        self.goal_x = Some(x);
    }

    /// Runs a move on the document, then re-lays out what it revealed.
    fn moved(&mut self, cx: &mut Context<Self>, move_: impl FnOnce(&mut Document)) {
        self.popup = None;
        move_(&mut self.doc);
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

    /// Copy: the selection's source, syntax and all.
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let range = self.doc.selection();
        if !range.is_empty() {
            let text = self.doc.text()[range].to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Cut: copies the selection, then deletes it.
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.doc.selection().is_empty() {
            self.copy(&Copy, window, cx);
            self.edited(cx, Document::delete);
        }
    }

    /// Paste: the clipboard's text over the selection.
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.edited(cx, |doc| doc.paste(&text));
            // As Source mode: pasting part of a name after `[[` suggests.
            self.update_suggestions(cx);
        }
    }

    /// A click on a link (its syntax hidden) follows it. Elsewhere a click
    /// places the caret (Shift extends the selection) and starts a drag; a
    /// double-click selects a word, a triple-click a line.
    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.close_suggestions(cx);
        if event.click_count == 1
            && !event.modifiers.shift
            && let Some(target) = self.link_at_point(event.position)
        {
            match target {
                Target::Note(target) => cx.emit(LivePreviewEvent::FollowLink(target)),
                Target::Url(url) => cx.open_url(&url),
            }
            return;
        }
        let Some((at, next_line)) = self.source_for_point(event.position, |_| true) else {
            return;
        };
        match event.click_count {
            2 => {
                let word = self.doc.word_at(at);
                self.moved(cx, |doc| doc.select(word));
            }
            3.. => {
                let line = self.doc.line_range(at);
                self.moved(cx, |doc| doc.select(line));
            }
            _ => {
                self.is_selecting = true;
                let extend = event.modifiers.shift;
                self.moved(cx, |doc| doc.move_to(at, extend));
                self.wrap_next = next_line;
            }
        }
    }

    /// The drag ends.
    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    /// While dragging, the selection follows the pointer.
    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Released over a link, the drag's mouse-up went to Kit's link
        // handler instead.
        if event.pressed_button != Some(MouseButton::Left) {
            self.is_selecting = false;
        }
        if self.is_selecting
            && let Some((at, next_line)) = self.source_for_point(event.position, |_| true)
        {
            self.moved(cx, |doc| doc.move_to(at, true));
            self.wrap_next = next_line;
        }
    }

    /// Where to draw the caret, when there's no selection.
    fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        if !self.doc.selection().is_empty() {
            return None;
        }
        let caret = self.doc.caret();
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
        let caret = self.doc.caret();
        let at = self.point_for_source(caret);
        let view = self.list.viewport_bounds();
        let line = self.line_height_at(caret);
        let block = self
            .doc
            .blocks()
            .leaf_at(caret)
            .and_then(|leaf| self.doc.blocks().block_index_of(leaf));
        let Some(block) = block else {
            return false;
        };
        // Off screen (not laid out, so no geometry) or past an edge.
        let hidden = at.is_none_or(|at| at.y < view.top() || at.y + line > view.bottom());
        if hidden {
            self.list.scroll_to_reveal_item(block);
        }
        hidden
    }

    /// The part of `range` (source) inside `leaf`, as a display range in
    /// that leaf, if it isn't empty.
    fn display_range(&self, leaf: usize, range: &Range<usize>) -> Option<Range<usize>> {
        let source = &self.doc.blocks().leaves[leaf].source;
        let start = range.start.max(source.start);
        let end = range.end.min(source.end);
        let view = self.views.get(leaf)?;
        (start < end)
            .then(|| view.to_display(start - source.start)..view.to_display(end - source.start))
    }

    /// The Kit block node for each block, and the decorations (the
    /// selection, and find's other matches) per leaf key.
    fn render_blocks(&self, cx: &App) -> (Vec<BlockNode>, Decorations) {
        let selection_color = cx.theme().colors.selection.opacity(0.5);
        // As Kit's editor: the other matches in a fainter selection colour.
        let match_color = cx.theme().colors.selection.opacity(0.2);
        let paragraph = |leaf: usize| -> Paragraph {
            let view = &self.views[leaf];
            let marked = self
                .marked_range
                .as_ref()
                .and_then(|m| self.display_range(leaf, m));
            let mut p = Paragraph::default();
            let marks = marks(view, marked, &|target| self.links.resolves(target));
            p.push(InlineNode::new(view.display_text().to_string()).marks(marks));
            p.with_leaf_key(leaf_key(leaf))
        };
        let selection = self.doc.selection();
        let mut decorations = Decorations::default();
        let mut decorate = |range: &Range<usize>, color: Hsla| {
            // From the first leaf the range reaches: it may start between
            // leaves (on a blank line).
            let leaves = &self.doc.blocks().leaves;
            let first = leaves.partition_point(|l| l.source.end < range.start);
            for leaf in first..self.views.len() {
                if self.doc.blocks().leaves[leaf].source.start > range.end {
                    break;
                }
                if let Some(shown) = self.display_range(leaf, range) {
                    decorations
                        .entry(leaf_key(leaf))
                        .or_default()
                        .push((shown, color));
                }
            }
        };
        for m in self.find.iter().flat_map(|f| f.other_matches()) {
            decorate(m, match_color);
        }
        decorate(&selection, selection_color);
        let blocks = self
            .doc
            .blocks()
            .blocks
            .iter()
            .map(|block| {
                let leaf = block.leaves.start;
                match &block.kind {
                    Kind::Heading(level) => BlockNode::Heading {
                        level: *level,
                        children: paragraph(leaf),
                        span: None,
                    },
                    Kind::Paragraph | Kind::Empty | Kind::Source => {
                        BlockNode::Paragraph(paragraph(leaf))
                    }
                }
            })
            .collect();
        (blocks, decorations)
    }
}

/// Kit marks for a leaf: the conceal runs flattened into non-overlapping
/// ranges, plus an underline under IME preedit text. A link that resolves
/// (or a web address) gets Kit's link mark, so its colour; an unresolved
/// one keeps the text colour, underlined (Kit has no colour mark to fade
/// it with, as the preview does).
fn marks(
    view: &View,
    marked: Option<Range<usize>>,
    resolves: &dyn Fn(&str) -> bool,
) -> Vec<(Range<usize>, TextMark)> {
    let len = view.display_text().len();
    let runs = view.runs();
    let links: Vec<_> = view
        .links()
        .iter()
        .map(|link| {
            let url = match &link.target {
                Target::Note(target) => resolves(target).then(|| target.clone()),
                Target::Url(url) => Some(url.clone()),
            };
            (link.range.clone(), url)
        })
        .collect();
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
                    Style::Highlight => mark.highlight = Some(HIGHLIGHT),
                    // Set from the link below.
                    Style::Link => {}
                    // Kit's heading block sets the size and weight.
                    Style::Heading(_) => {}
                    Style::Syntax => mark.highlight = Some(SYNTAX_TINT),
                }
            }
            let link = links
                .iter()
                .find(|(range, _)| range.start <= seg.start && seg.end <= range.end);
            match link {
                Some((_, Some(url))) => {
                    mark.link = Some(LinkMark {
                        url: url.clone().into(),
                        ..LinkMark::default()
                    })
                }
                Some((_, None)) => mark.underline = true,
                None => {}
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

impl EntityInputHandler for LivePreview {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.doc.text()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let selection = self.doc.anchored_selection();
        Some(UTF16Selection {
            range: self.range_to_utf16(&selection.range()),
            reversed: selection.head < selection.anchor,
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
        let composing = self.marked_range.take().is_some();
        let range = self.edit_range(range_utf16.as_ref());
        if composing {
            // The commit ends the composition's undo step.
            self.edited(cx, |doc| doc.commit_composition(range, new_text));
        } else if range == self.doc.selection() {
            self.edited(cx, |doc| doc.type_text(new_text));
        } else {
            self.edited(cx, |doc| doc.replace_typing(range, new_text));
        }
        self.update_suggestions(cx);
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
        self.edited(cx, |doc| doc.compose(range.clone(), new_text));
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        // The new selection is relative to the inserted text, in its UTF-16.
        if let Some(sel) = new_selected_range_utf16 {
            let start = range.start + utf16_to_utf8(new_text, sel.start);
            let end = range.start + utf16_to_utf8(new_text, sel.end);
            self.moved(cx, |doc| doc.select(start..end));
        }
        // The preedit counts as typed: it refilters the pop-up.
        self.update_suggestions(cx);
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
        let (at, _) = self.source_for_point(point, |_| true)?;
        Some(self.doc.to_utf16(at))
    }
}

impl LivePreview {
    /// The source range an IME or edit callback targets: the given UTF-16
    /// range, else the marked (preedit) text, else the selection.
    fn edit_range(&self, range_utf16: Option<&Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.doc.selection())
    }

    /// A source byte range in UTF-16 units.
    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.doc.to_utf16(range.start)..self.doc.to_utf16(range.end)
    }

    /// A UTF-16 range as source bytes.
    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.doc.from_utf16(range.start)..self.doc.from_utf16(range.end)
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
    editor: Entity<LivePreview>,
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
        let decorations = std::mem::take(&mut self.decorations);
        let (_, geometry) = collect_leaf_geometry(decorations, || child.prepaint(window, cx));
        *self.editor.read(cx).geometry.borrow_mut() = geometry;
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
        let scrolled = self
            .editor
            .update(cx, |editor, _| editor.scroll_caret_into_view());
        if scrolled {
            window.refresh();
        }
    }
}

impl Render for LivePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A click on a link is the editor's (see `on_mouse_down`).
        let rcx = RenderContext::from_app(cx).on_link_click(|_, _, _, _| {});
        let (blocks, decorations) = self.render_blocks(cx);
        let flows: Vec<FlowPosition> = {
            let refs: Vec<&BlockNode> = blocks.iter().collect();
            (0..refs.len())
                .map(|ix| FlowPosition::at(&refs, ix, ix + 1 == refs.len()))
                .collect()
        };
        let blocks = Rc::new(blocks);
        // Only the blocks the list lays out (those in view) render.
        let content = list(self.list.clone(), move |ix, window, cx| {
            div()
                .px_6()
                .when(ix == 0, |this| this.pt_4())
                .child(blocks[ix].render_with_id(("block", ix), flows[ix], &rcx, window, cx))
                .into_any_element()
        })
        .size_full()
        .into_any_element();
        let find = self.render_find(cx);
        // Focus gone (to the find bar, say), the pop-up goes too.
        if !self.focus_handle.is_focused(window) {
            self.popup = None;
        }
        let suggestions = self.render_suggestions(cx);
        let editor =
            div()
                .flex_1()
                .min_h_0()
                .key_context(KEY_CONTEXT)
                .track_focus(&self.focus_handle)
                .on_action(
                    cx.listener(|this, _: &Search, window, cx| this.open_find(false, window, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &Replace, window, cx| this.open_find(true, window, cx)),
                )
                .cursor(CursorStyle::IBeam)
                .on_action(cx.listener(|this, _: &Backspace, _, cx| {
                    // If the pop-up is open, backspacing refilters it.
                    let suggesting = this.popup.is_some();
                    this.edited(cx, Document::backspace);
                    if suggesting {
                        this.update_suggestions(cx);
                    }
                }))
                .on_action(cx.listener(|this, _: &Delete, _, cx| this.edited(cx, Document::delete)))
                .on_action(cx.listener(|this, _: &DeleteToPreviousWordStart, _, cx| {
                    this.edited(cx, Document::backspace_word)
                }))
                .on_action(cx.listener(|this, _: &DeleteToNextWordEnd, _, cx| {
                    this.edited(cx, Document::delete_word)
                }))
                .on_action(cx.listener(|this, _: &Newline, _, cx| {
                    if !this.accept_chosen_suggestion(cx) {
                        this.edited(cx, Document::newline)
                    }
                }))
                .on_action(cx.listener(|this, _: &InsertTab, _, cx| {
                    if !this.accept_chosen_suggestion(cx) {
                        this.edited(cx, |doc| doc.type_text("\t"))
                    }
                }))
                .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
                    if !this.close_suggestions(cx) {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &Undo, _, cx| {
                    this.edited(cx, |doc| {
                        doc.undo();
                    })
                }))
                .on_action(cx.listener(|this, _: &Redo, _, cx| {
                    this.edited(cx, |doc| {
                        doc.redo();
                    })
                }))
                .on_action(cx.listener(|this, _: &MoveLeft, _, cx| {
                    this.moved(cx, |doc| doc.move_left(false))
                }))
                .on_action(cx.listener(|this, _: &MoveRight, _, cx| {
                    this.moved(cx, |doc| doc.move_right(false))
                }))
                .on_action(cx.listener(|this, _: &SelectLeft, _, cx| {
                    this.moved(cx, |doc| doc.move_left(true))
                }))
                .on_action(cx.listener(|this, _: &SelectRight, _, cx| {
                    this.moved(cx, |doc| doc.move_right(true))
                }))
                .on_action(cx.listener(|this, _: &MoveUp, _, cx| {
                    if !this.step_suggestion(false, cx) {
                        this.vertical(false, false, cx)
                    }
                }))
                .on_action(cx.listener(|this, _: &MoveDown, _, cx| {
                    if !this.step_suggestion(true, cx) {
                        this.vertical(true, false, cx)
                    }
                }))
                .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.vertical(false, true, cx)))
                .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.vertical(true, true, cx)))
                .on_action(cx.listener(|this, _: &MoveToPreviousWord, _, cx| {
                    this.moved(cx, |doc| doc.move_word_left(false))
                }))
                .on_action(cx.listener(|this, _: &MoveToNextWord, _, cx| {
                    this.moved(cx, |doc| doc.move_word_right(false))
                }))
                .on_action(cx.listener(|this, _: &SelectToPreviousWordStart, _, cx| {
                    this.moved(cx, |doc| doc.move_word_left(true))
                }))
                .on_action(cx.listener(|this, _: &SelectToNextWordEnd, _, cx| {
                    this.moved(cx, |doc| doc.move_word_right(true))
                }))
                .on_action(cx.listener(|this, _: &MoveHome, _, cx| {
                    this.moved(cx, |doc| doc.move_line_start(false))
                }))
                .on_action(cx.listener(|this, _: &MoveEnd, _, cx| {
                    this.moved(cx, |doc| doc.move_line_end(false))
                }))
                .on_action(cx.listener(|this, _: &SelectToStartOfLine, _, cx| {
                    this.moved(cx, |doc| doc.move_line_start(true))
                }))
                .on_action(cx.listener(|this, _: &SelectToEndOfLine, _, cx| {
                    this.moved(cx, |doc| doc.move_line_end(true))
                }))
                .on_action(cx.listener(|this, _: &MoveToStart, _, cx| {
                    this.moved(cx, |doc| doc.move_doc_start(false))
                }))
                .on_action(cx.listener(|this, _: &MoveToEnd, _, cx| {
                    this.moved(cx, |doc| doc.move_doc_end(false))
                }))
                .on_action(cx.listener(|this, _: &SelectToStart, _, cx| {
                    this.moved(cx, |doc| doc.move_doc_start(true))
                }))
                .on_action(cx.listener(|this, _: &SelectToEnd, _, cx| {
                    this.moved(cx, |doc| doc.move_doc_end(true))
                }))
                .on_action(
                    cx.listener(|this, _: &SelectAll, _, cx| this.moved(cx, Document::select_all)),
                )
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(Self::cut))
                .on_action(cx.listener(Self::paste))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_move(cx.listener(Self::on_mouse_move))
                // The pop-up stays at the caret, so scrolling closes it.
                .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                    this.close_suggestions(cx);
                }))
                .child(HostElement {
                    editor: cx.entity(),
                    child: Some(content),
                    decorations,
                });
        // The bar sits outside the editor's key context, so typing in it
        // never reaches the note.
        v_flex()
            .size_full()
            .children(find)
            .child(editor)
            .children(suggestions)
    }
}
