//! Mermaid through merman, chosen in Phase 0 (docs/phase0-gate.md), set up
//! as the gate requires: no ELK, SVG text labels (htmlLabels off), merman's
//! fallback for labels that stay HTML, negative stroke widths clamped, and
//! a time limit.

use std::{panic, time::Duration};

use merman::{OperationControl, RenderError, RenderOutput, RenderRequest, SvgRequest};

use crate::{Diagram, DiagramError, DiagramRenderer, DiagramTheme};

/// How long a diagram may take to draw, unless told otherwise.
const TIME_LIMIT: Duration = Duration::from_secs(5);

pub struct Mermaid {
    time_limit: Duration,
}

impl Default for Mermaid {
    fn default() -> Self {
        Mermaid::new()
    }
}

impl Mermaid {
    pub fn new() -> Self {
        Mermaid {
            time_limit: TIME_LIMIT,
        }
    }

    /// Gives up on a diagram after `limit`.
    pub fn time_limit(self, limit: Duration) -> Self {
        Mermaid { time_limit: limit }
    }
}

impl DiagramRenderer for Mermaid {
    fn supported(&self) -> Vec<String> {
        merman::supported_diagrams()
            .iter()
            .map(|kind| kind.to_string())
            .collect()
    }

    fn render(&self, source: &str, theme: DiagramTheme) -> Result<Diagram, DiagramError> {
        if source.trim().is_empty() {
            return Err(DiagramError::Unsupported);
        }
        // merman is a port of mermaid.js, so options go in Mermaid's own init
        // directive. With htmlLabels off, labels are SVG <text>, which resvg
        // (and so Orbit) can draw; HTML <foreignObject> it can't.
        let theme = match theme {
            DiagramTheme::Light => "default",
            DiagramTheme::Dark => "dark",
        };
        let init = format!("%%{{init: {{\"theme\": \"{theme}\", \"htmlLabels\": false}}}}%%\n");
        // After any front matter of the diagram's own, which must lead.
        let at = front_matter_len(source);
        let full = format!("{}{init}{}", &source[..at], &source[at..]);
        let control = OperationControl::new().with_deadline(self.time_limit);
        // Nothing it touches is used again if it panics.
        let rendered = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            merman::Renderer::new().render(RenderRequest::svg(
                &full,
                control,
                SvgRequest::default(),
            ))
        }))
        .map_err(|_| DiagramError::Failed("the renderer crashed".into()))?;
        let svg = match rendered {
            Ok(RenderOutput::Svg(Some(svg))) => svg,
            Ok(_) | Err(RenderError::NoDiagram) => return Err(DiagramError::Unsupported),
            Err(RenderError::Cancelled(_)) => return Err(DiagramError::TimedOut),
            Err(RenderError::Parse(diagnostic)) => {
                let details = diagnostic.terminal_diagnostic_details();
                if details.code == "merman.parse.no_diagram_detected" {
                    return Err(DiagramError::Unsupported);
                }
                // Spans count in what merman was given, init line and all.
                let line = details.span.map(|span| {
                    let offset = match span.start.checked_sub(at) {
                        Some(past) => at + past.saturating_sub(init.len()),
                        None => span.start,
                    }
                    .min(source.len());
                    source.as_bytes()[..offset]
                        .iter()
                        .filter(|&&b| b == b'\n')
                        .count()
                        + 1
                });
                return Err(DiagramError::Syntax {
                    message: parse_message(&diagnostic.terminal_safe_message()),
                    line,
                });
            }
            Err(e) => return Err(DiagramError::Failed(e.to_string())),
        };
        // Mindmap and journey labels are HTML even with htmlLabels off, as
        // in mermaid.js; merman's fallback lays <text> over them.
        let svg = merman::svg::foreign_object_label_fallback_svg_text(
            svg.svg(),
            &merman::svg::DeterministicTextMeasurer::default(),
        );
        sized(&readable_css(&clamp_negative_stroke_widths(&svg)))
    }
}

/// The length of a `---` front matter block leading `source`, or 0.
fn front_matter_len(source: &str) -> usize {
    let mut lines = source.split_inclusive('\n');
    if lines.next().is_none_or(|first| first.trim_end() != "---") {
        return 0;
    }
    let mut len = source.find('\n').map_or(source.len(), |n| n + 1);
    for line in lines {
        len += line.len();
        if line.trim_end() == "---" {
            return len;
        }
    }
    0
}

/// merman's message without its "Diagram parse error (kind): " lead-in.
fn parse_message(message: &str) -> String {
    match message.split_once("): ") {
        Some((lead, rest)) if lead.starts_with("Diagram parse error") => rest.to_string(),
        _ => message.to_string(),
    }
}

/// The SVG drawn at its natural size: merman makes it as wide as the page
/// (`width="100%"`), sized only by its view box.
fn sized(svg: &str) -> Result<Diagram, DiagramError> {
    let no_size = || DiagramError::Failed("the drawing has no size".into());
    let root_end = svg.find('>').ok_or_else(no_size)?;
    let root = &svg[..root_end];
    let view_box = attribute(root, "viewBox").ok_or_else(no_size)?;
    let numbers: Vec<f32> = view_box
        .split([' ', ','])
        .filter(|n| !n.is_empty())
        .filter_map(|n| n.parse().ok())
        .collect();
    let [_, _, width, height] = numbers[..] else {
        return Err(no_size());
    };
    if !(width > 0. && height > 0.) {
        return Err(no_size());
    }
    let mut root = root.replacen("width=\"100%\"", &format!("width=\"{width}\""), 1);
    if attribute(&root, "height").is_none() {
        root.push_str(&format!(" height=\"{height}\""));
    }
    Ok(Diagram {
        svg: format!("{root}{}", &svg[root_end..]),
        width,
        height,
    })
}

/// The value of `name="…"` in an element's opening tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let len = tag[start..].find('"')?;
    Some(&tag[start..start + len])
}

/// The SVG's styles with attribute selectors (`[id$="-extensionStart"]`)
/// left out of their rules. resvg can't read them, and drops a whole rule
/// for one: class diagrams' hollow markers (`.aggregation`, `.extension`,
/// listed beside them) came out filled.
fn readable_css(svg: &str) -> String {
    let (Some(open), Some(close)) = (svg.find("<style>"), svg.find("</style>")) else {
        return svg.to_string();
    };
    let css = &svg[open + "<style>".len()..close];
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(brace) = rest.find('{') {
        let selectors = &rest[..brace];
        let Some(end) = block_end(rest, brace) else {
            break;
        };
        let block = &rest[brace..end];
        if selectors.trim_start().starts_with('@') {
            out.push_str(&rest[..end]);
        } else {
            let kept: Vec<&str> = selectors.split(',').filter(|s| !s.contains('[')).collect();
            if !kept.is_empty() {
                out.push_str(&kept.join(","));
                out.push_str(block);
            }
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    format!("{}<style>{out}{}", &svg[..open], &svg[close..])
}

/// Just past the `}` closing the block that opens at `open`.
fn block_end(css: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in css[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Mindmap CSS (as in mermaid.js) sets `stroke-width: 17 - 3 * (depth + 1)`,
/// negative from depth 5. Browsers ignore the invalid value and draw 1px;
/// resvg draws nothing, so deep branches lose their edges. Clamp to 1.
fn clamp_negative_stroke_widths(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(at) = rest.find("stroke-width:-") {
        out.push_str(&rest[..at]);
        out.push_str("stroke-width:1");
        let tail = &rest[at + "stroke-width:-".len()..];
        let number_len = tail
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(tail.len());
        rest = &tail[number_len..];
    }
    out.push_str(rest);
    out
}
