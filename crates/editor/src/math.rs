//! Math in Live Preview, typeset as the preview did (`diagrams::render_math`)
//! in the theme's text colour and size, at the window's pixel density so
//! formulas stay sharp. Typeset formulas are kept until the colour, size or
//! density changes, or too many are kept.

use std::{collections::HashMap, sync::Arc};

use diagrams::{MathStyle, render_math};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

/// Formulas kept before they're all forgotten (typing one makes one per
/// keystroke).
const KEPT: usize = 256;

/// A formula typeset: an SVG image and its size in logical pixels.
#[derive(Clone)]
pub(crate) struct Typeset {
    /// The formula drawn, an SVG.
    pub image: Arc<Image>,
    /// Its width, in logical pixels.
    pub width: f32,
    /// Its height, in logical pixels.
    pub height: f32,
}

/// The settings formulas are typeset with.
#[derive(Clone, PartialEq)]
struct Options {
    /// The text colour, `#rrggbb`.
    colour: String,
    /// The theme's colour for errors, `#rrggbb`: bad LaTeX's.
    error: String,
    /// The font size, in logical pixels.
    font_px: f32,
    /// The window's pixel density.
    scale: f32,
}

/// Formulas typeset, by their LaTeX and whether they're blocks.
#[derive(Default)]
pub(crate) struct MathCache {
    /// The settings this frame typesets with; unset before the first.
    options: Option<Options>,
    /// What's been typeset with them, by LaTeX and whether it's a block.
    typeset: HashMap<(String, bool), Typeset>,
}

impl MathCache {
    /// Takes this frame's colour, size and density, forgetting what was
    /// typeset with others.
    pub(crate) fn frame(&mut self, window: &Window, cx: &App) {
        let options = Options {
            colour: hex(cx.theme().foreground),
            error: hex(cx.theme().danger),
            font_px: f32::from(window.rem_size()) * 1.1,
            scale: window.scale_factor(),
        };
        if self.options.as_ref() != Some(&options) || self.typeset.len() > KEPT {
            self.typeset.clear();
            self.options = Some(options);
        }
    }

    /// `latex` typeset (as a block if `display`). LaTeX that doesn't parse
    /// is its source, `$…$`, in red.
    pub(crate) fn typeset(&mut self, latex: &str, display: bool) -> Typeset {
        // `frame` sets them first.
        let options = self.options.clone().unwrap_or_else(|| Options {
            colour: "#000000".into(),
            error: "#ff0000".into(),
            font_px: 16.,
            scale: 1.,
        });
        self.typeset
            .entry((latex.to_string(), display))
            .or_insert_with(|| {
                let style = if display {
                    MathStyle::Display
                } else {
                    MathStyle::Inline
                };
                let device = options.font_px * options.scale;
                match render_math(latex, style, &options.colour, device) {
                    Ok(math) => Typeset {
                        image: Arc::new(Image::from_bytes(ImageFormat::Svg, math.svg.into_bytes())),
                        width: math.width / options.scale,
                        height: math.height / options.scale,
                    },
                    Err(_) => error(latex, display, &options),
                }
            })
            .clone()
    }
}

/// A colour as `#rrggbb`.
fn hex(colour: Hsla) -> String {
    let rgb = colour.to_rgb();
    let channel = |c: f32| (c.clamp(0., 1.) * 255.).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(rgb.r),
        channel(rgb.g),
        channel(rgb.b)
    )
}

/// LaTeX that didn't parse, drawn as its source in the error colour.
fn error(latex: &str, display: bool, options: &Options) -> Typeset {
    let font_px = options.font_px;
    let dollars = if display { "$$" } else { "$" };
    let text = format!("{dollars}{latex}{dollars}");
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let width = 0.62 * font_px * text.chars().count() as f32;
    let height = 1.3 * font_px;
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='{height}'><text x='0' y='{baseline}' font-family='monospace' font-size='{font_px}' fill='{error}'>{escaped}</text></svg>",
        baseline = font_px,
        error = options.error
    );
    Typeset {
        image: Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())),
        width,
        height,
    }
}
