//! Managing a protected folder over time: a new passphrase or recovery key
//! and removing protection (each from Settings › Security, on an unlocked
//! folder), and what protection asks along the way: before a note moves
//! out, after the first moves in, and about a plain folder found at a
//! locked one's place. The work is the Vault facade's.

use std::{rc::Rc, sync::Arc};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use orbit_core::{UnprotectedCopy, Vault, VaultPath, Zeroizing};

use crate::appearance::AppSettings;
use crate::protection::{
    DIALOG_WIDTH, busy, buttons, error_text, field_label, note, passphrase_fields, recovery_key,
    text,
};
use crate::window::OrbitWindow;

/// Opens a dialog titled `title` around `view`.
fn open(title: String, view: AnyView, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, window, _| {
        dialog
            .title(title.clone())
            .w(window.rem_size() * DIALOG_WIDTH)
            .child(view.clone())
    });
}

/// Asks for a new passphrase for the unlocked protected `folder`.
pub fn ask_change_passphrase(
    _: WeakEntity<OrbitWindow>,
    vault: Arc<Vault>,
    folder: VaultPath,
    window: &mut Window,
    cx: &mut App,
) {
    let title = format!("New passphrase for {}", folder.name());
    let view = cx.new(|cx| ChangePassphrase::new(vault, folder, window, cx));
    open(title, view.into(), window, cx);
}

/// Gives the unlocked protected `folder` a new recovery key, once asked.
pub fn ask_new_recovery_key(
    _: WeakEntity<OrbitWindow>,
    vault: Arc<Vault>,
    folder: VaultPath,
    window: &mut Window,
    cx: &mut App,
) {
    let title = format!("New recovery key for {}", folder.name());
    let view = cx.new(|_| NewRecoveryKey {
        vault,
        folder,
        key: None,
        stored: false,
        error: None,
    });
    open(title, view.into(), window, cx);
}

/// Removes protection from the unlocked `folder`, once its name is typed.
pub fn ask_remove_protection(
    view: WeakEntity<OrbitWindow>,
    vault: Arc<Vault>,
    folder: VaultPath,
    window: &mut Window,
    cx: &mut App,
) {
    let title = format!("Remove protection from {}?", folder.name());
    let flow = cx.new(|cx| RemoveProtection::new(view, vault, folder, window, cx));
    open(title, flow.into(), window, cx);
}

/// Before `path` moves out of its protected folder into the plain
/// `folder`: it would be written in plain text. `then` moves it.
pub fn confirm_move_out(
    path: VaultPath,
    folder: VaultPath,
    then: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let then = Rc::new(then);
    let destination = match folder.is_root() {
        true => "the vault root".to_string(),
        false => folder.name().to_string(),
    };
    window.open_dialog(cx, move |dialog, window, _| {
        let then = then.clone();
        dialog
            .title(format!("Move {} out of protection?", path.name()))
            .w(window.rem_size() * DIALOG_WIDTH)
            .child(
                v_flex()
                    .gap_3()
                    .child(text(format!(
                        "In {destination} it's no longer encrypted: anyone with access to the \
                         disk can read it."
                    )))
                    .child(buttons(
                        ("Cancel", |_, window, cx| window.close_dialog(cx)),
                        ("Move out", true, move |_, window, cx| {
                            window.close_dialog(cx);
                            then(window, cx);
                        }),
                    ))
                    .into_any_element(),
            )
    });
}

/// After a note or folder first moves into a protected folder: its old
/// plain text may still be recoverable. Said once.
pub fn warn_after_moving_in(window: &mut Window, cx: &mut App) {
    if AppSettings::get(cx).warned_moving_in {
        return;
    }
    AppSettings::update(window, cx, |s| s.warned_moving_in = true);
    window.open_dialog(cx, |dialog, window, cx| {
        dialog
            .title("Moved into a protected folder")
            .w(window.rem_size() * DIALOG_WIDTH)
            .child(
                v_flex()
                    .gap_3()
                    .child(text(
                        "It's encrypted now, and the plain copy is deleted. Orbit says this once.",
                    ))
                    .child(note(
                        "Its old plain text may still be recoverable: from the disk until it's \
                         overwritten, and from backups, sync or the system trash.",
                        cx,
                    ))
                    .child(
                        h_flex().w_full().justify_end().pt_2().child(
                            Button::new("got-it")
                                .primary()
                                .label("Got it")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                    ),
            )
    });
}

/// Asks about each plain folder found at a protected folder's place as it
/// unlocked, one after another: move its contents in, or keep it.
pub fn ask_about_unprotected_copies(
    view: WeakEntity<OrbitWindow>,
    vault: Arc<Vault>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(UnprotectedCopy { copy, folder }) = vault.unprotected_copies().into_iter().next()
    else {
        return;
    };
    window.open_dialog(cx, move |dialog, window, cx| {
        let answer = |merge: bool| {
            let (view, vault, copy, folder) =
                (view.clone(), vault.clone(), copy.clone(), folder.clone());
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                window.close_dialog(cx);
                if merge {
                    match vault.merge_unprotected_copy(&copy) {
                        Ok(left) if left.is_empty() => warn_after_moving_in(window, cx),
                        Ok(left) => window.push_notification(
                            Notification::warning(format!(
                                "{} couldn't move into {} (a name taken there, say). They stay \
                                 in {}.",
                                names(&left),
                                folder.name(),
                                copy.name()
                            )),
                            cx,
                        ),
                        Err(e) => window.push_notification(
                            Notification::error(format!("Couldn't move {} in: {e}", copy.name())),
                            cx,
                        ),
                    }
                } else {
                    vault.keep_unprotected_copy(&copy);
                }
                view.update(cx, |this, cx| this.protection_changed(cx)).ok();
                ask_about_unprotected_copies(view.clone(), vault.clone(), window, cx);
            }
        };
        dialog
            .title(format!("An unprotected copy of {}", folder.name()))
            .w(window.rem_size() * DIALOG_WIDTH)
            .child(
                v_flex()
                    .gap_3()
                    .child(text(format!(
                        "While {} was locked, a plain folder was made in its place. It's now {}, \
                         and isn't encrypted.",
                        folder.name(),
                        copy.name()
                    )))
                    .child(note(
                        "Moving its contents in encrypts them and deletes the plain copies.",
                        cx,
                    ))
                    .child(buttons(
                        ("Keep separate", answer(false)),
                        ("Move contents in", true, answer(true)),
                    )),
            )
    });
}

/// The names of `paths`, listed.
fn names(paths: &[VaultPath]) -> String {
    let names: Vec<&str> = paths.iter().map(VaultPath::name).collect();
    names.join(", ")
}

/// The new-passphrase dialog: the passphrase twice, then Argon2 off the UI
/// thread.
struct ChangePassphrase {
    vault: Arc<Vault>,
    folder: VaultPath,
    passphrase: Entity<InputState>,
    confirm: Entity<InputState>,
    busy: bool,
    error: Option<SharedString>,
    _changes: [Subscription; 2],
}

impl ChangePassphrase {
    fn new(
        vault: Arc<Vault>,
        folder: VaultPath,
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
        ChangePassphrase {
            vault,
            folder,
            passphrase,
            confirm,
            busy: false,
            error: None,
            _changes,
        }
    }

    /// Re-wraps the folder's key off the UI thread (Argon2), then closes.
    fn change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true;
        self.error = None;
        let (vault, folder) = (self.vault.clone(), self.folder.clone());
        let passphrase = Zeroizing::new(self.passphrase.read(cx).value().to_string());
        let done =
            cx.background_spawn(async move { vault.change_passphrase(&folder, &passphrase) });
        cx.spawn_in(window, async move |this, cx| {
            let result = done.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        window.close_dialog(cx);
                        window.push_notification(
                            Notification::success(format!(
                                "{} has a new passphrase.",
                                this.folder.name()
                            )),
                            cx,
                        );
                    }
                    Err(e) => {
                        this.error = Some(format!("Couldn't change it: {e}").into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for ChangePassphrase {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.busy {
            return busy("Changing the passphrase…", cx);
        }
        let (fields, matches) = passphrase_fields(&self.passphrase, &self.confirm, cx);
        v_flex()
            .gap_3()
            .child(fields)
            .child(text(
                "The old passphrase stops working. The recovery key keeps working, and no note \
                 is re-encrypted.",
            ))
            .when_some(self.error.clone(), |this, error| {
                this.child(error_text(error, cx))
            })
            .child(buttons(
                (
                    "Cancel",
                    cx.listener(|_, _, window, cx| window.close_dialog(cx)),
                ),
                (
                    "Change passphrase",
                    matches,
                    cx.listener(|this, _, window, cx| this.change(window, cx)),
                ),
            ))
            .into_any_element()
    }
}

/// The new-recovery-key dialog: asked first, then the key shown once.
struct NewRecoveryKey {
    vault: Arc<Vault>,
    folder: VaultPath,
    /// The new key, once made: shown until the dialog closes.
    key: Option<Zeroizing<String>>,
    stored: bool,
    error: Option<SharedString>,
}

impl Render for NewRecoveryKey {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(key) = &self.key else {
            return v_flex()
                .gap_3()
                .child(text(
                    "The current recovery key stops working, and the new one is shown once. \
                     The passphrase keeps working.",
                ))
                .when_some(self.error.clone(), |this, error| {
                    this.child(error_text(error, cx))
                })
                .child(buttons(
                    (
                        "Cancel",
                        cx.listener(|_, _, window, cx| window.close_dialog(cx)),
                    ),
                    (
                        "Create new key",
                        true,
                        cx.listener(|this, _, _, cx| {
                            match this.vault.new_recovery_key(&this.folder) {
                                Ok(key) => this.key = Some(key),
                                Err(e) => {
                                    this.error = Some(format!("Couldn't create one: {e}").into());
                                }
                            }
                            cx.notify();
                        }),
                    ),
                ))
                .into_any_element();
        };
        v_flex()
            .gap_3()
            .child(recovery_key(
                key,
                self.stored,
                cx.listener(|this, checked: &bool, _, cx| {
                    this.stored = *checked;
                    cx.notify();
                }),
                cx,
            ))
            .child(buttons(
                (
                    "Close",
                    cx.listener(|_, _, window, cx| window.close_dialog(cx)),
                ),
                (
                    "Done",
                    self.stored,
                    cx.listener(|_, _, window, cx| window.close_dialog(cx)),
                ),
            ))
            .into_any_element()
    }
}

/// The remove-protection dialog, confirmed by typing the folder's name.
struct RemoveProtection {
    view: WeakEntity<OrbitWindow>,
    vault: Arc<Vault>,
    folder: VaultPath,
    typed: Entity<InputState>,
    busy: bool,
    error: Option<SharedString>,
    _changes: Subscription,
}

impl RemoveProtection {
    fn new(
        view: WeakEntity<OrbitWindow>,
        vault: Arc<Vault>,
        folder: VaultPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let typed = cx.new(|cx| InputState::new(window, cx).placeholder(folder.name().to_string()));
        let _changes = cx.subscribe(&typed, |_, _, _: &InputEvent, cx| cx.notify());
        RemoveProtection {
            view,
            vault,
            folder,
            typed,
            busy: false,
            error: None,
            _changes,
        }
    }

    /// Writes the folder out in plain text off the UI thread, then closes.
    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true;
        self.error = None;
        let (vault, folder) = (self.vault.clone(), self.folder.clone());
        let done = cx.background_spawn(async move { vault.remove_protection(&folder) });
        let view = self.view.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = done.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        window.close_dialog(cx);
                        view.update(cx, |this, cx| this.protection_changed(cx)).ok();
                        window.push_notification(
                            Notification::success(format!(
                                "{} isn't protected any more.",
                                this.folder.name()
                            )),
                            cx,
                        );
                    }
                    Err(orbit_core::Error::AlreadyExists(_)) => {
                        this.error = Some(
                            format!(
                                "Something else is named {} beside it: rename that first.",
                                this.folder.name()
                            )
                            .into(),
                        );
                        cx.notify();
                    }
                    Err(e) => {
                        this.error = Some(format!("Couldn't remove protection: {e}").into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for RemoveProtection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.busy {
            return busy(
                &format!("Removing protection from {}…", self.folder.name()),
                cx,
            );
        }
        let confirmed = self.typed.read(cx).value() == self.folder.name();
        v_flex()
            .gap_3()
            .child(text(
                "Everything in it is written to disk in plain text, and its encrypted copy is \
                 deleted. You can protect it again later.",
            ))
            .child(field_label("Type the folder's name to confirm"))
            .child(Input::new(&self.typed))
            .when_some(self.error.clone(), |this, error| {
                this.child(error_text(error, cx))
            })
            .child(buttons(
                (
                    "Cancel",
                    cx.listener(|_, _, window, cx| window.close_dialog(cx)),
                ),
                (
                    "Remove protection",
                    confirmed,
                    cx.listener(|this, _, window, cx| this.remove(window, cx)),
                ),
            ))
            .into_any_element()
    }
}
