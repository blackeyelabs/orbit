//! The rendered half of the Split view: Kit's markdown view plus Orbit's
//! plugins for front matter, callouts and math, with links and images
//! resolved inside the vault.
//!
//! Kit's markdown knows nothing of wikilinks, so before rendering every
//! vault link (wikilink, embed or markdown link) is rewritten as a markdown
//! link or image to `orbit-link:` (resolved) or `orbit-new:` (unresolved,
//! drawn apart by a plugin) with the link's target.

use std::{
    collections::HashMap,
    hash::{Hash as _, Hasher as _},
    sync::{Arc, Mutex},
};

use diagrams::{Math, MathStyle, render_math};
use gpui_kit::base::text::{
    InlineElement, InlineRenderContext, MarkdownExtensions, MarkdownNode, MarkdownParseContext,
    MarkdownPlugin, TextView, markdown_ast::Node,
};
use gpui_kit::base::v_flex;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::text::FrontmatterPlugin;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use md_engine::{CalloutKind, LinkStyle, callout};
use orbit_core::{Vault, VaultPath};

/// Rewritten resolved and unresolved links point at these.
const LINK_SCHEME: &str = "orbit-link:";
const NEW_SCHEME: &str = "orbit-new:";

/// Called with a followed link's target (`Note#Heading`).
pub type FollowLink = Arc<dyn Fn(&str, &mut Window, &mut App) + Send + Sync>;

/// What a preview needs beyond the note's text.
#[derive(Clone)]
pub struct PreviewContext {
    vault: Arc<Vault>,
    note: VaultPath,
    follow: FollowLink,
    /// Image link → its decoded Attachment, so a frame doesn't read the disk
    /// per image. Only found images are kept: a missing one is looked for
    /// again, in case it's added later.
    images: Arc<Mutex<HashMap<SharedString, Arc<Image>>>>,
}

impl PreviewContext {
    pub fn new(vault: Arc<Vault>, note: VaultPath, follow: FollowLink) -> Self {
        PreviewContext {
            vault,
            note,
            follow,
            images: Arc::default(),
        }
    }

    /// The note's text as the preview renders it: its vault links rewritten
    /// as markdown links (images, for embedded images) that say whether
    /// they resolve. Everything else is left as written.
    pub fn rewrite_links(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut copied = 0;
        for link in md_engine::links(text) {
            out.push_str(&text[copied..link.span.start]);
            copied = link.span.end;
            let resolved = self.vault.resolve(&self.note, link.path());
            let scheme = if resolved.is_some() {
                LINK_SCHEME
            } else {
                NEW_SCHEME
            };
            let image = link.embed
                && resolved
                    .as_ref()
                    .is_some_and(|p| image_format(p.name()).is_some());
            out.push_str(if image { "![" } else { "[" });
            // A wikilink's text is shown as written; a markdown link's is
            // markdown already.
            match link.style {
                LinkStyle::Wiki => out.push_str(&escape_markdown(&link.display)),
                LinkStyle::Markdown => out.push_str(&link.display),
            }
            out.push_str("](");
            out.push_str(scheme);
            out.push_str(&percent_encode(&link.target));
            out.push(')');
        }
        out.push_str(&text[copied..]);
        out
    }

    /// A click on a link: vault links are followed, anything else (a web
    /// address) opens in its app.
    fn click(&self, url: &str, window: &mut Window, cx: &mut App) {
        let target = url
            .strip_prefix(LINK_SCHEME)
            .or_else(|| url.strip_prefix(NEW_SCHEME));
        match target {
            Some(target) => {
                let target = md_engine::percent_decode(target).unwrap_or_else(|| target.into());
                (self.follow)(&target, window, cx)
            }
            None => cx.open_url(url),
        }
    }

    /// Attachments are read through the vault; anything else (a web
    /// address, say) is left to GPUI.
    fn image_source(&self, uri: &SharedUri) -> ImageSource {
        let link: SharedString = uri.to_string().into();
        let mut images = self.images.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(image) = images.get(&link) {
            return image.clone().into();
        }
        let target = link.strip_prefix(LINK_SCHEME).unwrap_or(&link);
        let format = image_format(target);
        let read = || self.vault.read_linked_attachment(&self.note, target);
        match format.and_then(|f| Some((f, read()?))) {
            Some((format, bytes)) => {
                let image = Arc::new(Image::from_bytes(format, bytes));
                images.insert(link, image.clone());
                image.into()
            }
            None => uri.clone().into(),
        }
    }
}

/// The image format a link's extension names.
fn image_format(link: &str) -> Option<ImageFormat> {
    let path = link.split(['#', '?']).next()?;
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::Webp,
        "gif" => ImageFormat::Gif,
        "svg" => ImageFormat::Svg,
        "bmp" => ImageFormat::Bmp,
        "tif" | "tiff" => ImageFormat::Tiff,
        _ => return None,
    })
}

/// The rendered markdown of `text`.
pub fn preview(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    context: &PreviewContext,
    window: &Window,
    cx: &App,
) -> TextView {
    let (images, links) = (context.clone(), context.clone());
    let math = MathOptions::new(window, cx);
    TextView::markdown(id, text)
        // Changing the theme or scale must retypeset math, which is parsed
        // once per text; everything else renders without reparsing.
        .markdown_extensions(
            MarkdownExtensions::default()
                .frontmatter()
                .parser_revision(math.revision()),
        )
        .plugin(FrontmatterPlugin::new())
        .plugin(CalloutPlugin(context.clone()))
        .plugin(InlineMathPlugin(math.clone()))
        .plugin(BlockMathPlugin(math))
        .plugin(UnresolvedLinkPlugin(context.clone()))
        .image_source(move |uri| images.image_source(uri))
        .on_link_click(move |url, _, window, cx| links.click(url, window, cx))
        .selectable(true)
}

/// Markdown punctuation in link text, escaped so it shows as written.
fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A link target as a URL: anything but letters, digits, `/`, `#` and
/// `-._~` is percent-encoded.
fn percent_encode(target: &str) -> String {
    let mut out = String::with_capacity(target.len());
    for byte in target.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'#' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Unresolved links: fainter than links that resolve, and following one
/// creates its note.
struct UnresolvedLinkPlugin(PreviewContext);

/// An unresolved link's target and text.
struct UnresolvedLink {
    url: SharedString,
    text: SharedString,
    key: u64,
}

impl MarkdownPlugin for UnresolvedLinkPlugin {
    fn name(&self) -> &str {
        "orbit-unresolved-link"
    }

    fn parse(&self, node: &Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let Node::Link(link) = node else {
            return None;
        };
        if !link.url.starts_with(NEW_SCHEME) {
            return None;
        }
        let text = node.to_string();
        let mut hasher = std::hash::DefaultHasher::new();
        (
            &link.url,
            cx.offset(),
            node.position().map(|p| p.start.offset),
        )
            .hash(&mut hasher);
        Some(
            MarkdownNode::new(
                "orbit-unresolved-link",
                UnresolvedLink {
                    url: link.url.clone().into(),
                    text: text.clone().into(),
                    key: hasher.finish(),
                },
            )
            .text(text),
        )
    }

    fn render(&self, node: &MarkdownNode, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(link) = node.data::<UnresolvedLink>() else {
            return div().into_any_element();
        };
        let context = self.0.clone();
        let url = link.url.clone();
        div()
            .id(("unresolved-link", link.key))
            .text_color(cx.theme().link.opacity(0.6))
            .cursor_pointer()
            .hover(|this| this.underline())
            .child(link.text.clone())
            .on_click(move |_, window, cx| context.click(&url, window, cx))
            .into_any_element()
    }
}

/// `> [!type] Title` blockquotes, drawn as a tinted box whose title and body
/// are previews of their own.
struct CalloutPlugin(PreviewContext);

struct CalloutData {
    kind: CalloutKind,
    title: SharedString,
    body: SharedString,
    /// Distinguishes the nested previews' element ids.
    key: u64,
}

impl MarkdownPlugin for CalloutPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "orbit-callout"
    }

    fn parse(&self, node: &Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let Node::Blockquote(_) = node else {
            return None;
        };
        let source = cx.node_source(node)?;
        let callout = callout(source)?;
        let mut hasher = std::hash::DefaultHasher::new();
        (source, cx.offset(), node.position().map(|p| p.start.offset)).hash(&mut hasher);
        Some(
            MarkdownNode::new(
                "orbit-callout",
                CalloutData {
                    kind: callout.kind,
                    title: callout.title.clone().into(),
                    body: callout.body.clone().into(),
                    key: hasher.finish(),
                },
            )
            .text(format!("{}\n{}", callout.title, callout.body))
            .markdown(source),
        )
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(data) = node.data::<CalloutData>() else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let colour = match data.kind {
            CalloutKind::Note | CalloutKind::Info | CalloutKind::Todo => theme.info,
            CalloutKind::Abstract | CalloutKind::Tip | CalloutKind::Example => theme.primary,
            CalloutKind::Success => theme.success,
            CalloutKind::Question | CalloutKind::Warning => theme.warning,
            CalloutKind::Failure | CalloutKind::Danger | CalloutKind::Bug => theme.danger,
            CalloutKind::Quote => theme.muted_foreground,
        };
        let key = data.key;
        v_flex()
            .my_2()
            .px_3()
            .py_2()
            .gap_1()
            .rounded(theme.radius)
            .border_l_4()
            .border_color(colour)
            .bg(colour.opacity(0.08))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colour)
                    .child(preview(
                        SharedString::from(format!("callout-title-{key}")),
                        data.title.clone(),
                        &self.0,
                        window,
                        cx,
                    )),
            )
            .when(!data.body.trim().is_empty(), |this| {
                this.child(preview(
                    SharedString::from(format!("callout-body-{key}")),
                    data.body.clone(),
                    &self.0,
                    window,
                    cx,
                ))
            })
            .into_any_element()
    }
}

/// Typesetting settings for this frame: the theme's text colour and size, at
/// the window's pixel density so formulas stay sharp.
#[derive(Clone, PartialEq)]
struct MathOptions {
    colour: String,
    font_px: f32,
    scale: f32,
}

impl MathOptions {
    fn new(window: &Window, cx: &App) -> Self {
        let rgb = cx.theme().foreground.to_rgb();
        let channel = |c: f32| (c.clamp(0., 1.) * 255.).round() as u8;
        MathOptions {
            colour: format!(
                "#{:02x}{:02x}{:02x}",
                channel(rgb.r),
                channel(rgb.g),
                channel(rgb.b)
            ),
            font_px: f32::from(window.rem_size()) * 1.1,
            scale: window.scale_factor(),
        }
    }

    fn revision(&self) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        (&self.colour, self.font_px.to_bits(), self.scale.to_bits()).hash(&mut hasher);
        hasher.finish()
    }

    /// Typesets at device pixels; sizes come back in logical pixels.
    fn typeset(&self, latex: &str, style: MathStyle) -> TypesetMath {
        render_math(latex, style, &self.colour, self.font_px * self.scale)
            .map(|math| {
                let Math {
                    svg,
                    width,
                    height,
                    baseline,
                } = math;
                TypesetMath::Ok {
                    image: Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())),
                    width: width / self.scale,
                    height: height / self.scale,
                    baseline: baseline / self.scale,
                }
            })
            .unwrap_or_else(|e| TypesetMath::Error(e.into()))
    }
}

enum TypesetMath {
    Ok {
        image: Arc<Image>,
        width: f32,
        height: f32,
        baseline: f32,
    },
    /// The LaTeX didn't parse; shown as source with the reason.
    Error(SharedString),
}

/// `$…$`, typeset on the text baseline.
struct InlineMathPlugin(MathOptions);

impl MarkdownPlugin for InlineMathPlugin {
    fn name(&self) -> &str {
        "orbit-inline-math"
    }

    fn parse(&self, node: &Node, _: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let Node::InlineMath(math) = node else {
            return None;
        };
        Some(
            MarkdownNode::new(
                "orbit-inline-math",
                self.0.typeset(&math.value, MathStyle::Inline),
            )
            .text(math.value.clone()),
        )
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        _: &InlineRenderContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        match node.data::<TypesetMath>()? {
            TypesetMath::Ok {
                image,
                width,
                height,
                baseline,
            } => Some(
                InlineElement::new(img(image.clone()).w(px(*width)).h(px(*height)))
                    .with_baseline(px(*baseline)),
            ),
            TypesetMath::Error(_) => Some(InlineElement::new(
                div()
                    .text_color(cx.theme().danger)
                    .child(format!("${}$", node.as_text())),
            )),
        }
    }
}

/// `$$…$$` blocks, centred on their own line.
struct BlockMathPlugin(MathOptions);

impl MarkdownPlugin for BlockMathPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "orbit-block-math"
    }

    fn parse(&self, node: &Node, _: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let Node::Math(math) = node else {
            return None;
        };
        Some(
            MarkdownNode::new(
                "orbit-block-math",
                self.0.typeset(&math.value, MathStyle::Display),
            )
            .text(math.value.clone()),
        )
    }

    fn render(&self, node: &MarkdownNode, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let row = div().my_2().flex().justify_center();
        match node.data::<TypesetMath>() {
            Some(TypesetMath::Ok {
                image,
                width,
                height,
                ..
            }) => row
                .child(img(image.clone()).w(px(*width)).h(px(*height)))
                .into_any_element(),
            Some(TypesetMath::Error(error)) => v_flex()
                .my_2()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(format!("Math error: {error}"))
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(node.as_text().to_string()),
                )
                .into_any_element(),
            None => div().into_any_element(),
        }
    }
}
