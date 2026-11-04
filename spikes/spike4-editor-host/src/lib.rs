//! Spike 4 (#36): Live Preview rendered through Kit's `editor_host`.
//! `model` is the std-only half (blocks, leaves, caret stops); `host` is
//! the GPUI half that renders a [`model::Doc`] as Kit block nodes and maps
//! caret, mouse, selection and IME through each leaf's geometry.

pub mod host;
pub mod model;
