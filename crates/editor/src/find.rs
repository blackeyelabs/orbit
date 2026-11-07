//! Find and replace in Live Preview: the find bar Ctrl+F and Ctrl+H open
//! above the note, as Kit's code editor has in Source mode. The search itself is the
//! document's ([`live_preview::Document::find`]); the current match is
//! selected, so it's revealed when it's inside hidden syntax.

use std::ops::Range;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, Input, InputEvent, InputState, OutdentInline, Replace, Search,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::LivePreview;

actions!(
    live_preview,
    [
        /// Ctrl+Alt+Enter in the find bar: replace every match.
        ReplaceAll
    ]
);

/// The find bar's key context.
const KEY_CONTEXT: &str = "LivePreviewFind";

/// Binds the find bar's own keys (Ctrl+F and Ctrl+H are the editor's).
pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "ctrl-alt-enter",
        ReplaceAll,
        Some(KEY_CONTEXT),
    )]);
}

/// The open find bar and what it found.
pub(crate) struct Find {
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    /// The replace field shows.
    replacing: bool,
    match_case: bool,
    /// Every match of the query, in source order.
    matches: Vec<Range<usize>>,
    /// The match that's selected, if one is.
    current: Option<usize>,
    _query_edits: Subscription,
}

impl Find {
    /// The matches other than the current one, to tint.
    pub(crate) fn other_matches(&self) -> impl Iterator<Item = &Range<usize>> {
        self.matches
            .iter()
            .enumerate()
            .filter(move |(ix, _)| Some(*ix) != self.current)
            .map(|(_, m)| m)
    }

    /// The first match starting at or after `offset`, else (wrapping) the
    /// first of all.
    fn first_from(&self, offset: usize) -> Option<usize> {
        let first = self.matches.iter().position(|m| m.start >= offset);
        first.or((!self.matches.is_empty()).then_some(0))
    }

    /// The match after the current one (before it, if not `forward`),
    /// wrapping; with none current, the first after the selection
    /// `from` (before it).
    fn next(&self, forward: bool, from: &Range<usize>) -> Option<usize> {
        let len = self.matches.len();
        if len == 0 {
            return None;
        }
        Some(match (self.current, forward) {
            (Some(ix), true) => (ix + 1) % len,
            (Some(ix), false) => (ix + len - 1) % len,
            (None, true) => self.first_from(from.end).unwrap_or(0),
            (None, false) => self
                .matches
                .iter()
                .rposition(|m| m.end <= from.start)
                .unwrap_or(len - 1),
        })
    }

    /// "2/5": the current match and how many there are, as Kit's bar says.
    fn label(&self) -> String {
        match self.current {
            Some(ix) => format!("{}/{}", ix + 1, self.matches.len()),
            None => format!("0/{}", self.matches.len()),
        }
    }
}

impl LivePreview {
    /// Ctrl+F or Ctrl+H: opens the bar (with the replace field for Ctrl+H),
    /// the selection as the query if there is one on a single line, and
    /// selects the match nearest the caret.
    pub(crate) fn open_find(
        &mut self,
        replacing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.find.is_none() {
            let query = cx.new(|cx| InputState::new(window, cx));
            let replacement = cx.new(|cx| InputState::new(window, cx));
            let query_edits = cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.search(cx);
                }
            });
            self.find = Some(Find {
                query,
                replacement,
                replacing,
                match_case: false,
                matches: Vec::new(),
                current: None,
                _query_edits: query_edits,
            });
        }
        let selected = &self.doc.text()[self.doc.selection()];
        let selected =
            (!selected.is_empty() && !selected.contains('\n')).then(|| selected.to_string());
        let Some(find) = self.find.as_mut() else {
            return;
        };
        find.replacing |= replacing;
        let query = find.query.clone();
        query.update(cx, |query, cx| {
            if let Some(selected) = selected {
                query.set_value(selected, window, cx);
            }
            query.select_all(window, cx);
            query.focus(window, cx);
        });
        self.search(cx);
    }

    /// Escape: closes the bar, back to the note where the match is.
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Finds the query afresh and selects the match at or after the
    /// selection's start (the first after the last one, wrapping).
    fn search(&mut self, cx: &mut Context<Self>) {
        let from = self.doc.selection().start;
        self.find_matches(cx);
        self.select_match(|find| find.first_from(from), cx);
    }

    /// Fills in the matches for the query as it is, none current.
    fn find_matches(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let query = find.query.read(cx).value();
        find.matches = self.doc.find(&query, find.match_case);
        find.current = None;
        cx.notify();
    }

    /// After the text changed (an edit, a reload) while the bar is open:
    /// the matches are found again, and the selection, if it's one, stays
    /// the current one.
    pub(crate) fn refresh_find(&mut self, cx: &mut Context<Self>) {
        self.find_matches(cx);
        let selection = self.doc.selection();
        if let Some(find) = self.find.as_mut() {
            find.current = find.matches.iter().position(|m| *m == selection);
        }
    }

    /// Selects the match `pick` chooses, scrolled into view.
    fn select_match(&mut self, pick: impl FnOnce(&Find) -> Option<usize>, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let Some(ix) = pick(find) else {
            return;
        };
        find.current = Some(ix);
        let range = find.matches[ix].clone();
        self.moved(cx, |doc| doc.select(range));
    }

    /// Enter or Shift+Enter: the next or previous match, wrapping.
    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let from = self.doc.selection();
        self.select_match(|find| find.next(forward, &from), cx);
    }

    /// Replace: the current match becomes the replacement, and the next
    /// match is selected.
    fn replace_current(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_ref() else {
            return;
        };
        let Some(range) = find.current.map(|ix| find.matches[ix].clone()) else {
            return self.step(true, cx);
        };
        let replacement = find.replacement.read(cx).value().to_string();
        let after = range.start + replacement.len();
        // The edit finds the matches again.
        self.edited(cx, |doc| {
            doc.replace(range.clone(), &replacement, range.start..after)
        });
        self.select_match(|find| find.first_from(after), cx);
    }

    /// Replace all: every match becomes the replacement, as one undo step.
    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_ref() else {
            return;
        };
        let matches = find.matches.clone();
        let replacement = find.replacement.read(cx).value().to_string();
        self.edited(cx, |doc| doc.replace_all(&matches, &replacement));
    }

    /// Enter: in the replace field, replaces the current match; elsewhere
    /// steps to the next (Shift: the previous).
    fn find_enter(&mut self, enter: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        let in_replacement = self
            .find
            .as_ref()
            .is_some_and(|f| f.replacement.read(cx).focus_handle(cx).is_focused(window));
        if in_replacement && !enter.shift {
            self.replace_current(cx);
        } else {
            self.step(!enter.shift, cx);
        }
    }

    /// Tab: between the query and the replace field.
    fn cycle_find_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_ref().filter(|f| f.replacing) else {
            return;
        };
        let query = find.query.read(cx).focus_handle(cx);
        let next = if query.is_focused(window) {
            find.replacement.read(cx).focus_handle(cx)
        } else {
            query
        };
        next.focus(window, cx);
    }

    /// Ctrl+H in the bar: shows or hides the replace field.
    fn toggle_replacing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        find.replacing = !find.replacing;
        let field = if find.replacing {
            &find.replacement
        } else {
            &find.query
        };
        field.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// The bar, when it's open.
    pub(crate) fn render_find(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let find = self.find.as_ref()?;
        let has_matches = !find.matches.is_empty();
        let theme = cx.theme();
        let bar = v_flex()
            .key_context(KEY_CONTEXT)
            .on_action(
                cx.listener(|this, enter: &Enter, window, cx| this.find_enter(enter, window, cx)),
            )
            .on_action(cx.listener(|this, _: &Escape, window, cx| this.close_find(window, cx)))
            .on_action(
                cx.listener(|this, _: &IndentInline, window, cx| this.cycle_find_focus(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OutdentInline, window, cx| {
                    this.cycle_find_focus(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Search, window, cx| this.open_find(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &Replace, window, cx| this.toggle_replacing(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ReplaceAll, _, cx| this.replace_all(cx)))
            .flex_none()
            .w_full()
            .gap_1()
            .px_3()
            .py_2()
            .bg(theme.tokens.popover)
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        Input::new(&find.query)
                            .focus_bordered(false)
                            .suffix(
                                Button::new("match-case")
                                    .selected(find.match_case)
                                    .toggled(find.match_case)
                                    .xsmall()
                                    .compact()
                                    .text()
                                    .icon(IconName::CaseSensitive)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(find) = this.find.as_mut() {
                                            find.match_case = !find.match_case;
                                        }
                                        this.search(cx);
                                    })),
                            )
                            .small()
                            .flex_1()
                            .shadow_none(),
                    )
                    .child(
                        Button::new("replace-mode")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Replace)
                            .selected(find.replacing)
                            .toggled(find.replacing)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.toggle_replacing(window, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new("prev-match")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronLeft)
                            .disabled(!has_matches)
                            .on_click(cx.listener(|this, _, _, cx| this.step(false, cx))),
                    )
                    .child(
                        Button::new("next-match")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronRight)
                            .disabled(!has_matches)
                            .on_click(cx.listener(|this, _, _, cx| this.step(true, cx))),
                    )
                    .child(
                        div()
                            .min_w_16()
                            .text_sm()
                            .when(!has_matches, |this| this.text_color(theme.muted_foreground))
                            .child(find.label()),
                    )
                    .child(
                        Button::new("close-find")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_find(window, cx)),
                            ),
                    ),
            )
            .when(find.replacing, |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(
                            Input::new(&find.replacement)
                                .focus_bordered(false)
                                .small()
                                .flex_1()
                                .shadow_none(),
                        )
                        .child(
                            Button::new("replace-one")
                                .small()
                                .label("Replace")
                                .disabled(!has_matches)
                                .on_click(cx.listener(|this, _, _, cx| this.replace_current(cx))),
                        )
                        .child(
                            Button::new("replace-all")
                                .small()
                                .label("Replace all")
                                .disabled(!has_matches)
                                .on_click(cx.listener(|this, _, _, cx| this.replace_all(cx))),
                        ),
                )
            });
        Some(bar.into_any_element())
    }
}
