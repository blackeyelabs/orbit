//! What the window's centre shows when there's no note to show, as on the
//! design canvas: an empty vault offers New note, and with nothing open,
//! opening a note, creating one and running a command, each with the keys
//! bound to it.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Icon};
use gpui_kit::*;

use crate::commands::{CommandPalette, QuickSwitcher};
use crate::file_actions::NewNote;
use crate::window::KEY_CONTEXT;

/// The vault has no notes yet.
pub fn empty_vault(window: &Window, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    centred()
        .gap_2p5()
        .child(
            Icon::new(IconName::FilePlus)
                .size(px(34.))
                .text_color(muted),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .child("This vault has no notes yet"),
        )
        .child(
            div()
                .max_w(rems(22.5))
                .text_sm()
                .text_center()
                .text_color(muted)
                .child(
                    "Notes are plain markdown files. Start one, or drop existing files into the \
                     folder.",
                ),
        )
        .child(
            h_flex()
                .gap_2p5()
                .pt_1p5()
                .child(
                    Button::new("empty-new-note")
                        .primary()
                        .label("New note")
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(NewNote), cx)),
                )
                .children(shortcut(&NewNote, window)),
        )
        .into_any_element()
}

/// The vault has notes, but none is open.
pub fn nothing_open(window: &Window, cx: &App) -> AnyElement {
    let offer = |id: &'static str, label: &'static str, action: fn() -> Box<dyn Action>| {
        let keys = shortcut(&*action(), window);
        h_flex()
            .id(id)
            .w_full()
            .justify_between()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .hover(|this| this.bg(cx.theme().accent))
            .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
            .child(label)
            .children(keys)
    };
    centred()
        .gap_2p5()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .child("Nothing open"),
        )
        .child(
            v_flex()
                .w(rems(19.))
                .gap_1()
                .pt_1p5()
                .child(offer("offer-open", "Open a note", || {
                    Box::new(QuickSwitcher)
                }))
                .child(offer("offer-create", "Create a note", || Box::new(NewNote)))
                .child(offer("offer-command", "Run a command", || {
                    Box::new(CommandPalette)
                })),
        )
        .into_any_element()
}

/// A column centred in what's left of the window.
fn centred() -> Div {
    v_flex()
        .flex_1()
        .size_full()
        .p_6()
        .items_center()
        .justify_center()
}

/// The keys bound to `action` in the window, as key caps.
fn shortcut(action: &dyn Action, window: &Window) -> Option<Kbd> {
    Kbd::binding_for_action(action, Some(KEY_CONTEXT), window)
}
