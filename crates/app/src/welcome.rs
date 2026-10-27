//! The Welcome window, from the design canvas: create a vault, open a folder
//! of notes, or pick a recent vault. A recent vault that moved shows "Can't
//! find …" with Locate folder and Open another vault.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, TitleBar};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use orbit_core::RecentVault;

use crate::appearance::{self, AppSettings};
use crate::vaults;

/// The width of the Welcome window's content, as on the design canvas.
const CONTENT_WIDTH: f32 = 28.75;

/// The name a new vault's folder is offered.
const NEW_VAULT_NAME: &str = "Notes";

pub struct Welcome {
    /// A vault that couldn't be found where it was: "Can't find …".
    missing: Option<RecentVault>,
    error: Option<SharedString>,
}

impl Welcome {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_window_appearance(window, |_, window, cx| appearance::apply(window, cx))
            .detach();
        Welcome {
            missing: None,
            error: None,
        }
    }

    /// Shows "Can't find …" for a vault that isn't at `root` any more.
    fn show_missing(&mut self, root: &Path) {
        self.missing = Some(RecentVault::at(root));
        self.error = None;
    }

    fn show_error(&mut self, error: impl Into<SharedString>) {
        self.error = Some(error.into());
    }

    /// Why the vault at `root` didn't open: "Can't find …" if it isn't
    /// there, else the error.
    pub fn show_open_error(&mut self, root: &Path, error: orbit_core::Error) {
        match error {
            orbit_core::Error::NotFound => self.show_missing(root),
            e => self.show_error(format!("Couldn't open {}: {e}", root.display())),
        }
    }

    /// Opens a vault and, once it's open, closes this window.
    /// Whether it opened.
    fn open(&mut self, root: &Path, cx: &mut Context<Self>) -> bool {
        let result = vaults::open_vault(root, cx);
        let opened = result.is_ok();
        match result {
            Ok(()) => cx.defer(vaults::close_welcome),
            Err(e) => self.show_open_error(root, e),
        }
        cx.notify();
        opened
    }

    /// Create a new vault: a new folder, chosen in the system dialog.
    fn create_vault(&mut self, cx: &mut Context<Self>) {
        let home = std::env::home_dir().unwrap_or_default();
        let chosen = cx.prompt_for_new_path(&home, Some(NEW_VAULT_NAME));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(folder))) = chosen.await else {
                return;
            };
            this.update(cx, |this, cx| match std::fs::create_dir_all(&folder) {
                Ok(()) => {
                    this.open(&folder, cx);
                }
                Err(e) => {
                    this.show_error(format!("Couldn't create {}: {e}", folder.display()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks for a folder in the system dialog, then does `then` with it.
    fn choose_folder(
        &mut self,
        prompt: &'static str,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, PathBuf, &mut Context<Self>) + 'static,
    ) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(prompt.into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut folders))) = chosen.await else {
                return;
            };
            let Some(folder) = folders.pop() else { return };
            this.update(cx, |this, cx| then(this, folder, cx)).ok();
        })
        .detach();
    }

    fn open_folder(&mut self, cx: &mut Context<Self>) {
        self.choose_folder("Open", cx, |this, folder, cx| {
            this.open(&folder, cx);
        });
    }

    /// Locate folder: the missing vault, found somewhere else.
    fn locate(&mut self, old_root: PathBuf, cx: &mut Context<Self>) {
        self.choose_folder("Open", cx, move |this, folder, cx| {
            // The list follows the vault only once it's really there.
            if this.open(&folder, cx) {
                vaults::relocate_recent(&old_root, &folder, cx);
            }
        });
    }

    fn render_home(&self, cx: &mut Context<Self>) -> AnyElement {
        let recent: Vec<_> = vaults::recent_vaults(cx)
            .into_iter()
            .enumerate()
            .map(|(ix, vault)| self.render_recent(ix, vault, cx))
            .collect();
        v_flex()
            .w(rems(CONTENT_WIDTH))
            .items_center()
            .gap_3p5()
            .text_center()
            .child(
                div()
                    .mt_1p5()
                    .text_3xl()
                    .font_weight(FontWeight::BOLD)
                    .child("Welcome to Orbit"),
            )
            .child(
                div().text_color(cx.theme().muted_foreground).child(
                    "Write notes and draw diagrams in plain files that stay on your computer.",
                ),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap_2p5()
                    .pt_3p5()
                    .child(
                        Button::new("create-vault")
                            .primary()
                            .large()
                            .icon(IconName::FolderPlus)
                            .label("Create a new vault")
                            .on_click(cx.listener(|this, _, _, cx| this.create_vault(cx))),
                    )
                    .child(
                        Button::new("open-folder")
                            .outline()
                            .large()
                            .icon(IconName::FolderOpen)
                            .label("Open a folder of notes")
                            .on_click(cx.listener(|this, _, _, cx| this.open_folder(cx))),
                    ),
            )
            .child(
                div()
                    .pt_1p5()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("A vault is just a folder. Orbit never moves or uploads your files."),
            )
            .when(!recent.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .mt_4()
                        .pt_4()
                        .gap_1()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .text_left()
                        .child(
                            div()
                                .pb_1()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(cx.theme().muted_foreground)
                                .child("RECENT"),
                        )
                        .children(recent),
                )
            })
            .into_any_element()
    }

    fn render_recent(&self, ix: usize, vault: RecentVault, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().accent;
        let path = home_relative(&vault.path);
        let found = vault.found;
        h_flex()
            .id(("recent", ix))
            .h(rems(2.5))
            .px_2p5()
            .gap_2p5()
            .rounded_lg()
            .cursor_pointer()
            .hover(move |this| this.bg(hover))
            .child(div().text_color(muted).child(IconName::Folder))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(vault.name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(muted)
                    .child(if found {
                        path
                    } else {
                        format!("Can't find · {path}")
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open(&vault.path, cx);
            }))
            .into_any_element()
    }

    fn render_missing(&self, vault: &RecentVault, cx: &mut Context<Self>) -> AnyElement {
        let old_root = vault.path.clone();
        v_flex()
            .w(rems(CONTENT_WIDTH))
            .items_center()
            .gap_3()
            .text_center()
            .child(
                div()
                    .text_color(cx.theme().danger)
                    .text_3xl()
                    .child(IconName::Folder),
            )
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child(format!("Can't find “{}”", vault.name)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "The folder was moved, renamed or sits on a drive that isn't \
                         connected. Nothing was changed. It was at {}.",
                        home_relative(&vault.path)
                    )),
            )
            .child(
                h_flex()
                    .gap_2p5()
                    .pt_1p5()
                    .child(
                        Button::new("locate")
                            .primary()
                            .label("Locate folder")
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.locate(old_root.clone(), cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new("open-another")
                            .outline()
                            .label("Open another vault")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.missing = None;
                                this.error = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for Welcome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let grain = AppSettings::get(cx).paper_grain;
        let body = match self.missing.clone() {
            Some(vault) => self.render_missing(&vault, cx),
            None => self.render_home(cx),
        };
        v_flex()
            .relative()
            .size_full()
            .child(crate::auto_lock::watch_pointer())
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                TitleBar::new().child(
                    div()
                        .flex_1()
                        .text_center()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Orbit"),
                ),
            )
            .child(
                v_flex()
                    .id("welcome")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .p_4()
                    .child(body)
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            div()
                                .max_w(rems(CONTENT_WIDTH))
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(error),
                        )
                    }),
            )
            .when(grain, |this| this.child(appearance::grain(window, cx)))
    }
}

/// `~/Documents/Notes` rather than `/home/me/Documents/Notes`.
fn home_relative(path: &Path) -> String {
    match std::env::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf))
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}
