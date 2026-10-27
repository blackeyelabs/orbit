//! Vault search's results in the sidebar, laid out as on the design canvas:
//! how many matched, then each note with its lines, the matches marked; or
//! what to try when nothing matched. A banner says while the vault is still
//! being indexed.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use orbit_core::{IndexingProgress, SearchResult, SearchResults, Snippet};

use crate::window::OrbitWindow;

/// What a search found, or why nothing.
pub fn render(query: &str, found: &SearchResults, cx: &mut Context<OrbitWindow>) -> AnyElement {
    let body = if found.results.is_empty() && !found.complete {
        // Not "no matches": they may not be indexed yet.
        div()
            .px_1()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child("Searching…")
            .into_any_element()
    } else if found.results.is_empty() {
        v_flex()
            .gap_1()
            .px_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("No matches for “{query}”")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Check the spelling, try fewer words, or remove a filter."),
            )
            .into_any_element()
    } else {
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .px_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(count(found))
                    .child("Best match"),
            )
            .children(
                found
                    .results
                    .iter()
                    .enumerate()
                    .map(|(ix, result)| render_result(ix, result, cx)),
            )
            .into_any_element()
    };
    v_flex()
        .id("search-results")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .gap_3()
        .px_3()
        .pb_3()
        .when(!found.complete, |this| {
            this.child(
                div()
                    .px_2()
                    .py_1()
                    .border_l_2()
                    .border_color(cx.theme().info)
                    .text_xs()
                    .child("Still indexing. Results may be incomplete."),
            )
        })
        .child(body)
        .into_any_element()
}

/// "6 matches in 3 notes", the numbers picked out.
fn count(found: &SearchResults) -> impl IntoElement {
    let plural = |n: usize, one: &str, many: &str| if n == 1 { one } else { many }.to_string();
    h_flex()
        .gap_1()
        .when(found.matches > 0, |this| {
            this.child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(found.matches.to_string()),
            )
            .child(plural(found.matches, "match in", "matches in"))
        })
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .child(found.matching_notes.to_string()),
        )
        .child(plural(found.matching_notes, "note", "notes"))
}

fn render_result(ix: usize, result: &SearchResult, cx: &mut Context<OrbitWindow>) -> AnyElement {
    let path = result.path.clone();
    v_flex()
        .id(("search-result", ix))
        .gap_1()
        .rounded_md()
        .cursor_pointer()
        .hover(|this| this.bg(cx.theme().list_hover))
        .child(
            h_flex()
                .gap_2()
                .px_1()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(IconName::FileText)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(result.title.clone()),
                )
                .when(result.matches > 0, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(result.matches.to_string()),
                    )
                }),
        )
        .children(
            result
                .snippets
                .iter()
                .map(|snippet| render_snippet(snippet, cx)),
        )
        .on_click(cx.listener(move |this, _, window, cx| this.open_file(path.clone(), window, cx)))
        .into_any_element()
}

fn render_snippet(snippet: &Snippet, cx: &App) -> impl IntoElement {
    let marked = HighlightStyle {
        background_color: Some(cx.theme().selection),
        color: Some(cx.theme().foreground),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    div()
        .pl(rems(1.625))
        .pr_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(
            StyledText::new(snippet.text.clone()).with_highlights(
                snippet
                    .highlights
                    .iter()
                    .map(|range| (range.clone(), marked)),
            ),
        )
}

/// "Indexing 2,340 of 8,100 notes".
pub fn progress_text(progress: IndexingProgress) -> String {
    format!(
        "Indexing {} of {} notes",
        thousands(progress.done),
        thousands(progress.total)
    )
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
