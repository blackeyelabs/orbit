//! Runs every enabled renderer over `corpus/` (then `corpus/user/`) in light
//! and dark themes and writes `output/`: one SVG and PNG per render,
//! `results.csv` and `gallery.html`.
//!
//!     cargo run --release --features merman,rusty-mermaid,mmdr

use std::{fs, path::Path, sync::Arc, time::Duration};

use spike2_mermaid::{
    Diagram, DiagramRenderer, Status, Theme, gallery_html, png_path, results_csv, run, svg_path,
};

#[cfg(feature = "merman")]
struct Merman;

#[cfg(feature = "merman")]
impl DiagramRenderer for Merman {
    fn name(&self) -> &str {
        "merman"
    }

    fn render(&self, source: &str, theme: Theme) -> Result<String, String> {
        use merman::{OperationControl, RenderOutput, RenderRequest, SvgRequest};
        // A parity port of mermaid.js, so options go in Mermaid's own init
        // directive. htmlLabels=false makes labels SVG <text> instead of
        // <foreignObject> HTML, which resvg (and so Orbit) can't draw.
        let theme = match theme {
            Theme::Light => "default",
            Theme::Dark => "dark",
        };
        let source =
            format!("%%{{init: {{\"theme\": \"{theme}\", \"htmlLabels\": false}}}}%%\n{source}");
        let output = merman::Renderer::new()
            .render(RenderRequest::svg(
                &source,
                OperationControl::new(),
                SvgRequest::default(),
            ))
            .map_err(|e| format!("{e:?}"))?;
        match output {
            // Mindmap and journey labels are HTML <foreignObject> even with
            // htmlLabels=false (as in mermaid.js); merman's own fallback adds
            // a <text> overlay so rasterizers like resvg can draw them.
            RenderOutput::Svg(Some(svg)) => {
                Ok(merman::svg::foreign_object_label_fallback_svg_text(
                    svg.svg(),
                    &merman::svg::DeterministicTextMeasurer::default(),
                ))
            }
            _ => Err("no Mermaid diagram found".into()),
        }
    }
}

#[cfg(feature = "rusty-mermaid")]
struct RustyMermaid;

#[cfg(feature = "rusty-mermaid")]
impl DiagramRenderer for RustyMermaid {
    fn name(&self) -> &str {
        "rusty-mermaid"
    }

    fn render(&self, source: &str, theme: Theme) -> Result<String, String> {
        let theme = match theme {
            Theme::Light => rusty_mermaid::Theme::light(),
            Theme::Dark => rusty_mermaid::Theme::dark(),
        };
        rusty_mermaid::to_svg(source, &theme).map_err(|e| format!("{e:?}"))
    }
}

#[cfg(feature = "mmdr")]
struct Mmdr;

#[cfg(feature = "mmdr")]
impl DiagramRenderer for Mmdr {
    fn name(&self) -> &str {
        "mmdr"
    }

    fn render(&self, source: &str, theme: Theme) -> Result<String, String> {
        use mermaid_rs_renderer::{RenderOptions, Theme as MmdrTheme, render_strict};
        let options = RenderOptions {
            theme: match theme {
                Theme::Light => MmdrTheme::modern(),
                Theme::Dark => MmdrTheme::dark(),
            },
            ..Default::default()
        };
        render_strict(source, options).map_err(|e| format!("{e:?}"))
    }
}

fn renderers() -> Vec<Arc<dyn DiagramRenderer>> {
    vec![
        #[cfg(feature = "merman")]
        Arc::new(Merman),
        #[cfg(feature = "rusty-mermaid")]
        Arc::new(RustyMermaid),
        #[cfg(feature = "mmdr")]
        Arc::new(Mmdr),
    ]
}

/// `corpus/*.mmd` sorted, then `corpus/user/*.mmd` sorted (prefixed `user-`).
fn load_corpus(dir: &Path) -> Vec<Diagram> {
    let read_dir = |dir: &Path, prefix: &str| -> Vec<Diagram> {
        let mut paths: Vec<_> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "mmd"))
            .collect();
        paths.sort();
        paths
            .into_iter()
            .map(|p| Diagram {
                name: format!("{prefix}{}", p.file_stem().unwrap().to_string_lossy()),
                source: fs::read_to_string(&p).expect("read diagram"),
            })
            .collect()
    };
    let mut corpus = read_dir(dir, "");
    corpus.extend(read_dir(&dir.join("user"), "user-"));
    corpus
}

/// Rasterizes an SVG to PNG with system fonts, for viewing outside a browser.
fn rasterize(svg: &str, png: &Path) -> Result<(), String> {
    use resvg::{tiny_skia, usvg};
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(svg, &options).map_err(|e| e.to_string())?;
    let size = tree.size().to_int_size();
    let scale = (1600.0 / size.width().max(size.height()) as f32).min(2.0);
    let (w, h) = (
        (size.width() as f32 * scale).ceil() as u32,
        (size.height() as f32 * scale).ceil() as u32,
    );
    let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or("empty image")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.save_png(png).map_err(|e| e.to_string())
}

fn main() {
    let renderers = renderers();
    if renderers.is_empty() {
        eprintln!("no renderer enabled; use --features merman,rusty-mermaid,mmdr");
        std::process::exit(2);
    }
    let corpus = load_corpus(Path::new("corpus"));
    let outcomes = run(
        &renderers,
        &corpus,
        &[Theme::Light, Theme::Dark],
        Duration::from_secs(20),
    );

    let root = Path::new("output");
    let mut raster_failures = String::new();
    for o in &outcomes {
        let Some(svg) = &o.svg else { continue };
        let path = root.join(svg_path(o));
        fs::create_dir_all(path.parent().unwrap()).expect("create output dir");
        fs::write(&path, svg).expect("write svg");
        if let Err(e) = rasterize(svg, &root.join(png_path(o))) {
            raster_failures.push_str(&format!("{}: {e}\n", svg_path(o)));
        }
    }
    // A render resvg can't draw is a failure for Orbit even if the renderer said ok.
    fs::write(root.join("raster-failures.txt"), &raster_failures).expect("write raster failures");
    fs::write(root.join("results.csv"), results_csv(&outcomes)).expect("write csv");
    fs::write(root.join("gallery.html"), gallery_html(&corpus, &outcomes)).expect("write gallery");

    for r in &renderers {
        let runs: Vec<_> = outcomes.iter().filter(|o| o.renderer == r.name()).collect();
        let ok = runs.iter().filter(|o| o.status == Status::Ok).count();
        println!("{:<14} {ok}/{} ok", r.name(), runs.len());
    }
    println!(
        "{} raster failures (output/raster-failures.txt)",
        raster_failures.lines().count()
    );
    println!("wrote output/results.csv and output/gallery.html");
}
