//! Link suggestions in Live Preview: typing after `[[` opens a pop-up of
//! notes at the caret, the rows and the text they insert coming from the
//! owner ([`Links::suggest`]), the same as Source mode's.

use std::ops::Range;

use gpui_kit::base::v_flex;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::LivePreview;

/// What Live Preview asks its owner about links.
pub trait Links {
    /// Whether a link's target (a note, maybe with a `#heading`) resolves
    /// in the vault: those that don't are drawn apart.
    fn resolves(&self, target: &str) -> bool;

    /// What to suggest with the caret at `caret` in `text`: nothing unless
    /// it's in a wikilink being typed.
    fn suggest(&self, text: &str, caret: usize) -> Option<Suggestions>;
}

/// The suggestions for a link being typed.
pub struct Suggestions {
    /// The source a chosen row replaces: the typed name, and the rest of
    /// the link if it's already closed.
    pub replace: Range<usize>,
    /// Best first.
    pub rows: Vec<Suggestion>,
}

/// One row of the pop-up.
pub struct Suggestion {
    /// What the row says: a note's title, an alias, or "Create …".
    pub label: String,
    /// Said after it, muted: the note's folder, or the note an alias is
    /// for.
    pub detail: Option<String>,
    /// What choosing it writes in place of [`Suggestions::replace`],
    /// the closing `]]` included.
    pub insert: String,
}

/// The open pop-up: the suggestions, the row chosen, and its scroll.
pub(crate) struct Popup {
    suggestions: Suggestions,
    chosen: usize,
    scroll: ScrollHandle,
}

impl Popup {
    /// The row below the chosen one (above, if not `down`), wrapping, kept
    /// in view.
    fn step(&mut self, down: bool) {
        let len = self.suggestions.rows.len();
        self.chosen = if down {
            (self.chosen + 1) % len
        } else {
            (self.chosen + len - 1) % len
        };
        self.scroll.scroll_to_item(self.chosen);
    }

    /// What choosing row `row` does: the range to replace and the text to
    /// write there.
    fn choose(self, row: usize) -> Option<(Range<usize>, String)> {
        let Suggestions { replace, rows } = self.suggestions;
        let chosen = rows.into_iter().nth(row)?;
        Some((replace, chosen.insert))
    }
}

/// The pop-up's width.
const POPUP_WIDTH: Pixels = px(320.);
/// The pop-up's tallest: past it, its rows scroll.
const POPUP_MAX_HEIGHT: Pixels = px(280.);

impl LivePreview {
    /// After typing or pasting: asks for suggestions at the caret, opening,
    /// refiltering or closing the pop-up.
    pub(crate) fn update_suggestions(&mut self, cx: &mut Context<Self>) {
        let selection = self.doc.selection();
        self.popup = selection
            .is_empty()
            .then(|| self.links.suggest(self.doc.text(), selection.start))
            .flatten()
            .filter(|s| !s.rows.is_empty())
            .map(|suggestions| Popup {
                suggestions,
                chosen: 0,
                scroll: ScrollHandle::new(),
            });
        cx.notify();
    }

    /// Closes the pop-up; says whether it was open.
    pub(crate) fn close_suggestions(&mut self, cx: &mut Context<Self>) -> bool {
        let was_open = self.popup.take().is_some();
        if was_open {
            cx.notify();
        }
        was_open
    }

    /// Up or Down while the pop-up is open: the row above or below. Says
    /// whether the pop-up took the key.
    pub(crate) fn step_suggestion(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some(popup) = self.popup.as_mut() else {
            return false;
        };
        popup.step(down);
        cx.notify();
        true
    }

    /// Enter or Tab while the pop-up is open: writes the chosen row's link.
    /// Says whether the pop-up took the key.
    pub(crate) fn accept_chosen_suggestion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(row) = self.popup.as_ref().map(|p| p.chosen) else {
            return false;
        };
        self.accept_suggestion(row, cx);
        true
    }

    /// Writes row `row`'s link in place of the typed name, the caret after
    /// it: one undo step.
    fn accept_suggestion(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some((replace, insert)) = self.popup.take().and_then(|p| p.choose(row)) else {
            return;
        };
        let caret = replace.start + insert.len();
        self.edited(cx, |doc| doc.replace(replace, &insert, caret..caret));
    }

    /// The pop-up under the caret, while it's open.
    pub(crate) fn render_suggestions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popup = self.popup.as_ref()?;
        let caret = self.caret_bounds()?;
        let theme = cx.theme();
        let rows = popup.suggestions.rows.iter().enumerate().map(|(ix, row)| {
            div()
                .id(("suggestion", ix))
                .flex()
                .gap_2()
                .px_2()
                .py_1()
                .rounded(theme.radius)
                .text_sm()
                .when(ix == popup.chosen, |this| this.bg(theme.accent))
                .hover(|this| this.bg(theme.accent.opacity(0.6)))
                .child(div().flex_none().child(row.label.clone()))
                .when_some(row.detail.clone(), |this, detail| {
                    this.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(detail),
                    )
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.accept_suggestion(ix, cx);
                    }),
                )
        });
        let list = v_flex()
            .id("suggestions")
            .occlude()
            .track_scroll(&popup.scroll)
            .w(POPUP_WIDTH)
            .max_h(POPUP_MAX_HEIGHT)
            .overflow_y_scroll()
            .p_1()
            .bg(theme.tokens.popover)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .shadow_md()
            .children(rows);
        let at = point(caret.left(), caret.bottom() + px(2.));
        Some(deferred(anchored().position(at).snap_to_window().child(list)).into_any_element())
    }
}
