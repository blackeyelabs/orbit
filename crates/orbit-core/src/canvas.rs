//! JSON Canvas files (`.canvas`), read only for search until canvases can be
//! edited (ADR 0005).

use serde_json::Value;

/// The text a canvas shows of its own: text cards' text, group labels and
/// the labels on its arrows, one after another. Link and file cards show
/// what they point at, which isn't the canvas's text. A file that isn't a
/// canvas has none.
pub(crate) fn card_text(json: &str) -> String {
    let Ok(canvas) = serde_json::from_str::<Value>(json) else {
        return String::new();
    };
    let list = |key: &str| {
        canvas[key]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    let cards = list("nodes")
        .iter()
        .filter_map(|node| match node["type"].as_str() {
            Some("text") => node["text"].as_str(),
            Some("group") => node["label"].as_str(),
            _ => None,
        });
    let arrows = list("edges")
        .iter()
        .filter_map(|edge| edge["label"].as_str());
    cards.chain(arrows).collect::<Vec<_>>().join("\n")
}
