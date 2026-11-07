//! ```mermaid blocks in Live Preview, drawn as the preview drew them: in
//! the background so typing never waits (`diagrams::DiagramCache`). While
//! a diagram is being drawn the editor looks again every so often (see
//! [`POLL`]), so nothing wakes it from the drawing thread. Away from the
//! caret a block shows its diagram; a click on it, or the caret coming in
//! (Up and Down find its leaf, an unseen empty line over the diagram's
//! top), shows its source. While a diagram is redrawn, or after an edit breaks it, the
//! last good drawing of that block (the note's first diagram, second…)
//! stays.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use diagrams::{DiagramCache, DiagramError, DiagramRenderer as _, DiagramTheme, Drawn, Mermaid};
use gpui_kit::base::v_flex;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use live_preview::blocks::Kind;

use crate::LivePreview;

/// Diagrams scale down to fit the note, to this much of their size;
/// smaller would be unreadable, so past it they scroll sideways instead.
const MIN_SCALE: f32 = 0.6;
/// The space a diagram takes while it's first drawn, in rems.
const DRAWING_HEIGHT: f32 = 6.;
/// Notes rarely have more diagrams than this; the drawings of any past it
/// aren't kept.
const KEPT_PER_NOTE: usize = 64;

/// The app's diagram renderer and drawings.
struct Diagrams {
    cache: Arc<DiagramCache>,
    /// What the renderer can draw, for saying so when it can't.
    supported: Arc<str>,
}

impl Global for Diagrams {}

/// How often an editor showing a diagram still being drawn looks again.
pub(crate) const POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// Starts the diagram renderer.
pub(crate) fn init(cx: &mut App) {
    let renderer = Mermaid::new();
    let supported = renderer.supported().join(", ").into();
    let cache = DiagramCache::new(Arc::new(renderer), || {});
    cx.set_global(Diagrams {
        cache: Arc::new(cache),
        supported,
    });
}

/// The theme diagrams are drawn in.
fn theme(cx: &App) -> DiagramTheme {
    if cx.theme().mode.is_dark() {
        DiagramTheme::Dark
    } else {
        DiagramTheme::Light
    }
}

/// Whether the diagram of `source` is still being drawn.
pub(crate) fn pending(source: &str, cx: &App) -> bool {
    cx.global::<Diagrams>()
        .cache
        .get(source, theme(cx))
        .is_none()
}

/// A drawing ready to show.
#[derive(Clone)]
struct Drawing {
    /// What the renderer drew, to tell a new drawing from this one.
    drawn: Drawn,
    /// The drawing as an SVG image.
    image: Arc<Image>,
    /// Its size, in pixels.
    width: f32,
    height: f32,
}

impl Drawing {
    /// The drawing `drawn` made of `diagram`, to show.
    fn new(drawn: &Drawn, diagram: &diagrams::Diagram) -> Self {
        Drawing {
            drawn: drawn.clone(),
            image: Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                diagram.svg.clone().into_bytes(),
            )),
            width: diagram.width,
            height: diagram.height,
        }
    }
}

/// Each diagram's last good drawing in one note, by its place among them.
#[derive(Clone, Default)]
pub(crate) struct LastDrawn(Arc<Mutex<HashMap<usize, Drawing>>>);

/// What a diagram block shows now.
#[derive(Clone, Debug, PartialEq)]
pub enum DiagramShows {
    /// "Drawing the diagram…": it hasn't been drawn yet.
    Drawing,
    /// A drawing this big, in pixels: its own or, while it's redrawn or
    /// broken, its last good one.
    Drawn { width: f32, height: f32 },
    /// Why it couldn't be drawn, with its source (and the last good
    /// drawing, if there is one).
    Failed,
}

/// What a block shows: a drawing, or not yet one, or why it failed.
enum Block {
    Drawn(Drawing),
    /// Not drawn yet: the last drawing meanwhile.
    Drawing(Option<Drawing>),
    /// Why it couldn't be drawn, and the last drawing.
    Failed {
        error: DiagramError,
        last: Option<Drawing>,
    },
}

impl Block {
    /// The drawing it shows as its diagram: its own, or the last while it's
    /// redrawn. (A failed one shows the last under the error.)
    fn picture(&self) -> Option<&Drawing> {
        match self {
            Block::Drawn(d) | Block::Drawing(Some(d)) => Some(d),
            Block::Drawing(None) | Block::Failed { .. } => None,
        }
    }

    /// What it shows, as the owner sees it.
    fn shows(&self) -> DiagramShows {
        match (self, self.picture()) {
            (Block::Failed { .. }, _) => DiagramShows::Failed,
            (_, Some(d)) => DiagramShows::Drawn {
                width: d.width,
                height: d.height,
            },
            (_, None) => DiagramShows::Drawing,
        }
    }
}

/// Whether a block is a diagram: a code block in Mermaid.
pub(crate) fn is_diagram(kind: &Kind) -> bool {
    matches!(kind, Kind::Code(code) if code
        .lang
        .as_deref()
        .is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid")))
}

/// A diagram block as drawn: the note's `place`-th diagram, of `source`.
pub(crate) struct DiagramFrame {
    /// Its block, which a click shows the source of.
    pub block: usize,
    /// Its place among the note's diagrams, which keys its last good
    /// drawing: edits elsewhere don't change it, but a diagram added above
    /// does, so the blocks below show another's last drawing until they're
    /// drawn again (as the preview did).
    pub place: usize,
    /// Its Mermaid source.
    pub source: String,
}

impl LastDrawn {
    /// What `frame`'s diagram shows now, keeping a new good drawing as its
    /// last.
    fn block(&self, frame: &DiagramFrame, cx: &App) -> Block {
        let mut last = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let previous = last.get(&frame.place).cloned();
        match cx.global::<Diagrams>().cache.get(&frame.source, theme(cx)) {
            None => Block::Drawing(previous),
            Some(drawn) => match &*drawn {
                Ok(diagram) => {
                    // The same drawing keeps its image, so it isn't
                    // rasterized again.
                    let drawing = previous
                        .filter(|p| Arc::ptr_eq(&p.drawn, &drawn))
                        .unwrap_or_else(|| Drawing::new(&drawn, diagram));
                    if frame.place < KEPT_PER_NOTE {
                        last.insert(frame.place, drawing.clone());
                    }
                    Block::Drawn(drawing)
                }
                Err(error) => Block::Failed {
                    error: error.clone(),
                    last: previous,
                },
            },
        }
    }

    /// What `frame`'s diagram shows now.
    pub(crate) fn shows(&self, frame: &DiagramFrame, cx: &App) -> DiagramShows {
        self.block(frame, cx).shows()
    }

    /// `frame`'s diagram drawn, its block's `anchor` (its leaf, an empty
    /// line) unseen over its top; a click on it shows its source.
    pub(crate) fn render(
        &self,
        frame: &DiagramFrame,
        anchor: AnyElement,
        editor: &Entity<LivePreview>,
        cx: &App,
    ) -> AnyElement {
        let id = SharedString::from(format!("diagram-{}", frame.place));
        let theme = cx.theme();
        let shown = match self.block(frame, cx) {
            Block::Failed { error, last } => {
                let supported = cx.global::<Diagrams>().supported.clone();
                let (title, detail) = describe(&error, &supported);
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.danger)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(detail),
                    )
                    .child(
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(theme.muted)
                            .font_family(theme.mono_font_family.clone())
                            .text_sm()
                            .child(frame.source.clone()),
                    )
                    .when_some(last, |this, last| this.child(picture(id, &last)))
                    .into_any_element()
            }
            block => match block.picture() {
                Some(drawing) => picture(id, drawing).into_any_element(),
                None => div()
                    .h(rems(DRAWING_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Drawing the diagram…")
                    .into_any_element(),
            },
        };
        let (editor, block) = (editor.clone(), frame.block);
        div()
            .relative()
            .my_2()
            .cursor_pointer()
            .child(shown)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .opacity(0.)
                    .child(anchor),
            )
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                editor.update(cx, |editor, cx| editor.show_source(block, window, cx));
            })
            .into_any_element()
    }
}

/// What to say about a diagram that couldn't be drawn: a title, then why.
fn describe(error: &DiagramError, supported: &str) -> (String, String) {
    match error {
        DiagramError::Syntax { message, line } => (
            match line {
                Some(line) => format!("Couldn't draw this diagram · line {line}"),
                None => "Couldn't draw this diagram".to_string(),
            },
            message.clone(),
        ),
        DiagramError::Unsupported => (
            "Orbit can't draw this kind of diagram".to_string(),
            format!("It can draw: {supported}."),
        ),
        DiagramError::TimedOut => (
            "This diagram took too long to draw".to_string(),
            "Try splitting it into smaller ones.".to_string(),
        ),
        DiagramError::Failed(why) => ("Couldn't draw this diagram".to_string(), why.clone()),
    }
}

/// A drawing at its size, scaled down to fit the note, or scrolling
/// sideways when it would be too small to read.
fn picture(id: SharedString, drawing: &Drawing) -> impl IntoElement {
    div().id(id).w_full().overflow_x_scroll().child(
        img(drawing.image.clone())
            .w_full()
            .max_w(px(drawing.width))
            .min_w(px(drawing.width * MIN_SCALE))
            .aspect_ratio(drawing.width / drawing.height),
    )
}
