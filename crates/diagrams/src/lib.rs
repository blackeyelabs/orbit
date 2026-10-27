//! The DiagramRenderer trait and the Mermaid backend, plus math typesetting.
//! Everything renders to self-contained SVG; the app rasterizes it.

mod cache;
mod math;
mod mermaid;

pub use cache::{DiagramCache, Drawn};
pub use math::{Math, MathStyle, render_math};
pub use mermaid::Mermaid;

/// Draws diagrams from their source.
pub trait DiagramRenderer: Send + Sync {
    /// The kinds of diagram it can draw, by name.
    fn supported(&self) -> Vec<String>;

    /// Draws `source`; never panics.
    fn render(&self, source: &str, theme: DiagramTheme) -> Result<Diagram, DiagramError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagramTheme {
    Light,
    Dark,
}

/// A drawn diagram: SVG at its natural size, in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagram {
    pub svg: String,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagramError {
    /// The source doesn't parse: why, and on which line (from 1), if known.
    Syntax {
        message: String,
        line: Option<usize>,
    },
    /// Not a kind of diagram the renderer draws.
    Unsupported,
    /// Drawing took longer than the renderer allows.
    TimedOut,
    /// The renderer couldn't draw it, for a reason of its own.
    Failed(String),
}

impl std::fmt::Display for DiagramError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagramError::Syntax {
                message,
                line: Some(line),
            } => {
                write!(f, "line {line}: {message}")
            }
            DiagramError::Syntax {
                message,
                line: None,
            } => f.write_str(message),
            DiagramError::Unsupported => f.write_str("not a kind of diagram Orbit can draw"),
            DiagramError::TimedOut => f.write_str("took too long to draw"),
            DiagramError::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for DiagramError {}
