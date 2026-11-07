//! The facade: a note being edited in Live Preview.

use std::{ops::Range, time::Instant};

use ropey::{LineType, Rope};

use crate::blocks::{
    Block, Blocks, Code, Item, Kind, Shows, next_grapheme, opening_fence, prev_grapheme,
};
use crate::conceal::{
    Layout, Link, Run, Selection, layout_code, layout_line, layout_math, layout_titled,
};
use crate::undo::{Change, EditKind, History};

/// Where the time comes from, for grouping undo steps.
pub type Clock = Box<dyn Fn() -> Instant>;

/// A selection no span can touch, so a leaf lays out fully concealed.
const NOTHING_REVEALED: Selection = Selection {
    anchor: usize::MAX,
    head: usize::MAX,
};

/// A note being edited in Live Preview: its source in a rope, the
/// selection, undo, and the split into blocks and leaves.
pub struct Document {
    /// The buffer every edit goes into.
    rope: Rope,
    /// The rope's text as one string, kept in step with it, for what reads
    /// a whole note at once: the block parse and the conceal model. It
    /// goes once blocks reparse incrementally from the rope (#55).
    text: String,
    blocks: Blocks,
    selection: Selection,
    history: History,
    clock: Clock,
}

impl Document {
    /// A note's `text` to edit, timed by the system clock, the caret at
    /// its start.
    pub fn new(text: &str) -> Self {
        Self::with_clock(text, Box::new(Instant::now))
    }

    /// A note's `text` to edit, timed by `clock`.
    pub fn with_clock(text: &str, clock: Clock) -> Self {
        let mut doc = Document {
            rope: Rope::from_str(text),
            text: text.to_string(),
            blocks: Blocks::parse(text),
            selection: Selection::caret(0),
            history: History::default(),
            clock,
        };
        doc.set_caret(0);
        doc
    }

    /// The note's source as it is now.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The note split into blocks and leaves.
    pub fn blocks(&self) -> &Blocks {
        &self.blocks
    }

    /// The caret: the selection's moving end.
    pub fn caret(&self) -> usize {
        self.selection.head
    }

    /// The selection, start first; empty for a caret.
    pub fn selection(&self) -> Range<usize> {
        self.selection.range()
    }

    /// The selection with its direction: where it started, and the caret.
    pub fn anchored_selection(&self) -> Selection {
        self.selection
    }

    /// Puts the caret at `offset`, snapped to where a caret may stand.
    pub fn set_caret(&mut self, offset: usize) {
        let offset = self.snap(offset, false);
        self.selection = Selection::caret(offset);
    }

    /// Selects `range`, the caret at its end.
    pub fn select(&mut self, range: Range<usize>) {
        self.set_selection(Selection::spanning(range));
    }

    /// Sets the selection as is, each end snapped to where a caret may
    /// stand.
    pub fn set_selection(&mut self, selection: Selection) {
        self.selection = Selection {
            anchor: self.snap(selection.anchor, false),
            head: self.snap(selection.head, selection.head < selection.anchor),
        };
    }

    /// Moves the caret to `offset`, or with `extend`, the selection's head.
    pub fn move_to(&mut self, offset: usize, extend: bool) {
        if extend {
            self.set_selection(Selection {
                anchor: self.selection.anchor,
                head: offset,
            });
        } else {
            self.set_caret(offset);
        }
    }

    /// Snaps `offset` (clamped to the text) to where a caret may stand.
    fn snap(&self, offset: usize, back: bool) -> usize {
        let offset = self.rope.floor_char_boundary(offset.min(self.text.len()));
        self.blocks.snap(offset, back)
    }

    /// Types `text` over the selection. Into an empty line at the end of a
    /// block, it goes on a line of its own.
    pub fn type_text(&mut self, text: &str) {
        let range = self.selection.range();
        let in_cell = crate::tables::cell_text(&self.blocks, range.clone(), text);
        let text = in_cell.as_deref().unwrap_or(text);
        let before = if range.is_empty() && !text.is_empty() {
            self.line_break_before(range.start)
        } else {
            ""
        };
        let inserted = format!("{before}{text}");
        let caret = range.start + inserted.len();
        self.edit(range, &inserted, EditKind::Typing, Selection::caret(caret));
    }

    /// Enter: a line break over the selection. At the end of a code
    /// block's opening fence that nothing closes yet (```` ``` ```` just
    /// typed), it closes the block, naming its language `txt` if none is,
    /// and puts the caret on the empty line inside.
    pub fn newline(&mut self) {
        let caret = self.caret();
        match self.opening_fence_ended_at(caret) {
            Some(code) if self.selection.range().is_empty() => {
                let named = if code.lang.is_some() { "" } else { "txt" };
                let inserted = format!("{named}\n\n```");
                let after = Selection::caret(caret + named.len() + 1);
                self.edit(caret..caret, &inserted, EditKind::Typing, after);
            }
            _ => {
                if !self.move_in_table(crate::tables::enter) && !self.continue_list() {
                    self.type_text("\n");
                }
            }
        }
    }

    /// Tab (or Shift+Tab, `back`) in a table cell: the caret goes to the
    /// end of the next cell (or the previous one), row after row; Tab in
    /// the last cell adds a row (one undo step) and goes into it. Says
    /// whether the caret was in a table.
    pub fn tab_in_table(&mut self, back: bool) -> bool {
        self.move_in_table(|text, blocks, caret| crate::tables::tab(text, blocks, caret, back))
    }

    /// Makes the move `to` works out from the caret in a table cell, with
    /// no selection; says whether there was one.
    fn move_in_table(
        &mut self,
        to: impl FnOnce(&str, &Blocks, usize) -> Option<crate::tables::CellMove>,
    ) -> bool {
        if !self.selection.range().is_empty() {
            return false;
        }
        match to(&self.text, &self.blocks, self.caret()) {
            Some(crate::tables::CellMove::To(offset)) => self.set_caret(offset),
            Some(crate::tables::CellMove::AddRow { text, caret }) => {
                let selection = Selection::caret(caret);
                self.take_edit(crate::Edited { text, selection }, EditKind::Other);
            }
            None => return false,
        }
        true
    }

    /// Enter in a list item or quote line, with no selection: the next item
    /// (or line) begins, the rest of the line going into it, and a numbered
    /// list renumbers; in an empty one, its marker goes, ending the list.
    /// One undo step. Says whether the caret was in one.
    pub fn continue_list(&mut self) -> bool {
        if !self.selection.range().is_empty() {
            return false;
        }
        match crate::lists::enter(&self.text, &self.blocks, self.caret()) {
            Some(edited) => {
                self.take_edit(edited, EditKind::Typing);
                true
            }
            None => false,
        }
    }

    /// Tab: every list item the selection touches nests a tab deeper, and
    /// its list renumbers; one undo step. Says whether one did; if not,
    /// nothing changes (Tab types a tab instead).
    pub fn indent(&mut self) -> bool {
        self.nest(false)
    }

    /// Shift+Tab: every list item the selection touches moves a level out
    /// (a tab, or up to 4 spaces, fewer), and its list renumbers; one undo
    /// step. Says whether one did.
    pub fn outdent(&mut self) -> bool {
        self.nest(true)
    }

    /// [`Self::indent`], or with `out` [`Self::outdent`].
    fn nest(&mut self, out: bool) -> bool {
        match crate::lists::indent(&self.text, &self.blocks, self.selection, out) {
            Some(edited) => {
                self.take_edit(edited, EditKind::Other);
                true
            }
            None => false,
        }
    }

    /// Makes a list edit: the part of the text that changed, as one edit
    /// of `kind`, then its selection.
    fn take_edit(&mut self, edited: crate::Edited, kind: EditKind) {
        let (range, inserted) = crate::differing(&self.text, &edited.text);
        let inserted = inserted.to_string();
        self.edit(range, &inserted, kind, edited.selection);
    }

    /// The code block whose opening fence ends at `offset`, if it's one
    /// just typed: nothing closes it, or only the closing fence of a block
    /// below (its body holds that block's opening fence).
    fn opening_fence_ended_at(&self, offset: usize) -> Option<&Code> {
        let leaf = self.blocks.leaf_at(offset)?;
        let Some(Block {
            kind: Kind::Code(code),
            ..
        }) = self.blocks.block_of(leaf)
        else {
            return None;
        };
        let source = &self.blocks.leaves[leaf].source;
        let text = &self.text[source.clone()];
        let mut lines = text.split('\n');
        let fence = lines.next().unwrap_or_default();
        let swallowed = lines.any(|line| opening_fence(line).is_some_and(|f| !f.lang.is_empty()));
        let new = code.close.is_none() || swallowed;
        (new && offset == source.start + fence.len()).then_some(code)
    }

    /// Pastes `text` over the selection: an undo step of its own.
    pub fn paste(&mut self, text: &str) {
        let range = self.selection.range();
        let in_cell = crate::tables::cell_text(&self.blocks, range.clone(), text);
        let text = in_cell.as_deref().unwrap_or(text);
        let caret = range.start + text.len();
        self.edit(range, text, EditKind::Other, Selection::caret(caret));
    }

    /// Backspace: deletes the selection, or the character before the caret.
    pub fn backspace(&mut self) {
        self.delete_to(|doc, caret| doc.prev_stop(caret));
    }

    /// Delete: deletes the selection, or the character after the caret.
    pub fn delete(&mut self) {
        self.delete_to(|doc, caret| doc.next_stop(caret));
    }

    /// Ctrl+Backspace: deletes the selection, or back to the word's start.
    pub fn backspace_word(&mut self) {
        self.delete_to(|doc, caret| doc.word_left(caret));
    }

    /// Ctrl+Delete: deletes the selection, or on to the word's end.
    pub fn delete_word(&mut self) {
        self.delete_to(|doc, caret| doc.word_right(caret));
    }

    /// Deletes the selection, or with none, from the caret to where `to`
    /// says, either way; the caret ends at the start.
    fn delete_to(&mut self, to: impl FnOnce(&Self, usize) -> usize) {
        let mut range = self.selection.range();
        if range.is_empty() {
            let caret = self.caret();
            let end = to(self, caret);
            range = caret.min(end)..caret.max(end);
            // In a table cell, never past its edges: that would take a
            // pipe and merge two cells.
            if let Some(cell) = crate::tables::cell_source(&self.blocks, caret) {
                range = range.start.max(cell.start)..range.end.min(cell.end);
            }
        }
        if range.is_empty() {
            return;
        }
        let start = range.start;
        self.edit(range, "", EditKind::Deleting, Selection::caret(start));
    }

    /// Replaces `range` with `text` and selects `selection` (a format
    /// command, say): an undo step of its own.
    pub fn replace(&mut self, range: Range<usize>, text: &str, selection: Range<usize>) {
        self.edit(range, text, EditKind::Other, Selection::spanning(selection));
    }

    /// Replaces every one of `matches` (in order, not overlapping, as
    /// [`Self::find`] gives them) with `text`, as one undo step: one
    /// change from the first match to the last. The caret ends after the
    /// last replacement. Matches the text no longer has (found before it
    /// changed) are left alone.
    pub fn replace_all(&mut self, matches: &[Range<usize>], text: &str) {
        let fits = |m: &Range<usize>| self.text.get(m.clone()).is_some();
        let in_order = matches.windows(2).all(|w| w[0].end <= w[1].start);
        if !in_order || !matches.iter().all(fits) {
            return;
        }
        let (Some(first), Some(last)) = (matches.first(), matches.last()) else {
            return;
        };
        let span = first.start..last.end;
        let mut replaced = String::with_capacity(span.len());
        let mut at = span.start;
        for m in matches {
            replaced.push_str(&self.text[at..m.start]);
            replaced.push_str(text);
            at = m.end;
        }
        let caret = span.start + replaced.len();
        self.edit(span, &replaced, EditKind::Other, Selection::caret(caret));
    }

    /// Ticks the task that is block `block`, or unticks it: `[ ]` and
    /// `[x]` swap in the source, an undo step of its own, the selection
    /// where it was. Says whether the block is a task.
    pub fn toggle_task(&mut self, block: usize) -> bool {
        let Some(Block {
            kind: Kind::ListItem(Item {
                task: Some(ticked), ..
            }),
            leaves,
        }) = self.blocks.blocks.get(block)
        else {
            return false;
        };
        let leaf = &self.blocks.leaves[leaves.start];
        let Shows::Prose { marker } = &leaf.shows else {
            return false;
        };
        let marker = &self.text[leaf.source.start..leaf.source.start + *marker];
        let Some(open) = marker.find('[') else {
            return false;
        };
        let at = leaf.source.start + open + 1;
        let mark = if *ticked { " " } else { "x" };
        self.edit(at..at + 1, mark, EditKind::Other, self.selection);
        true
    }

    /// Names block `block`'s language `lang` (the language picker): the
    /// first word after its opening fence becomes `lang`, an undo step of
    /// its own, the selection staying on the same text. Says whether the
    /// block is code.
    pub fn set_code_language(&mut self, block: usize, lang: &str) -> bool {
        let Some(Block {
            kind: Kind::Code(_),
            leaves,
        }) = self.blocks.blocks.get(block)
        else {
            return false;
        };
        let start = self.blocks.leaves[leaves.start].source.start;
        let line = self.text[start..].split('\n').next().unwrap_or_default();
        let Some(fence) = opening_fence(line) else {
            return false;
        };
        let range = start + fence.lang.start..start + fence.lang.end;
        let shift = |o: usize| {
            if o >= range.end {
                o - range.len() + lang.len()
            } else {
                o
            }
        };
        let after = Selection {
            anchor: shift(self.selection.anchor),
            head: shift(self.selection.head),
        };
        self.edit(range, lang, EditKind::Other, after);
        true
    }

    /// Replaces `range` with `text` as typing, putting the caret after it.
    pub fn replace_typing(&mut self, range: Range<usize>, text: &str) {
        let in_cell = crate::tables::cell_text(&self.blocks, range.clone(), text);
        let text = in_cell.as_deref().unwrap_or(text);
        let caret = range.start + text.len();
        self.edit(range, text, EditKind::Typing, Selection::caret(caret));
    }

    /// An IME composition's preedit: replaces `range` (the preedit so
    /// far) with `text`, the caret after it. From the first preedit to the
    /// commit, a composition is one undo step.
    pub fn compose(&mut self, range: Range<usize>, text: &str) {
        let in_cell = crate::tables::cell_text(&self.blocks, range.clone(), text);
        let text = in_cell.as_deref().unwrap_or(text);
        let caret = range.start + text.len();
        self.edit(range, text, EditKind::Composing, Selection::caret(caret));
    }

    /// An IME composition's commit: replaces the preedit in `range` with
    /// `text`, ending the composition's undo step.
    pub fn commit_composition(&mut self, range: Range<usize>, text: &str) {
        self.compose(range, text);
        self.history.end_step();
    }

    /// Takes the text as edited elsewhere (Source mode): one undo step for
    /// the part that differs, the caret after it.
    pub fn take_text(&mut self, text: &str) {
        let (range, inserted) = crate::differing(&self.text, text);
        if range.is_empty() && inserted.is_empty() {
            return;
        }
        let caret = range.start + inserted.len();
        self.edit(range, inserted, EditKind::Other, Selection::caret(caret));
    }

    /// Replaces the whole text with `text` from disk. It isn't an edit, so
    /// there's nothing to undo before it.
    pub fn load(&mut self, text: &str) {
        let caret = self.caret();
        self.rope = Rope::from_str(text);
        self.text = text.to_string();
        self.blocks = Blocks::parse(text);
        self.history.clear();
        self.set_caret(caret);
    }

    /// Forgets the undo history (a protected folder locked).
    pub fn forget_history(&mut self) {
        self.history.clear();
    }

    /// Undoes the last step; whether there was one.
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.history.undo() else {
            return false;
        };
        let (changes, before) = (step.changes.clone(), step.before);
        for change in changes.iter().rev() {
            self.apply(change.inserted_range(), &change.removed);
        }
        self.set_selection(before);
        true
    }

    /// Redoes the last step undone; whether there was one.
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.history.redo() else {
            return false;
        };
        let (changes, after) = (step.changes.clone(), step.after);
        for change in &changes {
            self.apply(change.removed_range(), &change.inserted);
        }
        self.set_selection(after);
        true
    }

    /// Replaces `range` with `text` as an edit of `kind`, recorded for
    /// undo, then selects `after`.
    fn edit(&mut self, range: Range<usize>, text: &str, kind: EditKind, after: Selection) {
        let removed = self.text[range.clone()].to_string();
        if removed.is_empty() && text.is_empty() {
            return;
        }
        let before = self.selection;
        self.apply(range.clone(), text);
        self.set_selection(after);
        let change = Change {
            at: range.start,
            removed,
            inserted: text.to_string(),
        };
        let now = (self.clock)();
        self.history
            .record(change, kind, before, self.selection, now);
    }

    /// Replaces `range` with `text` in the buffer and reparses.
    fn apply(&mut self, range: Range<usize>, text: &str) {
        self.rope.remove(range.clone());
        self.rope.insert(range.start, text);
        self.text.replace_range(range, text);
        self.blocks = Blocks::parse(&self.text);
    }

    /// Text typed at `offset` into an empty line right after a block that
    /// ends the note goes on a new line, not onto the block's last line.
    fn line_break_before(&self, offset: usize) -> &'static str {
        let empty_line = self
            .blocks
            .leaf_at(offset)
            .filter(|&l| self.blocks.leaves[l].source.is_empty())
            .and_then(|l| self.blocks.block_of(l))
            .is_some_and(|b| b.kind == Kind::Empty);
        if empty_line && offset > 0 && !self.text[..offset].ends_with('\n') {
            "\n"
        } else {
            ""
        }
    }

    /// The caret stop after `offset` (Right).
    pub fn next_stop(&self, offset: usize) -> usize {
        self.blocks.next_stop(&self.text, offset)
    }

    /// The caret stop before `offset` (Left).
    pub fn prev_stop(&self, offset: usize) -> usize {
        self.blocks.prev_stop(&self.text, offset)
    }

    /// Right: one stop on, or to the selection's end (with `extend`, the
    /// selection's head one stop on).
    pub fn move_right(&mut self, extend: bool) {
        match (extend, self.selection.range()) {
            (false, r) if !r.is_empty() => self.set_caret(r.end),
            _ => self.move_to(self.next_stop(self.caret()), extend),
        }
    }

    /// Left: one stop back, or to the selection's start.
    pub fn move_left(&mut self, extend: bool) {
        match (extend, self.selection.range()) {
            (false, r) if !r.is_empty() => self.set_caret(r.start),
            _ => self.move_to(self.prev_stop(self.caret()), extend),
        }
    }

    /// Ctrl+Right: to the end of this or the next word.
    pub fn move_word_right(&mut self, extend: bool) {
        self.move_to(self.word_right(self.caret()), extend);
    }

    /// Ctrl+Left: to the start of this or the previous word.
    pub fn move_word_left(&mut self, extend: bool) {
        self.move_to(self.word_left(self.caret()), extend);
    }

    /// Home: the start of the caret's line, within its leaf.
    pub fn move_line_start(&mut self, extend: bool) {
        self.move_to(self.line_range(self.caret()).start, extend);
    }

    /// End: the end of the caret's line, within its leaf.
    pub fn move_line_end(&mut self, extend: bool) {
        self.move_to(self.line_range(self.caret()).end, extend);
    }

    /// Ctrl+Home: the start of the note.
    pub fn move_doc_start(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    /// Ctrl+End: the end of the note.
    pub fn move_doc_end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend);
    }

    /// Selects the whole note.
    pub fn select_all(&mut self) {
        let end = self.snap(self.text.len(), true);
        self.set_selection(Selection {
            anchor: self.snap(0, false),
            head: end,
        });
    }

    /// Where Ctrl+Right from `offset` goes: past any spaces and
    /// punctuation, then to the end of the word; from a leaf's end, to the
    /// next leaf.
    fn word_right(&self, offset: usize) -> usize {
        let Some(leaf) = self.blocks.leaf_at(offset) else {
            return self.next_stop(offset);
        };
        let end = self.blocks.leaves[leaf].source.end;
        if offset == end {
            return self.next_stop(offset);
        }
        let mut at = offset;
        while at < end && !is_word_at(&self.text, at) {
            at = next_grapheme(&self.text, at);
        }
        while at < end && is_word_at(&self.text, at) {
            at = next_grapheme(&self.text, at);
        }
        at.min(end)
    }

    /// Where Ctrl+Left from `offset` goes: back past spaces and
    /// punctuation, then to the start of the word; from a leaf's start, to
    /// the previous leaf.
    fn word_left(&self, offset: usize) -> usize {
        let Some(leaf) = self.blocks.leaf_at(offset) else {
            return self.prev_stop(offset);
        };
        let start = self.blocks.leaves[leaf].source.start;
        if offset == start {
            return self.prev_stop(offset);
        }
        let mut at = offset;
        while at > start && !is_word_at(&self.text, prev_grapheme(&self.text, at)) {
            at = prev_grapheme(&self.text, at);
        }
        while at > start && is_word_at(&self.text, prev_grapheme(&self.text, at)) {
            at = prev_grapheme(&self.text, at);
        }
        at.max(start)
    }

    /// The word at `offset` (a double-click): the run of letters and
    /// digits it's in or touches, else the one character there.
    pub fn word_at(&self, offset: usize) -> Range<usize> {
        let offset = self.snap(offset, false);
        let Some(leaf) = self.blocks.leaf_at(offset) else {
            return offset..offset;
        };
        let bounds = self.blocks.leaves[leaf].source.clone();
        let in_word = |at: usize| at < bounds.end && is_word_at(&self.text, at);
        let mut start = offset;
        let mut end = offset;
        if in_word(offset) || (offset > bounds.start && in_word(prev_grapheme(&self.text, offset)))
        {
            while start > bounds.start && in_word(prev_grapheme(&self.text, start)) {
                start = prev_grapheme(&self.text, start);
            }
            while in_word(end) {
                end = next_grapheme(&self.text, end);
            }
        } else if offset < bounds.end {
            end = next_grapheme(&self.text, offset);
        }
        start..end
    }

    /// The source line `offset` is on, within its leaf (a triple-click,
    /// Home and End).
    pub fn line_range(&self, offset: usize) -> Range<usize> {
        let offset = self.snap(offset, false);
        let line = self.rope.byte_to_line_idx(offset, LineType::LF);
        let start = self.rope.line_to_byte_idx(line, LineType::LF);
        let end = self.text[start..]
            .find('\n')
            .map_or(self.text.len(), |n| start + n);
        match self.blocks.leaf_at(offset) {
            Some(leaf) => {
                let leaf = &self.blocks.leaves[leaf].source;
                start.max(leaf.start)..end.min(leaf.end)
            }
            None => offset..offset,
        }
    }

    /// The caret's line and column in the source, from zero: the column in
    /// characters.
    pub fn line_column(&self) -> (u32, u32) {
        let caret = self.caret();
        let line = self.rope.byte_to_line_idx(caret, LineType::LF);
        let start = self.rope.line_to_byte_idx(line, LineType::LF);
        let column = self.text[start..caret].chars().count();
        (line as u32, column as u32)
    }

    /// Where `line` (from zero) starts, or the note's end past its last.
    pub fn line_start(&self, line: u32) -> usize {
        let line = (line as usize).min(self.rope.len_lines(LineType::LF) - 1);
        self.rope.line_to_byte_idx(line, LineType::LF)
    }

    /// A byte offset in UTF-16 units, as platform input (IME) counts.
    pub fn to_utf16(&self, offset: usize) -> usize {
        self.rope.byte_to_utf16_idx(offset.min(self.text.len()))
    }

    /// A UTF-16 offset as a byte offset.
    pub fn from_utf16(&self, offset: usize) -> usize {
        self.rope
            .utf16_to_byte_idx(offset.min(self.rope.len_utf16()))
    }

    /// Every match of `query` in the source, in order and not overlapping;
    /// case is ignored unless `match_case`. An empty query matches nothing.
    pub fn find(&self, query: &str, match_case: bool) -> Vec<Range<usize>> {
        let mut found = Vec::new();
        if query.is_empty() {
            return found;
        }
        let text = &self.text;
        let mut at = 0;
        while at < text.len() {
            match match_len(&text[at..], query, match_case) {
                Some(len) => {
                    found.push(at..at + len);
                    at += len;
                }
                None => at += text[at..].chars().next().map_or(1, char::len_utf8),
            }
        }
        found
    }

    /// Whether the selection reaches leaf `leaf`, so syntax in it can show:
    /// a list item's or quote's marker shows anywhere on its line, and a
    /// callout's anywhere in the callout.
    pub fn reveals(&self, leaf: usize) -> bool {
        self.blocks
            .reveal_ranges()
            .get(leaf)
            .is_some_and(|r| self.selection.touches(r))
    }

    /// What each leaf shows for the current selection. A leaf the
    /// selection doesn't reach lays out fully concealed; a source block's
    /// leaf is verbatim.
    pub fn views(&self) -> Vec<View> {
        let selection = self.selection;
        let reveal = self.blocks.reveal_ranges();
        self.blocks
            .leaves
            .iter()
            .zip(reveal)
            .map(|(leaf, reveal)| {
                let text = &self.text[leaf.source.clone()];
                let local = if selection.touches(&reveal) {
                    let rel =
                        |o: usize| o.clamp(leaf.source.start, leaf.source.end) - leaf.source.start;
                    Selection {
                        anchor: rel(selection.anchor),
                        head: rel(selection.head),
                    }
                } else {
                    NOTHING_REVEALED
                };
                match &leaf.shows {
                    Shows::Prose { marker } => View::Concealed(layout_line(text, *marker, local)),
                    Shows::Title { marker, title } => {
                        View::Concealed(layout_titled(text, *marker, title.as_deref(), local))
                    }
                    Shows::Code { fence } => View::Concealed(layout_code(text, *fence, local)),
                    Shows::Math => View::Concealed(layout_math(text, local)),
                    Shows::Source => View::Verbatim(text.to_string()),
                }
            })
            .collect()
    }
}

/// How long the match of `query` at the start of `text` is, if there is
/// one: character by character, case ignored unless `match_case`.
fn match_len(text: &str, query: &str, match_case: bool) -> Option<usize> {
    let mut chars = text.char_indices();
    for q in query.chars() {
        let (_, c) = chars.next()?;
        let same = c == q || (!match_case && c.to_lowercase().eq(q.to_lowercase()));
        if !same {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(i, _)| i))
}

/// Whether the grapheme at `at` is part of a word: a letter, digit or `_`.
fn is_word_at(text: &str, at: usize) -> bool {
    text[at..]
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// What one leaf shows: its text with syntax concealed, or verbatim.
/// Offsets are relative to the leaf.
#[derive(Debug)]
pub enum View {
    /// Prose, laid out by the conceal model.
    Concealed(Layout),
    /// Source shown as typed.
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

    /// The links in the text, in display offsets.
    pub fn links(&self) -> &[Link] {
        match self {
            View::Concealed(l) => l.links(),
            View::Verbatim(_) => &[],
        }
    }

    /// The math in the text, in display offsets.
    pub fn maths(&self) -> &[crate::conceal::Math] {
        match self {
            View::Concealed(l) => l.maths(),
            View::Verbatim(_) => &[],
        }
    }

    /// The images in the text, in display offsets.
    pub fn images(&self) -> &[crate::conceal::Image] {
        match self {
            View::Concealed(l) => l.images(),
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
