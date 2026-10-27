//! Settings › Appearance (theme, paper grain and interface size) and
//! Settings › Security (when protected folders lock, what protection
//! covers, and managing the unlocked ones). Changes apply immediately and
//! are saved (orbit-core's Settings).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::*;
use orbit_core::{ThemeChoice, Vault, VaultPath};

use crate::appearance::AppSettings;
use crate::clipboard_guard;
use crate::protection_manage;
use crate::window::OrbitWindow;

pub struct AppearancePane;

/// Applies a settings change and redraws the pane it's made in.
fn change<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    edit: impl FnOnce(&mut orbit_core::Settings),
) {
    AppSettings::update(window, cx, edit);
    cx.notify();
}

/// A setting: its label and hint, and its control.
fn row(
    label: impl Into<SharedString>,
    hint: impl Into<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    let (label, hint) = (label.into(), hint.into());
    h_flex()
        .gap_4()
        .py_3()
        .justify_between()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            v_flex()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(label),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(hint),
                ),
        )
        .child(control)
}

impl Render for AppearancePane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = AppSettings::get(cx).clone();
        let theme_button = |id: &'static str, label: &'static str, choice: ThemeChoice| {
            let selected = settings.theme == choice;
            let button = Button::new(id).label(label).small();
            let button = if selected {
                button.primary()
            } else {
                button.outline()
            };
            button.on_click(cx.listener(move |_, _, window, cx| {
                change(window, cx, |s| s.theme = choice);
            }))
        };
        let size = settings.interface_size_percent();
        let (can_shrink, can_grow) = (
            settings.can_shrink_interface(),
            settings.can_grow_interface(),
        );
        v_flex()
            .child(row(
                "Theme",
                "Orbit Paper, Orbit Paper Dark, or follow the desktop.",
                h_flex()
                    .gap_2()
                    .child(theme_button(
                        "theme-paper",
                        "Orbit Paper",
                        ThemeChoice::OrbitPaper,
                    ))
                    .child(theme_button(
                        "theme-paper-dark",
                        "Orbit Paper Dark",
                        ThemeChoice::OrbitPaperDark,
                    ))
                    .child(theme_button(
                        "theme-system",
                        "Match system",
                        ThemeChoice::MatchSystem,
                    )),
                cx,
            ))
            .child(row(
                "Paper grain",
                "A soft halftone texture on every surface.",
                Switch::new("paper-grain")
                    .checked(settings.paper_grain)
                    .on_click(cx.listener(|_, checked: &bool, window, cx| {
                        let checked = *checked;
                        change(window, cx, |s| s.paper_grain = checked);
                    })),
                cx,
            ))
            .child(row(
                "Interface size",
                "Scales the whole interface.",
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("size-down")
                            .label("−")
                            .small()
                            .outline()
                            .disabled(!can_shrink)
                            .on_click(cx.listener(|_, _, window, cx| {
                                change(window, cx, |s| s.shrink_interface());
                            })),
                    )
                    .child(
                        div()
                            .w(rems(3.))
                            .text_center()
                            .text_sm()
                            .child(format!("{size}%")),
                    )
                    .child(
                        Button::new("size-up")
                            .label("+")
                            .small()
                            .outline()
                            .disabled(!can_grow)
                            .on_click(cx.listener(|_, _, window, cx| {
                                change(window, cx, |s| s.grow_interface());
                            })),
                    ),
                cx,
            ))
    }
}

/// Settings › Security, for one vault window: auto-lock, and the unlocked
/// protected folders, each with what can be done to it.
pub struct SecurityPane {
    view: WeakEntity<OrbitWindow>,
    vault: Arc<Vault>,
    /// Redraws when folders lock or unlock in the window.
    _window: Option<Subscription>,
}

impl SecurityPane {
    pub fn new(view: WeakEntity<OrbitWindow>, vault: Arc<Vault>, cx: &mut Context<Self>) -> Self {
        let _window = view
            .upgrade()
            .map(|window| cx.observe(&window, |_, _, cx| cx.notify()));
        SecurityPane {
            view,
            vault,
            _window,
        }
    }

    fn render_folders(&self, cx: &mut Context<Self>) -> AnyElement {
        let folders = self.vault.unlocked_folders();
        if folders.is_empty() {
            return div()
                .py_3()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(
                    "No protected folder is unlocked. Locked folders aren't listed: unlock one to \
                     change its passphrase or recovery key, or to remove its protection.",
                )
                .into_any_element();
        }
        v_flex()
            .children(folders.into_iter().enumerate().map(|(ix, folder)| {
                let action = |id: &'static str, label: &'static str, open: OpenDialog| {
                    let (view, vault, folder) =
                        (self.view.clone(), self.vault.clone(), folder.clone());
                    Button::new((id, ix))
                        .label(label)
                        .small()
                        .outline()
                        .on_click(move |_, window, cx| {
                            open(view.clone(), vault.clone(), folder.clone(), window, cx)
                        })
                };
                v_flex()
                    .gap_2()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(IconName::LockOpen)
                            .child(folder.to_string()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(action(
                                "passphrase",
                                "Change passphrase",
                                protection_manage::ask_change_passphrase,
                            ))
                            .child(action(
                                "recovery",
                                "New recovery key",
                                protection_manage::ask_new_recovery_key,
                            ))
                            .child(action(
                                "remove",
                                "Remove protection",
                                protection_manage::ask_remove_protection,
                            )),
                    )
            }))
            .into_any_element()
    }
}

/// How long text copied from a protected note stays in the clipboard, in
/// seconds.
const CLEAR_CLIPBOARD_CHOICES: [u16; 4] = [10, 30, 60, 120];

/// Opens one of the dialogs that manage an unlocked protected folder.
type OpenDialog = fn(WeakEntity<OrbitWindow>, Arc<Vault>, VaultPath, &mut Window, &mut App);

/// The idle times offered before protected folders lock; `None` is never.
const AUTO_LOCK_CHOICES: [(Option<u16>, &str); 5] = [
    (Some(5), "5 min"),
    (Some(10), "10 min"),
    (Some(30), "30 min"),
    (Some(60), "1 hour"),
    (None, "Never"),
];

impl Render for SecurityPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = AppSettings::get(cx).clone();
        let idle_choices = AUTO_LOCK_CHOICES.iter().map(|&(minutes, label)| {
            let button = Button::new(label).label(label).small();
            let button = match settings.auto_lock_minutes == minutes {
                true => button.primary(),
                false => button.outline(),
            };
            button.on_click(cx.listener(move |_, _, window, cx| {
                change(window, cx, |s| s.auto_lock_minutes = minutes);
            }))
        });
        let clear_choices = CLEAR_CLIPBOARD_CHOICES.iter().map(|&seconds| {
            let label = clipboard_guard::after(seconds);
            let button = Button::new(("clear-clipboard", usize::from(seconds)))
                .label(label)
                .small();
            let button = match settings.clear_clipboard_seconds == seconds {
                true => button.primary(),
                false => button.outline(),
            };
            button.on_click(cx.listener(move |_, _, window, cx| {
                change(window, cx, |s| s.clear_clipboard_seconds = seconds);
            }))
        });
        v_flex()
            .child(
                div()
                    .pb_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "Protected folders encrypt their notes, attachments, file names and \
                         folder structure on disk. They don't hide that a protected folder \
                         exists, how many files it holds or roughly how large they are, and \
                         don't guard a computer that's unlocked and in use. Copies made before \
                         protecting, in backups, sync or the system trash, aren't touched.",
                    ),
            )
            .child(row(
                "Lock when idle",
                "Protected folders lock after this long without input.",
                h_flex().gap_1().children(idle_choices),
                cx,
            ))
            .child(row(
                "Lock on suspend",
                "When the computer sleeps.",
                Switch::new("lock-on-suspend")
                    .checked(settings.lock_on_suspend)
                    .on_click(cx.listener(|_, checked: &bool, window, cx| {
                        let checked = *checked;
                        change(window, cx, |s| s.lock_on_suspend = checked);
                    })),
                cx,
            ))
            .child(row(
                "Lock when the screen locks",
                "When the desktop's screen locks.",
                Switch::new("lock-on-screen-lock")
                    .checked(settings.lock_on_screen_lock)
                    .on_click(cx.listener(|_, checked: &bool, window, cx| {
                        let checked = *checked;
                        change(window, cx, |s| s.lock_on_screen_lock = checked);
                    })),
                cx,
            ))
            .child(row(
                "Ask before copying",
                "Copying from a protected note asks first.",
                Switch::new("ask-before-protected-copy")
                    .checked(settings.ask_before_protected_copy)
                    .on_click(cx.listener(|_, checked: &bool, window, cx| {
                        let checked = *checked;
                        change(window, cx, |s| s.ask_before_protected_copy = checked);
                    })),
                cx,
            ))
            .child(row(
                "Clear the clipboard after",
                "Text copied from a protected note is cleared from the clipboard.",
                h_flex().gap_1().children(clear_choices),
                cx,
            ))
            .child(
                div()
                    .pt_4()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Unlocked protected folders"),
            )
            .child(self.render_folders(cx))
    }
}
