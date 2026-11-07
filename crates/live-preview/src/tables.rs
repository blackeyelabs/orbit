//! Moving around a table, worked out on a note's text: where Tab, Shift+Tab
//! and Enter take the caret from a cell, the row they add past the last
//! one, and what keeps typing and deleting inside a cell. The
//! [`crate::Document`] facade applies it.

use std::ops::Range;

use crate::blocks::{Blocks, Kind, Table, is_table_row};

/// Where a key takes the caret from a cell.
pub(crate) enum CellMove {
    /// To this offset, nothing edited.
    To(usize),
    /// Into a new empty row, added at the table's end: the whole new
    /// text, and the caret in it.
    AddRow { text: String, caret: usize },
}

/// The cell an offset is in, and its table.
struct Cell<'a> {
    table: &'a Table,
    /// The table's cells, row by row: leaf indices.
    rows: Vec<Range<usize>>,
    /// The cell's leaf.
    leaf: usize,
    /// Its row.
    row: usize,
    /// Its column.
    column: usize,
}

/// The cell holding `offset`, if it's in a table.
fn cell_at(blocks: &Blocks, offset: usize) -> Option<Cell<'_>> {
    let leaf = blocks.leaf_at(offset)?;
    let block = blocks.block_of(leaf)?;
    let Kind::Table(table) = &block.kind else {
        return None;
    };
    let rows = table.row_leaves(block.leaves.clone());
    let row = rows.iter().position(|r| r.contains(&leaf))?;
    Some(Cell {
        table,
        column: leaf - rows[row].start,
        rows,
        leaf,
        row,
    })
}

/// The source of the cell `offset` is in, if it's in a table.
pub(crate) fn cell_source(blocks: &Blocks, offset: usize) -> Option<Range<usize>> {
    cell_at(blocks, offset).map(|cell| blocks.leaves[cell.leaf].source.clone())
}

/// Text written over `range` in a cell, kept in it: a `|` escaped, a line
/// break a space. `None` if `range` isn't within one cell.
pub(crate) fn cell_text(blocks: &Blocks, range: Range<usize>, text: &str) -> Option<String> {
    let source = cell_source(blocks, range.start)?;
    (range.end <= source.end).then(|| text.replace('|', "\\|").replace('\n', " "))
}

/// Tab (or Shift+Tab, `back`) in a cell: the end of the next cell (or the
/// previous one), row after row; past the last, a new row's first cell.
/// `None` outside a table, or back from the first cell.
pub(crate) fn tab(text: &str, blocks: &Blocks, caret: usize, back: bool) -> Option<CellMove> {
    let cell = cell_at(blocks, caret)?;
    let first = cell.rows.first()?.start;
    let last = cell.rows.last()?.end.checked_sub(1)?;
    let next = if back {
        (cell.leaf > first).then(|| cell.leaf - 1)?
    } else if cell.leaf < last {
        cell.leaf + 1
    } else {
        return Some(add_row(text, blocks, &cell, 0));
    };
    Some(CellMove::To(blocks.leaves[next].source.end))
}

/// Enter in a cell: the end of the cell below it (or the last one that row
/// has), past rows with none; at the bottom, the same column of a new row.
pub(crate) fn enter(text: &str, blocks: &Blocks, caret: usize) -> Option<CellMove> {
    let cell = cell_at(blocks, caret)?;
    let below = cell.rows[cell.row + 1..].iter().find(|r| !r.is_empty());
    Some(match below {
        Some(below) => {
            let leaf = (below.start + cell.column).min(below.end - 1);
            CellMove::To(blocks.leaves[leaf].source.end)
        }
        None => add_row(text, blocks, &cell, cell.column),
    })
}

/// A new empty row after the table's last line, with the caret in its
/// cell `column`.
fn add_row(text: &str, blocks: &Blocks, cell: &Cell, column: usize) -> CellMove {
    // The table's last line: past its last cell's, any rows with none.
    let last_cell = cell.rows.iter().rev().find(|r| !r.is_empty());
    let mut end = last_cell.map_or(0, |r| blocks.leaves[r.end - 1].source.end);
    end = text[end..].find('\n').map_or(text.len(), |n| end + n);
    while let Some(next) = text.get(end + 1..)
        && is_table_row(next.split('\n').next().unwrap_or_default())
    {
        end = text[end + 1..]
            .find('\n')
            .map_or(text.len(), |n| end + 1 + n);
    }
    let columns = cell.table.align.len();
    // `|  |  |`: each empty cell stands just inside its padding.
    let row = format!("\n|{}", "  |".repeat(columns));
    let mut new = text.to_string();
    new.insert_str(end, &row);
    let caret = end + 3 + 3 * column;
    CellMove::AddRow { text: new, caret }
}
