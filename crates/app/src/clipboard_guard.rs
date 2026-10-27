//! Copying out of a protected note is deliberate and short-lived: Copy and
//! Cut there ask first (unless Settings say not to), and what's copied
//! leaves the clipboard after Settings' `clear_clipboard_seconds`, unless
//! something else was copied meanwhile. Plain notes copy as ever. The
//! recovery key, copied when it's shown, leaves the clipboard the same way.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui_kit::base::input::{Copy as CopyText, Cut};
use gpui_kit::base::v_flex;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::*;
use orbit_core::Zeroizing;

use crate::appearance::AppSettings;
use crate::protection::{DIALOG_WIDTH, buttons, text};

/// What's being taken out of a protected note.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Copying {
    /// The selection is copied.
    Copy,
    /// The selection is copied and deleted.
    Cut,
}

impl Copying {
    /// The editor action that does it.
    pub fn action(self) -> Box<dyn Action> {
        match self {
            Copying::Copy => Box::new(CopyText),
            Copying::Cut => Box::new(Cut),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Copying::Copy => "Copy",
            Copying::Cut => "Cut",
        }
    }
}

/// Asks before copying out of a protected note, unless Settings say not
/// to; then `copy` copies (or cuts) and says what it put in the clipboard,
/// which is cleared from it later. Never run within `copy`'s caller: it
/// runs from the dialog, or just after.
pub fn ask(
    copying: Copying,
    copy: impl Fn(&mut Window, &mut App) -> Option<Zeroizing<String>> + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let go = Rc::new(move |window: &mut Window, cx: &mut App| {
        if let Some(copied) = copy(window, cx) {
            clear_later(copied, cx);
        }
    });
    if !AppSettings::get(cx).ask_before_protected_copy {
        return window.defer(cx, move |window, cx| go(window, cx));
    }
    let seconds = AppSettings::get(cx).clear_clipboard_seconds;
    let dont_ask = Rc::new(Cell::new(false));
    window.open_dialog(cx, move |dialog, window, _| {
        let (go, dont_ask, checked) = (go.clone(), dont_ask.clone(), dont_ask.clone());
        dialog
            .title("Copy from a protected note?")
            .w(window.rem_size() * DIALOG_WIDTH)
            .child(
                v_flex()
                    .gap_3()
                    .child(text(format!(
                        "Other apps can read the clipboard. Orbit clears it {} after copying.",
                        after(seconds)
                    )))
                    .child(
                        Checkbox::new("dont-ask")
                            .label("Don't ask again")
                            .checked(checked.get())
                            .on_click(move |on: &bool, window, _| {
                                checked.set(*on);
                                window.refresh();
                            }),
                    )
                    .child(buttons(
                        ("Cancel", |_, window, cx| window.close_dialog(cx)),
                        (copying.label(), true, move |_, window, cx| {
                            if dont_ask.get() {
                                AppSettings::update(window, cx, |s| {
                                    s.ask_before_protected_copy = false
                                });
                            }
                            window.close_dialog(cx);
                            go(window, cx);
                        }),
                    )),
            )
    });
}

/// Copies a secret, such as a recovery key, to be cleared from the
/// clipboard as protected text is.
pub fn copy_secret(secret: Zeroizing<String>, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(secret.to_string()));
    clear_later(secret, cx);
}

/// What the clipboard holds as text, if anything.
pub fn clipboard_text(cx: &App) -> Option<Zeroizing<String>> {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .map(Zeroizing::new)
}

/// "30 seconds", "2 minutes": how long until the clipboard is cleared.
pub fn after(seconds: u16) -> String {
    match seconds {
        1 => "1 second".into(),
        60 => "1 minute".into(),
        s if s >= 120 && s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{s} seconds"),
    }
}

/// Whether the clipboard, holding `held`, still holds what was `copied`
/// from a protected note: only then is it cleared, so nothing copied since
/// is lost. Nothing copied needs no clearing.
fn still_holds(held: Option<&str>, copied: &str) -> bool {
    !copied.is_empty() && held == Some(copied)
}

/// Clears the clipboard after Settings' wait if it still holds `copied`.
fn clear_later(copied: Zeroizing<String>, cx: &mut App) {
    let wait = Duration::from_secs(u64::from(AppSettings::get(cx).clear_clipboard_seconds));
    cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        cx.update(|cx| {
            let held = clipboard_text(cx);
            if still_holds(held.as_deref().map(String::as_str), &copied) {
                cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::{after, still_holds};

    #[test]
    fn only_what_was_copied_from_a_protected_note_is_cleared() {
        assert!(still_holds(
            Some("the quiet albatross"),
            "the quiet albatross"
        ));
        assert!(!still_holds(Some("copied since"), "the quiet albatross"));
        assert!(!still_holds(None, "the quiet albatross"));
        assert!(!still_holds(Some(""), ""), "nothing was copied");
    }

    #[test]
    fn waits_are_said_in_words() {
        assert_eq!(after(10), "10 seconds");
        assert_eq!(after(30), "30 seconds");
        assert_eq!(after(60), "1 minute");
        assert_eq!(after(90), "90 seconds");
        assert_eq!(after(120), "2 minutes");
    }
}
