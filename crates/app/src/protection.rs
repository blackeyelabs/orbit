//! Protected folders in the window, laid out as on the design canvas: the
//! three-step protect flow, the unlock prompt (which names no folder) and
//! the status bar's lock control. The work is the Vault facade's; the slow
//! parts (Argon2, encrypting a folder) run off the UI thread.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use orbit_core::{
    ProtectionKeys, Strength, UnlockError, Vault, VaultPath, Zeroizing, passphrase_strength,
};

use crate::window::OrbitWindow;

// Canvas sizes at 100% (480 px), in rems.
pub(crate) const DIALOG_WIDTH: f32 = 30.;

/// Opens the protect flow on `folder` (one of `folders`, which it can be
/// changed to).
pub fn ask_protect(
    view: WeakEntity<OrbitWindow>,
    vault: Arc<Vault>,
    folder: VaultPath,
    folders: Vec<VaultPath>,
    window: &mut Window,
    cx: &mut App,
) {
    let flow = cx.new(|cx| ProtectFlow::new(view, vault, folder, folders, window, cx));
    window.open_dialog(cx, move |dialog, window, _| {
        dialog
            .title("Protect a folder")
            .w(window.rem_size() * DIALOG_WIDTH)
            .child(flow.clone())
    });
}

/// Opens the unlock prompt.
pub fn ask_unlock(
    view: WeakEntity<OrbitWindow>,
    vault: Arc<Vault>,
    window: &mut Window,
    cx: &mut App,
) {
    let prompt = cx.new(|cx| UnlockPrompt::new(view, vault, window, cx));
    let focus = prompt.read(cx).input.clone();
    window.open_dialog(cx, move |dialog, window, _| {
        dialog
            .title("Unlock")
            .w(window.rem_size() * DIALOG_WIDTH)
            .child(prompt.clone())
    });
    focus.update(cx, |input, cx| input.focus(window, cx));
}

/// The status bar's lock control: "Locked", or "Unlocked · N" with a menu
/// for locking.
pub fn status_control(unlocked: &[VaultPath], cx: &mut Context<OrbitWindow>) -> AnyElement {
    if unlocked.is_empty() {
        return Button::new("protection")
            .ghost()
            .xsmall()
            .icon(IconName::Lock)
            .label("Locked")
            .tooltip("Unlock")
            .on_click(cx.listener(|this, _, window, cx| this.ask_unlock(window, cx)))
            .into_any_element();
    }
    let view = cx.entity().downgrade();
    let folders = unlocked.to_vec();
    let count = folders.len();
    Button::new("protection")
        .ghost()
        .xsmall()
        .icon(IconName::LockOpen)
        .label(format!("Unlocked · {count}"))
        .text_color(cx.theme().success)
        .dropdown_menu(move |menu, _, _| {
            let lock = |label: String, folder: Option<VaultPath>| {
                let view = view.clone();
                PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    let folder = folder.clone();
                    view.update(cx, |this, cx| match folder {
                        Some(folder) => this.lock_folder(&folder, window, cx),
                        None => this.lock_all_folders(window, cx),
                    })
                    .ok();
                })
            };
            let heading = match count {
                1 => "1 folder unlocked".to_string(),
                n => format!("{n} folders unlocked"),
            };
            let mut menu = menu.label(heading);
            menu = match folders.as_slice() {
                [only] => menu.item(lock("Lock now".into(), Some(only.clone()))),
                many => many.iter().fold(menu, |menu, folder| {
                    menu.item(lock(
                        format!("Lock {}", folder.name()),
                        Some(folder.clone()),
                    ))
                }),
            };
            let unlock_view = view.clone();
            menu.item(lock("Lock all    Ctrl+Shift+L".into(), None))
                .separator()
                .item(
                    PopupMenuItem::new("Unlock another…").on_click(move |_, window, cx| {
                        unlock_view
                            .update(cx, |this, cx| this.ask_unlock(window, cx))
                            .ok();
                    }),
                )
        })
        .into_any_element()
}

enum Step {
    Folder,
    Passphrase,
    /// Deriving the key from the passphrase.
    Preparing,
    Recovery {
        keys: Option<ProtectionKeys>,
        /// The recovery key as shown, kept once rather than made per frame.
        key: Zeroizing<String>,
        stored: bool,
    },
    /// Encrypting the folder.
    Protecting,
}

struct ProtectFlow {
    view: WeakEntity<OrbitWindow>,
    vault: Arc<Vault>,
    folder: VaultPath,
    folders: Vec<VaultPath>,
    passphrase: Entity<InputState>,
    confirm: Entity<InputState>,
    step: Step,
    error: Option<SharedString>,
    _changes: [Subscription; 2],
}

impl ProtectFlow {
    fn new(
        view: WeakEntity<OrbitWindow>,
        vault: Arc<Vault>,
        folder: VaultPath,
        folders: Vec<VaultPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut field =
            |cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).masked(true));
        let (passphrase, confirm) = (field(cx), field(cx));
        let changed = |cx: &mut Context<Self>, input: &Entity<InputState>| {
            cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify())
        };
        let _changes = [changed(cx, &passphrase), changed(cx, &confirm)];
        ProtectFlow {
            view,
            vault,
            folder,
            folders,
            passphrase,
            confirm,
            step: Step::Folder,
            error: None,
            _changes,
        }
    }

    fn passphrase(&self, cx: &App) -> SharedString {
        self.passphrase.read(cx).value()
    }

    /// From the passphrase to the recovery key: Argon2 runs meanwhile.
    fn prepare(&mut self, cx: &mut Context<Self>) {
        self.step = Step::Preparing;
        self.error = None;
        let (vault, passphrase) = (self.vault.clone(), self.passphrase(cx).to_string());
        let keys = cx.background_spawn(async move { vault.prepare_protection(&passphrase) });
        cx.spawn(async move |this, cx| {
            let keys = keys.await;
            this.update(cx, |this, cx| {
                match keys {
                    Ok(keys) => {
                        this.step = Step::Recovery {
                            key: keys.recovery_key(),
                            keys: Some(keys),
                            stored: false,
                        }
                    }
                    Err(e) => {
                        this.step = Step::Passphrase;
                        this.error = Some(format!("Couldn't make the keys: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Encrypts the folder, then closes, or says why it couldn't.
    fn protect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Step::Recovery { keys, .. } = &mut self.step else {
            return;
        };
        let Some(keys) = keys.take() else {
            return;
        };
        self.step = Step::Protecting;
        let (vault, folder) = (self.vault.clone(), self.folder.clone());
        // Its open notes are saved and closed: nothing is edited while it's
        // encrypted.
        self.view
            .update(cx, |this, cx| this.close_tabs_within(&folder, cx))
            .ok();
        let done = cx.background_spawn(async move { vault.protect_folder(&folder, keys) });
        let view = self.view.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = done.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    window.close_dialog(cx);
                    view.update(cx, |this, cx| this.protection_changed(cx)).ok();
                }
                Err(e) => {
                    // The keys are spent: back to the passphrase for new ones.
                    this.step = Step::Passphrase;
                    this.error =
                        Some(format!("Couldn't protect {}: {e}", this.folder.name()).into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_folder(&self, cx: &mut Context<Self>) -> AnyElement {
        let folders = self.folders.clone();
        let this = cx.entity().downgrade();
        v_flex()
            .gap_3()
            .child(step_label("1 · Choose the folder"))
            .child(
                h_flex()
                    .gap_2()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(IconName::FolderClosed)
                    .child(div().flex_1().child(self.folder.to_string()))
                    .when(folders.len() > 1, |row| {
                        row.child(
                            Button::new("change-folder")
                                .ghost()
                                .small()
                                .label("Change")
                                .dropdown_menu(move |menu, _, _| {
                                    folders.iter().fold(menu, |menu, folder| {
                                        let (this, chosen) = (this.clone(), folder.clone());
                                        menu.item(PopupMenuItem::new(folder.to_string()).on_click(
                                            move |_, _, cx| {
                                                let chosen = chosen.clone();
                                                this.update(cx, |this, cx| {
                                                    this.folder = chosen;
                                                    cx.notify();
                                                })
                                                .ok();
                                            },
                                        ))
                                    })
                                }),
                        )
                    }),
            )
            .child(text(
                "Orbit encrypts the contents, file names and folder structure with a passphrase. \
                 While locked, the folder doesn't appear in the tree, search or links.",
            ))
            .child(note(
                "Someone with access to your disk can still see that a protected folder exists, \
                 how many files it holds and their approximate sizes. Copies of its notes from \
                 before, in backups or deleted files, aren't touched.",
                cx,
            ))
            .child(buttons(
                (
                    "Cancel",
                    cx.listener(|_, _, window, cx| window.close_dialog(cx)),
                ),
                (
                    "Continue",
                    true,
                    cx.listener(|this, _, _, cx| {
                        this.step = Step::Passphrase;
                        cx.notify();
                    }),
                ),
            ))
            .into_any_element()
    }

    fn render_passphrase(&self, cx: &mut Context<Self>) -> AnyElement {
        let (fields, matches) = passphrase_fields(&self.passphrase, &self.confirm, cx);
        v_flex()
            .gap_3()
            .child(step_label("2 · Set a passphrase"))
            .child(fields)
            .when_some(self.error.clone(), |this, error| {
                this.child(error_text(error, cx))
            })
            .child(buttons(
                (
                    "Back",
                    cx.listener(|this, _, _, cx| {
                        this.step = Step::Folder;
                        cx.notify();
                    }),
                ),
                (
                    "Continue",
                    matches,
                    cx.listener(|this, _, _, cx| this.prepare(cx)),
                ),
            ))
            .into_any_element()
    }

    fn render_recovery(&self, key: &str, stored: bool, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_3()
            .child(step_label("3 · Save the recovery key"))
            .child(recovery_key(
                key,
                stored,
                cx.listener(|this, checked: &bool, _, cx| {
                    if let Step::Recovery { stored, .. } = &mut this.step {
                        *stored = *checked;
                    }
                    cx.notify();
                }),
                cx,
            ))
            .when_some(self.error.clone(), |this, error| {
                this.child(error_text(error, cx))
            })
            .child(buttons(
                (
                    "Back",
                    cx.listener(|this, _, _, cx| {
                        // These keys aren't used: new ones come from the
                        // passphrase again.
                        this.step = Step::Passphrase;
                        cx.notify();
                    }),
                ),
                (
                    "Protect folder",
                    stored,
                    cx.listener(|this, _, window, cx| this.protect(window, cx)),
                ),
            ))
            .into_any_element()
    }
}

/// A new passphrase, typed twice, with its strength and whether the two
/// match; and whether they do.
pub(crate) fn passphrase_fields(
    passphrase: &Entity<InputState>,
    confirm: &Entity<InputState>,
    cx: &App,
) -> (AnyElement, bool) {
    let (typed, again) = (passphrase.read(cx).value(), confirm.read(cx).value());
    let matches = !typed.is_empty() && typed == again;
    let (label, colour) = match passphrase_strength(&typed) {
        Strength::Weak => ("Weak", cx.theme().danger),
        Strength::Fair => ("Fair", cx.theme().warning),
        Strength::Strong => ("Strong", cx.theme().success),
    };
    let fields = v_flex()
        .gap_3()
        .child(field_label("Passphrase"))
        .child(Input::new(passphrase).mask_toggle())
        .when(!typed.is_empty(), |this| {
            this.child(div().text_xs().text_color(colour).child(label))
        })
        .child(field_label("Confirm passphrase"))
        .child(Input::new(confirm).mask_toggle())
        .when(!again.is_empty(), |this| {
            this.child(if matches {
                div()
                    .text_xs()
                    .text_color(cx.theme().success)
                    .child("Passphrases match")
            } else {
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child("Passphrases don't match")
            })
        })
        .child(text(
            "Four or more unrelated words make a stronger passphrase than a short password \
             with symbols.",
        ));
    (fields.into_any_element(), matches)
}

/// A recovery key shown once: copy, save to a file, and the "I have stored
/// it" checkbox, which `on_stored` hears.
pub(crate) fn recovery_key(
    key: &str,
    stored: bool,
    on_stored: impl Fn(&bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let key = Zeroizing::new(key.to_string());
    let (copied, saved) = (key.clone(), key.clone());
    v_flex()
        .gap_3()
        .child(text(
            "This key opens the folder if you forget the passphrase. It's shown only once.",
        ))
        .child(
            div()
                .p_3()
                .rounded_md()
                .bg(cx.theme().muted)
                .text_center()
                .font_family(cx.theme().mono_font_family.clone())
                .child(key.to_string()),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("copy-key")
                        .outline()
                        .small()
                        .icon(IconName::Copy)
                        .label("Copy")
                        .on_click(move |_, _, cx| {
                            crate::clipboard_guard::copy_secret(copied.clone(), cx)
                        }),
                )
                .child(
                    Button::new("save-key")
                        .outline()
                        .small()
                        .icon(IconName::Download)
                        .label("Save to file")
                        .on_click(move |_, window, cx| save_key(saved.clone(), window, cx)),
                ),
        )
        .child(note(
            "Without this key, a forgotten passphrase means permanent loss. Orbit can't \
             reset or recover it for you.",
            cx,
        ))
        .child(
            Checkbox::new("stored")
                .label("I have stored my recovery key")
                .checked(stored)
                .on_click(on_stored),
        )
}

/// Saves the recovery key to a file the person chooses; a failure is a
/// notification.
fn save_key(key: Zeroizing<String>, window: &mut Window, cx: &mut App) {
    let start = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let chosen = cx.prompt_for_new_path(&start, Some("Orbit recovery key.txt"));
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(file))) = chosen.await else {
                return;
            };
            let text = Zeroizing::new(format!(
                "Orbit recovery key\n\n{}\n\nIt unlocks a protected folder if its passphrase \
                 is forgotten. Keep it somewhere safe, away from the folder.\n",
                key.as_str()
            ));
            if let Err(e) = orbit_core::save_elsewhere(&file, &text) {
                cx.update(|window, cx| {
                    window.push_notification(
                        Notification::error(format!("Couldn't save the key: {e}")),
                        cx,
                    );
                })
                .ok();
            }
        })
        .detach();
}

impl Render for ProtectFlow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.step {
            Step::Folder => self.render_folder(cx),
            Step::Passphrase => self.render_passphrase(cx),
            Step::Preparing => busy("Making the keys…", cx),
            Step::Recovery {
                keys: Some(_),
                key,
                stored,
            } => self.render_recovery(key, *stored, cx),
            Step::Recovery { keys: None, .. } | Step::Protecting => {
                busy(&format!("Protecting {}…", self.folder.name()), cx)
            }
        }
    }
}

struct UnlockPrompt {
    view: WeakEntity<OrbitWindow>,
    vault: Arc<Vault>,
    input: Entity<InputState>,
    /// The recovery key is typed instead of the passphrase.
    recovery: bool,
    error: Option<SharedString>,
    /// No attempt before then, after a wrong one.
    wait_until: Option<Instant>,
    busy: bool,
    /// Redraws the countdown while there's a wait.
    _ticking: Option<Task<()>>,
    _enter: Subscription,
}

impl UnlockPrompt {
    fn new(
        view: WeakEntity<OrbitWindow>,
        vault: Arc<Vault>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).masked(true));
        let _enter = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(window, cx);
            }
        });
        UnlockPrompt {
            view,
            vault,
            input,
            recovery: false,
            error: None,
            wait_until: None,
            busy: false,
            _ticking: None,
            _enter,
        }
    }

    fn wait_left(&self) -> Option<Duration> {
        self.wait_until
            .and_then(|until| until.checked_duration_since(Instant::now()))
            .filter(|left| !left.is_zero())
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let secret = self.input.read(cx).value().to_string();
        if self.busy || secret.is_empty() || self.wait_left().is_some() {
            return;
        }
        self.busy = true;
        let (vault, recovery) = (self.vault.clone(), self.recovery);
        let tried = cx.background_spawn(async move {
            match recovery {
                true => vault.unlock_with_recovery_key(&secret),
                false => vault.unlock(&secret),
            }
        });
        let view = self.view.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = tried.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        window.close_dialog(cx);
                        view.update(cx, |this, cx| this.protection_changed(cx)).ok();
                        crate::protection_manage::ask_about_unprotected_copies(
                            view.clone(),
                            this.vault.clone(),
                            window,
                            cx,
                        );
                    }
                    Err(UnlockError::Wrong { retry_in }) => {
                        this.error = Some(
                            match this.recovery {
                                true => "That recovery key did not work.",
                                false => "That passphrase did not work.",
                            }
                            .into(),
                        );
                        this.wait_until = Some(Instant::now() + retry_in);
                        this.tick(cx);
                    }
                    // Not tried: only the wait is shown.
                    Err(UnlockError::TooSoon(retry_in)) => {
                        this.wait_until = Some(Instant::now() + retry_in);
                        this.tick(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Redraws twice a second until the wait is over.
    fn tick(&mut self, cx: &mut Context<Self>) {
        self._ticking = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let waiting = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.wait_left().is_some()
                    })
                    .unwrap_or(false);
                if !waiting {
                    break;
                }
            }
        }));
    }

    fn switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recovery = !self.recovery;
        self.error = None;
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }
}

impl Render for UnlockPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wait = self.wait_left();
        let (label, switch) = match self.recovery {
            false => ("Passphrase", "Use recovery key instead"),
            true => ("Recovery key", "Back to passphrase"),
        };
        v_flex()
            .gap_3()
            .child(field_label(label))
            .child(Input::new(&self.input).mask_toggle())
            .when_some(self.error.clone(), |this, error| {
                this.child(error_text(error, cx))
            })
            .when_some(wait, |this, wait| {
                this.child(text(format!(
                    "You can try again in {} seconds.",
                    wait.as_secs_f32().ceil() as u64
                )))
            })
            .when(self.recovery, |this| {
                this.child(text(
                    "Without the recovery key, a forgotten passphrase can't be recovered. Orbit \
                     doesn't keep a copy.",
                ))
            })
            .child(
                div()
                    .id("switch-secret")
                    .text_sm()
                    .text_color(cx.theme().link)
                    .cursor_pointer()
                    .child(switch)
                    .on_click(cx.listener(|this, _, window, cx| this.switch(window, cx))),
            )
            .child(buttons(
                (
                    "Cancel",
                    cx.listener(|_, _, window, cx| window.close_dialog(cx)),
                ),
                (
                    if self.busy { "Unlocking…" } else { "Unlock" },
                    !self.busy && wait.is_none(),
                    cx.listener(|this, _, window, cx| this.submit(window, cx)),
                ),
            ))
    }
}

/// Back (or Cancel) and a primary button, enabled or not.
pub(crate) fn buttons(
    (back, on_back): (
        &'static str,
        impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ),
    (go, enabled, on_go): (
        &'static str,
        bool,
        impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ),
) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_end()
        .gap_2()
        .pt_2()
        .child(Button::new("back").outline().label(back).on_click(on_back))
        .child(
            Button::new("go")
                .primary()
                .label(go)
                .disabled(!enabled)
                .on_click(on_go),
        )
}

pub(crate) fn step_label(label: &'static str) -> impl IntoElement {
    div().text_xs().font_weight(FontWeight::BOLD).child(label)
}

pub(crate) fn field_label(label: &'static str) -> impl IntoElement {
    div()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

pub(crate) fn text(text: impl Into<SharedString>) -> impl IntoElement {
    div().text_sm().child(text.into())
}

pub(crate) fn note(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .p_2()
        .border_l_2()
        .border_color(cx.theme().warning)
        .text_xs()
        .child(text)
}

pub(crate) fn error_text(error: SharedString, cx: &App) -> impl IntoElement {
    div().text_sm().text_color(cx.theme().danger).child(error)
}

pub(crate) fn busy(what: &str, cx: &App) -> AnyElement {
    div()
        .py_6()
        .text_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(what.to_string())
        .into_any_element()
}
