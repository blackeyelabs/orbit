//! ```mermaid blocks in the preview, drawn in the background so typing
//! never waits (`diagrams::DiagramCache`). Kit's TextView hosts them as a
//! block plugin on fenced code with the `mermaid` language: the same hook
//! as block math. A drawing that lands asks the windows to redraw, and the
//! preview parses again with it.
//!
//! While a diagram is being redrawn, or after an edit breaks it, the last
//! good drawing of that block (the note's first diagram, second…) stays.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use diagrams::{DiagramCache, DiagramError, DiagramRenderer as _, DiagramTheme, Drawn, Mermaid};
use gpui_kit::base::text::{
    MarkdownNode, MarkdownParseContext, MarkdownPlugin, markdown_ast::Node,
};
use gpui_kit::base::v_flex;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Diagrams scale down to fit the preview, to this much of their size;
/// smaller would be unreadable, so past it they scroll sideways instead.
const MIN_SCALE: f32 = 0.6;
/// The space a diagram takes while it's first drawn.
const DRAWING_HEIGHT: f32 = 6.;

/// The app's diagram renderer and drawings.
struct Diagrams {
    cache: Arc<DiagramCache>,
    /// What the renderer can draw, for saying so when it can't.
    supported: Arc<str>,
    /// Bumped as each drawing lands, so previews parse again.
    revision: u64,
}

impl Global for Diagrams {}

pub fn init(cx: &mut App) {
    let (ready, drawn) = async_channel::unbounded();
    let renderer = Mermaid::new();
    let supported = renderer.supported().join(", ").into();
    let cache = DiagramCache::new(Arc::new(renderer), move || {
        ready.send_blocking(()).ok();
    });
    cx.set_global(Diagrams {
        cache: Arc::new(cache),
        supported,
        revision: 0,
    });
    cx.spawn(async move |cx| {
        while drawn.recv().await.is_ok() {
            cx.update(|cx| {
                cx.global_mut::<Diagrams>().revision += 1;
                cx.refresh_windows();
            });
        }
    })
    .detach();
}

/// Changes as drawings land, for the preview's parser revision.
pub fn revision(cx: &App) -> u64 {
    cx.global::<Diagrams>().revision
}

/// Each diagram's last good drawing in one note, by its place among them.
#[derive(Clone, Default)]
pub struct LastDrawn(Arc<Mutex<HashMap<usize, Drawing>>>);

/// Notes rarely have more diagrams than this; the drawings of any past it
/// aren't kept.
const KEPT_PER_NOTE: usize = 64;

/// A drawing ready to show.
#[derive(Clone)]
struct Drawing {
    drawn: Drawn,
    image: Arc<Image>,
    width: f32,
    height: f32,
}

impl Drawing {
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

/// What a block shows.
enum Block {
    Drawn(Drawing),
    /// Not drawn yet: the last drawing meanwhile.
    Drawing(Option<Drawing>),
    /// The source as written, why it couldn't be drawn, and the last
    /// drawing.
    Failed {
        error: DiagramError,
        last: Option<Drawing>,
    },
}

pub struct DiagramPlugin {
    cache: Arc<DiagramCache>,
    supported: Arc<str>,
    theme: DiagramTheme,
    last: LastDrawn,
}

impl DiagramPlugin {
    pub fn new(last: LastDrawn, cx: &App) -> Self {
        let diagrams = cx.global::<Diagrams>();
        DiagramPlugin {
            cache: diagrams.cache.clone(),
            supported: diagrams.supported.clone(),
            theme: if cx.theme().mode.is_dark() {
                DiagramTheme::Dark
            } else {
                DiagramTheme::Light
            },
            last,
        }
    }
}

impl MarkdownPlugin for DiagramPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "orbit-diagram"
    }

    fn parse(&self, node: &Node, context: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let Node::Code(code) = node else {
            return None;
        };
        if !code
            .lang
            .as_deref()
            .is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid"))
        {
            return None;
        }
        // Its place among the note's diagrams, which edits elsewhere in the
        // note don't change.
        let start = code
            .position
            .as_ref()
            .map_or(0, |p| p.start.offset.min(context.source().len()));
        let place = mermaid_fences(&context.source()[..start]);
        let mut last = self.last.0.lock().unwrap_or_else(|e| e.into_inner());
        let previous = last.get(&place).cloned();
        let block = match self.cache.get(&code.value, self.theme) {
            None => Block::Drawing(previous),
            Some(drawn) => match &*drawn {
                Ok(diagram) => {
                    // The same drawing keeps its image, so it isn't
                    // rasterized again.
                    let drawing = previous
                        .filter(|p| Arc::ptr_eq(&p.drawn, &drawn))
                        .unwrap_or_else(|| Drawing::new(&drawn, diagram));
                    if place < KEPT_PER_NOTE {
                        last.insert(place, drawing.clone());
                    }
                    Block::Drawn(drawing)
                }
                Err(error) => Block::Failed {
                    error: error.clone(),
                    last: previous,
                },
            },
        };
        Some(MarkdownNode::new("orbit-diagram", (place, block)).text(code.value.clone()))
    }

    fn render(&self, node: &MarkdownNode, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some((place, block)) = node.data::<(usize, Block)>() else {
            return div().into_any_element();
        };
        let id = SharedString::from(format!("diagram-{place}"));
        match block {
            Block::Drawn(drawing) | Block::Drawing(Some(drawing)) => {
                picture(id, drawing).into_any_element()
            }
            Block::Drawing(None) => div()
                .my_2()
                .h(rems(DRAWING_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Drawing the diagram…")
                .into_any_element(),
            Block::Failed { error, last } => {
                let (title, detail) = match error {
                    DiagramError::Syntax { message, line } => (
                        match line {
                            Some(line) => format!("Couldn't draw this diagram · line {line}"),
                            None => "Couldn't draw this diagram".to_string(),
                        },
                        message.clone(),
                    ),
                    DiagramError::Unsupported => (
                        "Orbit can't draw this kind of diagram".to_string(),
                        format!("It can draw: {}.", self.supported),
                    ),
                    DiagramError::TimedOut => (
                        "This diagram took too long to draw".to_string(),
                        "Try splitting it into smaller ones.".to_string(),
                    ),
                    DiagramError::Failed(why) => {
                        ("Couldn't draw this diagram".to_string(), why.clone())
                    }
                };
                v_flex()
                    .my_2()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().danger)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    )
                    .child(
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(cx.theme().muted)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_sm()
                            .child(node.as_text().to_string()),
                    )
                    .when_some(last.clone(), |this, last| this.child(picture(id, &last)))
                    .into_any_element()
            }
        }
    }
}

/// How many fences opening a mermaid block (```mermaid, ~~~mermaid) start
/// lines of `text`.
fn mermaid_fences(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let line = line.trim_start();
            ['`', '~'].iter().any(|&fence| {
                let info = line.trim_start_matches(fence);
                line.len() - info.len() >= 3 && info.trim().eq_ignore_ascii_case("mermaid")
            })
        })
        .count()
}

/// A drawing at its size, scaled down to fit the preview, or scrolling
/// sideways when it would be too small to read.
fn picture(id: SharedString, drawing: &Drawing) -> impl IntoElement {
    div().id(id).my_2().w_full().overflow_x_scroll().child(
        img(drawing.image.clone())
            .w_full()
            .max_w(px(drawing.width))
            .min_w(px(drawing.width * MIN_SCALE))
            .aspect_ratio(drawing.width / drawing.height),
    )
}
