//! Diagrams as Orbit shows them: rasterized by resvg, which GPUI draws SVG
//! with (with the system's fonts).

use diagrams::{DiagramRenderer, DiagramTheme, Mermaid};
use resvg::{tiny_skia, usvg};

/// The diagram for `source`, drawn in light: pixels, width, height.
fn raster(source: &str) -> (tiny_skia::Pixmap, f32) {
    let diagram = Mermaid::new().render(source, DiagramTheme::Light).unwrap();
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(&diagram.svg, &options).unwrap();
    let size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).unwrap();
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    (pixmap, diagram.width)
}

/// How dark the pixels in a box are, 0 to 1.
fn darkness(pixmap: &tiny_skia::Pixmap, x: u32, y: u32, w: u32, h: u32) -> f32 {
    let mut total = 0.;
    for py in y..y + h {
        for px in x..x + w {
            let p = pixmap.pixel(px, py).unwrap();
            let lightness = (p.red() as f32 + p.green() as f32 + p.blue() as f32) / 765.;
            total += (1. - lightness) * p.alpha() as f32 / 255.;
        }
    }
    total / (w * h) as f32
}

/// The darkest small square in each horizontal band of the image: a
/// marker's middle, if it's filled, is near black.
fn darkest_spot(pixmap: &tiny_skia::Pixmap) -> f32 {
    let mut darkest: f32 = 0.;
    for y in (0..pixmap.height().saturating_sub(4)).step_by(2) {
        for x in (0..pixmap.width().saturating_sub(4)).step_by(2) {
            darkest = darkest.max(darkness(pixmap, x, y, 4, 4));
        }
    }
    darkest
}

#[test]
fn aggregation_and_inheritance_markers_are_hollow() {
    // Two boxes and a line: nothing should be solid dark but the line,
    // which is thinner than the square measured.
    for arrow in ["o--", "<|--"] {
        let (pixmap, _) = raster(&format!("classDiagram\n  A {arrow} B\n"));
        assert!(
            darkest_spot(&pixmap) < 0.8,
            "{arrow}: a filled marker ({})",
            darkest_spot(&pixmap)
        );
    }
    // The control: composition's diamond is filled.
    let (pixmap, _) = raster("classDiagram\n  A *-- B\n");
    assert!(darkest_spot(&pixmap) > 0.8);
}

#[test]
fn cjk_labels_are_drawn_where_the_system_has_a_font_for_them() {
    let mut fonts = usvg::fontdb::Database::new();
    fonts.load_system_fonts();
    let has_cjk = fonts.faces().any(|face| {
        face.families.iter().any(|(name, _)| {
            ["CJK", "Han Sans", "Han Serif", "WenQuanYi"]
                .iter()
                .any(|k| name.contains(k))
        })
    });
    if !has_cjk {
        eprintln!("skipped: no CJK font installed");
        return;
    }
    let (labelled, _) = raster("flowchart LR\n  A[日本語のラベル]\n");
    let (blank, _) = raster("flowchart LR\n  A[ ]\n");
    let ink = |p: &tiny_skia::Pixmap| {
        darkness(p, 0, 0, p.width(), p.height()) * (p.width() * p.height()) as f32
    };
    assert!(
        ink(&labelled) > ink(&blank) * 1.2,
        "the label has no glyphs"
    );
}
