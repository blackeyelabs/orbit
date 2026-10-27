//! The right panel, laid out as on the design canvas: the open note's
//! Outline, then its Backlinks, each linking note with its links in context,
//! then the vault's Tags, and the notes of the one chosen.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use md_engine::Heading;
use orbit_core::{Backlink, Mention, TagCount, VaultPath, note_title, same_tag};

use crate::window::OrbitWindow;

// Canvas sizes at 100% (300, 18, 16, 26 and 14 px), in rems.
const WIDTH: f32 = 18.75;
const PADDING_Y: f32 = 1.125;
const PADDING_X: f32 = 1.;
const SECTION_GAP: f32 = 1.625;
/// How far each heading level is indented in the Outline.
const LEVEL_INDENT: f32 = 0.875;

/// What the panel shows.
pub struct Contents<'a> {
    /// The open note's headings; `None` with no note open.
    pub outline: Option<&'a [Heading]>,
    /// The open note's backlinks.
    pub backlinks: &'a [Backlink],
    /// Every tag in the vault, with its count.
    pub tags: &'a [TagCount],
    /// The tag chosen in the Tags section, and its notes.
    pub tagged: Option<(&'a str, &'a [VaultPath])>,
}

/// The panel: the open note's sections, or a hint when none is, then Tags.
pub fn render(contents: Contents, cx: &mut Context<OrbitWindow>) -> AnyElement {
    let panel = v_flex()
        .id("right-panel")
        .w(rems(WIDTH))
        .flex_none()
        .h_full()
        .overflow_y_scroll()
        .py(rems(PADDING_Y))
        .px(rems(PADDING_X))
        .gap(rems(SECTION_GAP))
        .border_l_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().sidebar);
    match contents.outline {
        Some(outline) => panel
            .child(render_outline(outline, cx))
            .child(render_backlinks(contents.backlinks, cx)),
        None => panel.child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Open a note to see its outline and backlinks."),
        ),
    }
    .child(render_tags(contents.tags, contents.tagged, cx))
    .into_any_element()
}

fn render_outline(outline: &[Heading], cx: &mut Context<OrbitWindow>) -> AnyElement {
    // The note's highest level is the left edge, whatever it is.
    let top = outline.iter().map(|h| h.level).min().unwrap_or(1);
    let items = outline.iter().enumerate().map(|(ix, heading)| {
        let offset = heading.offset;
        let depth = f32::from(heading.level - top);
        div()
            .id(("outline", ix))
            .py_1()
            .pr_2()
            .pl(rems(0.5 + LEVEL_INDENT * depth))
            .rounded_md()
            .text_sm()
            .when(heading.level == top, |this| {
                this.font_weight(FontWeight::SEMIBOLD)
            })
            .hover(|this| this.bg(cx.theme().list_hover))
            .cursor_pointer()
            .child(heading.text.clone())
            .on_click(
                cx.listener(move |this, _, window, cx| this.go_to_line_of(offset, window, cx)),
            )
    });
    v_flex()
        .gap_2()
        .child(section_header(IconName::List, "OUTLINE", None, cx))
        .child(
            v_flex()
                .gap_px()
                .children(items)
                .when(outline.is_empty(), |this| {
                    this.child(empty("No headings", cx))
                }),
        )
        .into_any_element()
}

fn render_backlinks(backlinks: &[Backlink], cx: &mut Context<OrbitWindow>) -> AnyElement {
    let cards = backlinks.iter().enumerate().map(|(ix, backlink)| {
        let from = backlink.from.clone();
        let title = note_title(&backlink.from).to_string();
        v_flex()
            .id(("backlink", ix))
            .gap_1()
            .py(rems(0.5625))
            .px(rems(0.625))
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .hover(|this| this.bg(cx.theme().list_hover))
            .cursor_pointer()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .children(backlink.mentions.iter().map(|m| mention(m, cx)))
            .on_click(
                cx.listener(move |this, _, window, cx| this.open_note(from.clone(), window, cx)),
            )
    });
    let count = backlinks.len().to_string();
    v_flex()
        .gap_2()
        .child(section_header(IconName::Link, "BACKLINKS", Some(count), cx))
        .children(cards)
        .when(backlinks.is_empty(), |this| {
            this.child(empty("No notes link here", cx))
        })
        .into_any_element()
}

fn render_tags(
    tags: &[TagCount],
    tagged: Option<(&str, &[VaultPath])>,
    cx: &mut Context<OrbitWindow>,
) -> AnyElement {
    let chips = tags.iter().enumerate().map(|(ix, tag)| {
        let is_chosen = tagged.is_some_and(|(chosen, _)| same_tag(chosen, &tag.name));
        let name = tag.name.clone();
        let colour = tag_colour(&tag.name, cx);
        h_flex()
            .id(("tag", ix))
            .h(rems(1.5))
            .px_3()
            .gap_2()
            .rounded_full()
            .border_1()
            .border_color(colour)
            .bg(colour.opacity(if is_chosen { 0.5 } else { 0.2 }))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .hover(|this| this.bg(colour.opacity(0.35)))
            .child(format!("#{}", tag.name))
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(tag.count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.choose_tag(Some(&name), cx)))
    });
    let notes = tagged.map(|(tag, notes)| {
        let rows =
            notes.iter().enumerate().map(|(ix, path)| {
                let open = path.clone();
                h_flex()
                    .id(("tagged", ix))
                    .gap_2()
                    .py_1()
                    .px_2()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|this| this.bg(cx.theme().list_hover))
                    .child(IconName::FileText)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .child(note_title(path).to_string()),
                    )
                    .when(!path.parent().is_root(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(path.parent().to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_note(open.clone(), window, cx)
                    }))
            });
        v_flex()
            .gap_px()
            .child(
                h_flex()
                    .px_2()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_1().child(format!("Notes tagged #{tag}")))
                    .child(
                        Button::new("clear-tag")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("Show all tags")
                            .on_click(cx.listener(|this, _, _, cx| this.choose_tag(None, cx))),
                    ),
            )
            .children(rows)
    });
    v_flex()
        .gap_3()
        .child(section_header(IconName::Hash, "TAGS", None, cx))
        .child(h_flex().flex_wrap().gap_2().children(chips))
        .when(tags.is_empty(), |this| this.child(empty("No tags yet", cx)))
        .children(notes)
        .into_any_element()
}

/// A tag's colour: one of the theme's five, always the same for the tag.
fn tag_colour(name: &str, cx: &App) -> Hsla {
    let theme = cx.theme();
    let colours = [
        theme.chart_1,
        theme.chart_2,
        theme.chart_3,
        theme.chart_4,
        theme.chart_5,
    ];
    let hash = name.to_lowercase().bytes().fold(0usize, |hash, b| {
        hash.wrapping_mul(31).wrapping_add(b.into())
    });
    colours[hash % colours.len()]
}

/// A link's line, the link picked out.
fn mention(mention: &Mention, cx: &App) -> impl IntoElement {
    let link = HighlightStyle {
        color: Some(cx.theme().link),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(
            StyledText::new(mention.context.clone())
                .with_highlights([(mention.link.clone(), link)]),
        )
}

fn section_header(
    icon: IconName,
    label: &'static str,
    count: Option<String>,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(cx.theme().muted_foreground)
        .child(icon)
        .child(div().flex_1().child(label))
        .when_some(count, |this, count| this.child(count))
}

fn empty(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .px_2()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}
