//! Math typesetting for the preview.

use diagrams::{MathStyle, render_math};

#[test]
fn a_formula_renders_to_self_contained_svg_in_the_given_colour() {
    let math = render_math(r"\frac{a^2}{\sqrt{b}}", MathStyle::Display, "#1f2a44", 16.0).unwrap();
    assert!(math.svg.starts_with("<svg"), "{}", &math.svg[..80]);
    // Sized in pixels, not points, so it draws at `font_px`.
    assert!(!math.svg[..math.svg.find('>').unwrap()].contains("pt\""));
    // Glyphs are outlines, so the SVG draws without KaTeX fonts installed.
    assert!(math.svg.contains("<path"));
    assert!(!math.svg.contains("<text"));
    assert!(math.svg.contains("31,42,68") || math.svg.to_lowercase().contains("#1f2a44"));
    assert!(math.width > 0.0 && math.height > 0.0);
    // A fraction reaches below the text baseline.
    assert!(math.baseline > 0.0 && math.baseline < math.height);
}

#[test]
fn inline_math_is_smaller_than_display_math() {
    let display = render_math(r"\sum_{i=1}^n i", MathStyle::Display, "#000000", 16.0).unwrap();
    let inline = render_math(r"\sum_{i=1}^n i", MathStyle::Inline, "#000000", 16.0).unwrap();
    assert!(inline.height < display.height);
}

#[test]
fn invalid_latex_is_an_error_not_a_panic() {
    assert!(render_math(r"\frac{a", MathStyle::Display, "#000000", 16.0).is_err());
}
