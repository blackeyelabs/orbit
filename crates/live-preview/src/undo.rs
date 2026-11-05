//! Undo and redo for a [`crate::Document`]: edits group into one step while
//! they're the same kind and come less than [`GROUP_PAUSE`] apart.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use crate::conceal::Selection;

/// A pause in editing at least this long starts a new undo step.
pub const GROUP_PAUSE: Duration = Duration::from_millis(300);

/// What kind of edit a change is: a change of kind starts a new undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// Typing text, Enter included.
    Typing,
    /// Backspace and Delete.
    Deleting,
    /// An IME composition, preedit to commit: one step however long it
    /// pauses, so undo never brings back half-composed text.
    Composing,
    /// Anything else (paste, a format command, a change from Source mode):
    /// always a step of its own.
    Other,
}

/// One replacement, as applied: `range` held `removed` before, and holds
/// `inserted` after.
#[derive(Clone, Debug)]
pub struct Change {
    /// Where it starts.
    pub at: usize,
    /// The text it replaced.
    pub removed: String,
    /// The text it put there.
    pub inserted: String,
}

impl Change {
    /// The range the inserted text takes up.
    pub fn inserted_range(&self) -> Range<usize> {
        self.at..self.at + self.inserted.len()
    }

    /// The range the removed text took up.
    pub fn removed_range(&self) -> Range<usize> {
        self.at..self.at + self.removed.len()
    }
}

/// One undo step: its changes in the order they were made, and the
/// selection before and after.
#[derive(Clone, Debug)]
pub struct Step {
    /// In the order made; undone in reverse.
    pub changes: Vec<Change>,
    /// The selection to restore on undo.
    pub before: Selection,
    /// The selection to restore on redo.
    pub after: Selection,
    kind: EditKind,
    last_edit: Instant,
}

/// The steps to undo and to redo.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl History {
    /// Records a change made at `now`: into the last step if it's the same
    /// kind of edit and close enough in time, else as a new step. Any redo
    /// is lost.
    pub fn record(
        &mut self,
        change: Change,
        kind: EditKind,
        before: Selection,
        after: Selection,
        now: Instant,
    ) {
        self.redo.clear();
        let paused = |last: &Step| now.saturating_duration_since(last.last_edit) >= GROUP_PAUSE;
        if let Some(last) = self.undo.last_mut()
            && kind != EditKind::Other
            && last.kind == kind
            && (kind == EditKind::Composing || !paused(last))
        {
            last.changes.push(change);
            last.after = after;
            last.last_edit = now;
            return;
        }
        self.undo.push(Step {
            changes: vec![change],
            before,
            after,
            kind,
            last_edit: now,
        });
    }

    /// The step to undo, moved to the redo stack.
    pub fn undo(&mut self) -> Option<&Step> {
        let step = self.undo.pop()?;
        self.redo.push(step);
        self.redo.last()
    }

    /// The step to redo, moved back to the undo stack.
    pub fn redo(&mut self) -> Option<&Step> {
        let step = self.redo.pop()?;
        self.undo.push(step);
        // Typing after a redo starts a step of its own.
        self.end_step();
        self.undo.last()
    }

    /// Closes the last step: the next edit starts a new one.
    pub fn end_step(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.kind = EditKind::Other;
        }
    }

    /// Forgets every step.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}
