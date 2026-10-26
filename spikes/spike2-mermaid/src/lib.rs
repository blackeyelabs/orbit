//! Spike 2 harness: runs every Mermaid renderer over a corpus and records
//! what happened, so the renderers can be compared side by side.

use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// One Mermaid backend under test.
pub trait DiagramRenderer: Send + Sync {
    fn name(&self) -> &str;
    /// Renders `source` to an SVG document, or explains why it couldn't.
    fn render(&self, source: &str, theme: Theme) -> Result<String, String>;
}

pub struct Diagram {
    pub name: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// The renderer returned an error (the right response to bad input).
    Error(String),
    /// The renderer panicked; the message if it had one.
    Panic(String),
    /// Still running when the time limit passed (the thread is abandoned).
    Timeout,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub renderer: String,
    pub diagram: String,
    pub theme: Theme,
    pub status: Status,
    /// Wall time until the result (or the limit, for a timeout).
    pub elapsed: Duration,
    pub svg: Option<String>,
}

/// Renders every diagram with every renderer in every theme. Each render runs
/// on its own thread so a panic or a hang can't stop the run.
pub fn run(
    renderers: &[Arc<dyn DiagramRenderer>],
    corpus: &[Diagram],
    themes: &[Theme],
    limit: Duration,
) -> Vec<Outcome> {
    let mut out = Vec::new();
    for r in renderers {
        for d in corpus {
            for &theme in themes {
                let (status, elapsed, svg) = render_one(r.clone(), d.source.clone(), theme, limit);
                out.push(Outcome {
                    renderer: r.name().to_string(),
                    diagram: d.name.clone(),
                    theme,
                    status,
                    elapsed,
                    svg,
                });
            }
        }
    }
    out
}

fn render_one(
    renderer: Arc<dyn DiagramRenderer>,
    source: String,
    theme: Theme,
    limit: Duration,
) -> (Status, Duration, Option<String>) {
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            renderer.render(&source, theme)
        }));
        // The receiver is gone if we already timed out; nothing to do then.
        let _ = tx.send(result);
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(Ok(svg))) => (Status::Ok, started.elapsed(), Some(svg)),
        Ok(Ok(Err(e))) => (Status::Error(e), started.elapsed(), None),
        Ok(Err(panic)) => (Status::Panic(panic_message(panic)), started.elapsed(), None),
        Err(_) => (Status::Timeout, limit, None),
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Error(_) => "error",
            Status::Panic(_) => "panic",
            Status::Timeout => "timeout",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Status::Error(m) | Status::Panic(m) => m,
            Status::Ok | Status::Timeout => "",
        }
    }
}

/// One CSV row per outcome. Fields are quoted when they contain a comma,
/// quote or newline; newlines in details are flattened to spaces.
pub fn results_csv(outcomes: &[Outcome]) -> String {
    let mut csv = String::from("renderer,diagram,theme,status,ms,svg_bytes,detail\n");
    for o in outcomes {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            csv_field(&o.renderer),
            csv_field(&o.diagram),
            o.theme.label(),
            o.status.label(),
            o.elapsed.as_millis(),
            o.svg.as_ref().map_or(0, String::len),
            csv_field(&o.status.detail().replace('\n', " "))
        ));
    }
    csv
}

/// Where an outcome's SVG is written, relative to the output directory.
pub fn svg_path(o: &Outcome) -> String {
    format!("out/{}/{}-{}.svg", o.renderer, o.diagram, o.theme.label())
}

/// Where the resvg raster of an outcome's SVG is written.
pub fn png_path(o: &Outcome) -> String {
    format!("out/{}/{}-{}.png", o.renderer, o.diagram, o.theme.label())
}

/// A side-by-side page: one row per diagram and theme, one column per
/// renderer (in run order), the diagram source in the first column.
pub fn gallery_html(corpus: &[Diagram], outcomes: &[Outcome]) -> String {
    let renderers = unique_in_order(outcomes.iter().map(|o| o.renderer.as_str()));
    let themes = unique_in_order(outcomes.iter().map(|o| o.theme));

    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>Spike 2 gallery</title>\n<style>\
         body{font:14px system-ui;margin:16px} table{border-collapse:collapse}\
         td,th{border:1px solid #ccc;padding:8px;vertical-align:top}\
         td img{max-width:420px;max-height:420px} pre{max-width:260px;white-space:pre-wrap;font-size:12px}\
         tr.dark td.out{background:#1e1e1e;color:#eee} .fail{color:#b00020}\
         </style>\n<table><tr><th>diagram</th>",
    );
    for r in &renderers {
        html.push_str(&format!("<th>{}</th>", escape(r)));
    }
    html.push_str("</tr>\n");
    for d in corpus {
        for &theme in &themes {
            html.push_str(&format!(
                "<tr class={}><td><b>{}</b> ({})<pre>{}</pre></td>",
                theme.label(),
                escape(&d.name),
                theme.label(),
                escape(&d.source)
            ));
            for r in &renderers {
                let cell = outcomes
                    .iter()
                    .find(|o| o.renderer == *r && o.diagram == d.name && o.theme == theme);
                html.push_str("<td class=out>");
                match cell {
                    // The resvg raster is what Orbit would draw; the SVG is a click away.
                    Some(o) if o.status == Status::Ok => html.push_str(&format!(
                        "<a href=\"{}\"><img src=\"{}\" alt=\"{} {}\"></a><br>{} ms",
                        svg_path(o),
                        png_path(o),
                        escape(r),
                        escape(&d.name),
                        o.elapsed.as_millis()
                    )),
                    Some(o) => html.push_str(&format!(
                        "<span class=fail>{}</span><br>{}",
                        o.status.label(),
                        escape(o.status.detail())
                    )),
                    None => html.push_str("not run"),
                }
                html.push_str("</td>");
            }
            html.push_str("</tr>\n");
        }
    }
    html.push_str("</table>\n");
    html
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unique_in_order<T: PartialEq>(items: impl Iterator<Item = T>) -> Vec<T> {
    let mut seen = Vec::new();
    for item in items {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
