//! Images in Live Preview: a leaf's text split around the images in it,
//! each drawn by Kit as an inline image, read from the vault through the
//! owner ([`crate::Links::image`]). An image the vault doesn't have is a
//! small grey box, so it can still be seen and clicked.

use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Mutex},
};

use gpui_kit::base::text::editor_host::{ImageNode, InlineNode, TextMark};
use gpui_kit::*;
use live_preview::conceal::{Image as ImageSpec, OBJECT, Target};

use crate::Links;

/// Where an image read from the vault is, to Kit: this, then its target.
const VAULT_SCHEME: &str = "orbit-image:";

/// An image the vault doesn't have: a grey box with a border.
const MISSING: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='48' height='32'><rect x='0.5' y='0.5' width='47' height='31' rx='4' fill='%23888' fill-opacity='0.15' stroke='%23888' stroke-opacity='0.6'/></svg>";

/// The images one frame draws, by the address Kit asks for them by.
#[derive(Clone, Default)]
pub(crate) struct FrameImages(Arc<Mutex<HashMap<String, Arc<Image>>>>);

impl FrameImages {
    /// The image Kit asks for by `uri`, if this frame has it; else the
    /// address as given (a web address, the missing box).
    pub(crate) fn source(&self, uri: &SharedUri) -> ImageSource {
        let images = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match images.get(uri.as_ref()) {
            Some(image) => image.clone().into(),
            None => uri.clone().into(),
        }
    }

    /// Kit's node for `image`: read from the vault through `links` (kept
    /// for this frame), a web address as is, or the missing box.
    fn node(&self, image: &ImageSpec, links: &dyn Links) -> ImageNode {
        let url = match &image.target {
            Target::Url(url) => url.clone(),
            Target::Note(target) => match links.image(target) {
                Some(found) => {
                    let url = format!("{VAULT_SCHEME}{target}");
                    let mut images = self.0.lock().unwrap_or_else(|e| e.into_inner());
                    images.insert(url.clone(), found);
                    url
                }
                None => MISSING.to_string(),
            },
        };
        let mut node = ImageNode::default();
        node.url = url.into();
        node.alt = (!image.alt.is_empty()).then(|| image.alt.clone().into());
        node.width = image.width.map(|w| px(w as f32).into());
        node.height = image.height.map(|h| px(h as f32).into());
        node
    }

    /// A leaf's display `text` as Kit's inline nodes: its runs of text, with
    /// their `marks`, and its `images` between them.
    pub(crate) fn inline_nodes(
        &self,
        text: &str,
        marks: Vec<(Range<usize>, TextMark)>,
        images: &[ImageSpec],
        links: &dyn Links,
    ) -> Vec<InlineNode> {
        let mut nodes = Vec::with_capacity(2 * images.len() + 1);
        let mut start = 0;
        let text_run = |range: Range<usize>, nodes: &mut Vec<InlineNode>| {
            let run_marks = marks
                .iter()
                .filter(|(r, _)| r.start < range.end && range.start < r.end)
                .map(|(r, mark)| {
                    let clipped = r.start.max(range.start)..r.end.min(range.end);
                    (
                        clipped.start - range.start..clipped.end - range.start,
                        mark.clone(),
                    )
                })
                .collect();
            nodes.push(InlineNode::new(text[range].to_string()).marks(run_marks));
        };
        for image in images {
            if start < image.at {
                text_run(start..image.at, &mut nodes);
            }
            let mut node = self.node(image, links);
            // An image in a link is the link's, as its text would be.
            node.link = marks
                .iter()
                .find(|(r, mark)| r.contains(&image.at) && mark.link.is_some())
                .and_then(|(_, mark)| mark.link.clone());
            nodes.push(InlineNode::image(node));
            start = image.at + OBJECT.len();
        }
        if start < text.len() || nodes.is_empty() {
            text_run(start..text.len(), &mut nodes);
        }
        nodes
    }
}
