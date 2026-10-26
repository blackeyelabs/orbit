//! LaTeX math to SVG, through RaTeX (a KaTeX port).

use ratex_layout::{LayoutOptions, layout, to_display_list};
use ratex_svg::{SvgColorSyntax, SvgOptions, render_to_svg_with_color_syntax};
use ratex_types::{color::Color, math_style};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathStyle {
    /// `$…$`, sized to sit in a line of text.
    Inline,
    /// `$$…$$`, on its own line.
    Display,
}

/// A typeset formula. Sizes are in pixels at the requested font size.
#[derive(Clone, Debug)]
pub struct Math {
    pub svg: String,
    pub width: f32,
    pub height: f32,
    /// Distance from the top to the text baseline.
    pub baseline: f32,
}

/// Typesets `latex` at `font_px` pixels per em in `color` (`#rrggbb`).
/// Glyphs are embedded as outlines, so the SVG needs no fonts to draw.
pub fn render_math(
    latex: &str,
    style: MathStyle,
    color: &str,
    font_px: f32,
) -> Result<Math, String> {
    let color = Color::parse(color).ok_or_else(|| format!("not a colour: {color}"))?;
    let style = match style {
        MathStyle::Inline => math_style::MathStyle::Text,
        MathStyle::Display => math_style::MathStyle::Display,
    };
    let ast = ratex_parser::parser::parse(latex).map_err(|e| e.to_string())?;
    let list = to_display_list(&layout(
        &ast,
        &LayoutOptions::default().with_style(style).with_color(color),
    ));
    let options = SvgOptions {
        font_size: f64::from(font_px),
        padding: 0.0,
        embed_glyphs: true,
        ..SvgOptions::default()
    };
    let svg = render_to_svg_with_color_syntax(&list, &options, SvgColorSyntax::Rgb);
    let em = font_px;
    Ok(Math {
        // RaTeX sizes the document in points; pixels keep it at `font_px`.
        svg: svg
            .replacen("pt\" height=", "\" height=", 1)
            .replacen("pt\">", "\">", 1),
        width: list.width as f32 * em,
        height: (list.height + list.depth) as f32 * em,
        baseline: list.height as f32 * em,
    })
}
