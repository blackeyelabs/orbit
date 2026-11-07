//! Callouts in Live Preview: each line of one drawn as its part of a tinted
//! box with a coloured edge, as the preview drew them, the title line in
//! the callout's colour. While the caret is in the callout, its source shows
//! instead, as a quote.

use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use live_preview::blocks::CalloutLine;
use md_engine::CalloutKind;

/// A callout line as drawn.
pub(crate) struct CalloutFrame {
    /// Its callout's kind, and whether it's the title line.
    pub line: CalloutLine,
    /// It's the callout's last line, where its box ends.
    pub last: bool,
}

/// A callout's colour, as the preview had it.
fn colour(kind: CalloutKind, theme: &Theme) -> Hsla {
    match kind {
        CalloutKind::Note | CalloutKind::Info | CalloutKind::Todo => theme.info,
        CalloutKind::Abstract | CalloutKind::Tip | CalloutKind::Example => theme.primary,
        CalloutKind::Success => theme.success,
        CalloutKind::Question | CalloutKind::Warning => theme.warning,
        CalloutKind::Failure | CalloutKind::Danger | CalloutKind::Bug => theme.danger,
        CalloutKind::Quote => theme.muted_foreground,
    }
}

/// A callout line's rendered `node` in its part of the callout's box: the
/// title's top, a body line, or the bottom.
pub(crate) fn callout_frame(frame: &CalloutFrame, node: AnyElement, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colour = colour(frame.line.kind, theme);
    div()
        .w_full()
        .px_3()
        .border_l_4()
        .border_color(colour)
        .bg(colour.opacity(0.08))
        .when(frame.line.title, |this| {
            this.pt_2()
                .rounded_t(theme.radius)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colour)
        })
        .when(frame.last, |this| this.pb_2().rounded_b(theme.radius))
        .child(node)
        .into_any_element()
}
