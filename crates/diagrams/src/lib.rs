//! The DiagramRenderer trait and the Mermaid backend, plus math typesetting.
//! Everything renders to self-contained SVG; the app rasterizes it.

mod math;

pub use math::{Math, MathStyle, render_math};
