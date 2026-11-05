//! The Live Preview document model, GPUI-free: a note's text in a rope,
//! the selection, undo, the split into blocks and leaves, and the conceal
//! model that decides what each leaf shows. [`Document`] is its facade;
//! the `editor` crate draws one.
//!
//! Every offset is a UTF-8 byte offset into the note's source.

pub mod blocks;
pub mod conceal;
mod document;
pub mod undo;

pub use document::{Clock, Document, View};

/// Where `old` and `new` differ: the range of `old` to replace, and what
/// replaces it, trimmed of the text they start and end with alike.
pub fn differing<'a>(old: &str, new: &'a str) -> (std::ops::Range<usize>, &'a str) {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let most = a.len().min(b.len()) - prefix;
    let mut suffix = (0..most)
        .take_while(|&i| a[a.len() - 1 - i] == b[b.len() - 1 - i])
        .count();
    while !old.is_char_boundary(a.len() - suffix) || !new.is_char_boundary(b.len() - suffix) {
        suffix -= 1;
    }
    (prefix..a.len() - suffix, &new[prefix..b.len() - suffix])
}
