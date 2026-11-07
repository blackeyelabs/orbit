//! List editing, worked out on a note's text: what Enter does in a list
//! item or quote line, what Tab and Shift+Tab do to list items, and the
//! renumbering after. Each gives the whole new text and where the
//! selection goes; the [`crate::Document`] facade applies it as one edit.

use std::ops::Range;

use crate::Edited;
use crate::blocks::{Blocks, Item, Kind, Shows};
use crate::conceal::Selection;

/// How much a level out takes from the start of `line`: a tab, or up to 4
/// spaces.
fn outdent_width(line: &str) -> usize {
    if line.starts_with('\t') {
        1
    } else {
        line.bytes().take(4).take_while(|&b| b == b' ').count()
    }
}

/// Where the line holding `offset` starts.
fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |n| n + 1)
}

/// The marker the next item after one written `marker` starts with: the
/// same bullet, or the next number, and an unticked box for a task.
fn next_marker(marker: &str, item: &Item) -> String {
    let rest = match item.number {
        Some(n) => {
            let digits = marker.bytes().take_while(u8::is_ascii_digit).count();
            let delimiter = &marker[digits..digits + 1];
            format!("{}{delimiter} ", n + 1)
        }
        None => format!("{} ", &marker[..1]),
    };
    match item.task {
        Some(_) => format!("{rest}[ ] "),
        None => rest,
    }
}

/// Enter at `caret` in a list item or quote line: the next item (or quote
/// line) begins after it, the rest of the line going with it. An empty
/// nested item moves out a level instead, and an empty one at the edge
/// loses its marker, ending the list or quote there. `None` elsewhere.
pub(crate) fn enter(text: &str, blocks: &Blocks, caret: usize) -> Option<Edited> {
    let leaf_ix = blocks.leaf_at(caret)?;
    let leaf = &blocks.leaves[leaf_ix];
    let Shows::Prose { marker } = leaf.shows else {
        return None;
    };
    let kind = &blocks.block_of(leaf_ix)?.kind;
    if !matches!(kind, Kind::ListItem(_) | Kind::Quote(_)) || marker == 0 {
        return None;
    }
    let start = line_start(text, leaf.source.start);
    let marker_end = leaf.source.start + marker;
    if caret < marker_end {
        return None;
    }
    if text[marker_end..leaf.source.end].trim().is_empty() {
        let out = outdent_width(&text[start..]);
        let (cleared, mut caret) = if out > 0 {
            (start..start + out, caret - out)
        } else {
            (start..leaf.source.end, start)
        };
        let mut new = text.to_string();
        new.replace_range(cleared, "");
        let moved = [start];
        let new = renumber(&new, start..start, &moved, &mut [&mut caret]);
        return Some(Edited {
            text: new,
            selection: Selection::caret(caret),
        });
    }
    let indent = &text[start..leaf.source.start];
    let written = &text[leaf.source.start..marker_end];
    let next = match kind {
        Kind::ListItem(item) => next_marker(written, item),
        _ => written.to_string(),
    };
    let inserted = format!("\n{indent}{next}");
    // The rest of the line starts the new item, without the spaces before it.
    let spaces =
        text[caret..leaf.source.end].len() - text[caret..leaf.source.end].trim_start().len();
    let mut new = text.to_string();
    new.replace_range(caret..caret + spaces, &inserted);
    let mut caret = caret + inserted.len();
    let new = renumber(&new, caret..caret, &[], &mut [&mut caret]);
    Some(Edited {
        text: new,
        selection: Selection::caret(caret),
    })
}

/// Tab (or Shift+Tab, `out`) with `selection`: every list item it touches
/// nests a tab deeper (or a level out: a tab, or up to 4 spaces, fewer).
/// A line a selection ends at the start of isn't touched. `None` if no
/// item moves, so Tab types a tab.
pub(crate) fn indent(
    text: &str,
    blocks: &Blocks,
    selection: Selection,
    out: bool,
) -> Option<Edited> {
    let range = selection.range();
    let items: Vec<usize> = blocks
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, Kind::ListItem(_)))
        .map(|b| blocks.leaves[b.leaves.start].source.clone())
        .filter(|leaf| {
            let line = line_start(text, leaf.start)..leaf.end;
            let ends_before = !range.is_empty() && range.end == line.start;
            line.start <= range.end && range.start <= line.end && !ends_before
        })
        .map(|leaf| line_start(text, leaf.start))
        .collect();
    if items.is_empty() {
        return None;
    }
    // Each line's change, at its start in `text`: a tab added, or this
    // many bytes taken.
    let changes: Vec<(usize, isize)> = items
        .into_iter()
        .map(|line| match out {
            true => (line, -(outdent_width(&text[line..]) as isize)),
            false => (line, 1),
        })
        .filter(|&(_, change)| change != 0)
        .collect();
    if changes.is_empty() {
        return None;
    }
    let mut new = String::with_capacity(text.len() + changes.len());
    let mut at = 0;
    for &(line, change) in &changes {
        new.push_str(&text[at..line]);
        at = line;
        if change > 0 {
            new.push('\t');
        } else {
            at += change.unsigned_abs();
        }
    }
    new.push_str(&text[at..]);
    // Where an offset into `text` is in `new`.
    let moved = |o: usize| {
        let shift: isize = changes
            .iter()
            .map(|&(line, change)| match change {
                1 if o >= line => 1,
                c if c < 0 => -((o.clamp(line, line + c.unsigned_abs()) - line) as isize),
                _ => 0,
            })
            .sum();
        (o as isize + shift) as usize
    };
    let (mut anchor, mut head) = (moved(selection.anchor), moved(selection.head));
    // An item moved to a level it's first at starts it at 1.
    let moved_items: Vec<usize> = changes
        .iter()
        // Its own tab is after its line's start.
        .map(|&(line, change)| moved(line) - usize::from(change > 0))
        .collect();
    let span = anchor.min(head)..anchor.max(head);
    let new = renumber(&new, span, &moved_items, &mut [&mut anchor, &mut head]);
    Some(Edited {
        text: new,
        selection: Selection { anchor, head },
    })
}

/// Renumbers every list the source range `span` reaches: at each level,
/// each numbered item after the first takes the number after the one
/// before it at that level. The first keeps its number, unless its line
/// starts at one of `moved` (an item just nested or unnested): then it's
/// 1. `offsets` (into `text`) are moved to where they are in the result.
fn renumber(text: &str, span: Range<usize>, moved: &[usize], offsets: &mut [&mut usize]) -> String {
    let blocks = Blocks::parse(text);
    let is_list = |kind: &Kind| matches!(kind, Kind::ListItem(_) | Kind::Empty);
    let start_of = |b: usize| blocks.leaves[blocks.blocks[b].leaves.start].source.start;
    // From the start of the list holding the span's start.
    let here = blocks.block_at(span.start);
    let first = (0..=here)
        .rev()
        .take_while(|&b| is_list(&blocks.blocks[b].kind))
        .last()
        .unwrap_or(here);
    // The number last given at each level, while that level is numbered.
    let mut counts: Vec<Option<u32>> = Vec::new();
    let mut changes: Vec<(Range<usize>, String)> = Vec::new();
    for (ix, block) in blocks.blocks.iter().enumerate().skip(first) {
        let item = match &block.kind {
            Kind::ListItem(item) => item,
            Kind::Empty => continue,
            // A list ends; past the span, so does the renumbering.
            _ if start_of(ix) > span.end => break,
            _ => {
                counts.clear();
                continue;
            }
        };
        counts.truncate(item.depth + 1);
        counts.resize(item.depth + 1, None);
        let Some(number) = item.number else {
            counts[item.depth] = None;
            continue;
        };
        let at = blocks.leaves[block.leaves.start].source.start;
        let first = if moved.contains(&line_start(text, at)) {
            1
        } else {
            number
        };
        let wanted = counts[item.depth].map_or(first, |n| n + 1);
        counts[item.depth] = Some(wanted);
        if wanted != number {
            let digits = text[at..].bytes().take_while(u8::is_ascii_digit).count();
            changes.push((at..at + digits, wanted.to_string()));
        }
    }
    let mut new = text.to_string();
    for (range, number) in changes.iter().rev() {
        new.replace_range(range.clone(), number);
        let delta = number.len() as isize - range.len() as isize;
        for offset in offsets.iter_mut() {
            if **offset >= range.end {
                **offset = (**offset as isize + delta) as usize;
            }
        }
    }
    new
}
