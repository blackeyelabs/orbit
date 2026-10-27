//! The right panel, laid out as on the design canvas: the open note's
//! Outline, then its Backlinks, each linking note with its links in context.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use md_engine::Heading;
use orbit_core::{Backlink, Mention};

use crate::window::OrbitWindow;

// Canvas sizes at 100% (300, 18, 16, 26 and 14 px), in rems.
const WIDTH: f32 = 18.75;
const PADDING_Y: f32 = 1.125;
const PADDING_X: f32 = 1.;
const SECTION_GAP: f32 = 1.625;
/// How far each heading level is indented in the Outline.
const LEVEL_INDENT: f32 = 0.875;

/// The panel for the open note, or a hint when none is.
pub fn render(
    note: Option<(&[Heading], &[Backlink])>,
    cx: &mut Context<OrbitWindow>,
) -> AnyElement {
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
    match note {
        Some((outline, backlinks)) => panel
            .child(render_outline(outline, cx))
            .child(render_backlinks(backlinks, cx)),
        None => panel.child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Open a note to see its outline and backlinks."),
        ),
    }
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
        let title = note_title(backlink.from.name());
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

/// A note's name as a title: without a markdown note's extension.
fn note_title(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((title, ext)) if ext.eq_ignore_ascii_case("md") => title.to_string(),
        _ => name.to_string(),
    }
}
