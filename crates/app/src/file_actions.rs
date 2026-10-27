//! Dialogs and the tree's context menu for managing the vault: new note and
//! folder, rename, move and delete. The work itself is the Vault facade's;
//! these only ask and forward to [`OrbitWindow`].

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use orbit_core::{EntryKind, VaultPath};

use crate::window::OrbitWindow;

actions!(
    file_actions,
    [
        /// A new note at the vault root, opened in a tab.
        NewNote
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "ctrl-n",
        NewNote,
        Some(crate::window::KEY_CONTEXT),
    )]);
}

/// The tree's right-click menu for one entry.
pub fn tree_menu(
    menu: PopupMenu,
    view: WeakEntity<OrbitWindow>,
    path: VaultPath,
    kind: EntryKind,
    folders: Vec<VaultPath>,
) -> PopupMenu {
    let item = |label, icon, act| menu_item(&view, &path, label, icon, act);
    let menu = if kind == EntryKind::Folder {
        menu.item(item(
            "New note",
            IconName::FilePlus,
            Box::new(OrbitWindow::new_note),
        ))
        .item(item(
            "New folder",
            IconName::FolderPlus,
            Box::new(OrbitWindow::new_folder),
        ))
        .separator()
    } else {
        menu
    };
    let (move_view, move_path) = (view.clone(), path.clone());
    menu.item(item(
        "Rename…",
        IconName::Pencil,
        Box::new(move |this, path, window, cx| ask_rename(this, path, kind, window, cx)),
    ))
    .item(
        PopupMenuItem::new("Move to…")
            .icon(IconName::FolderInput)
            .on_click(move |_, window, cx| {
                ask_move(
                    move_view.clone(),
                    move_path.clone(),
                    folders.clone(),
                    window,
                    cx,
                )
            }),
    )
    .separator()
    .item(item("Delete", IconName::Trash, Box::new(ask_delete)))
}

/// What a menu item does to the window, given the entry's path.
type EntryAction = Box<dyn Fn(&mut OrbitWindow, VaultPath, &mut Window, &mut Context<OrbitWindow>)>;

fn menu_item(
    view: &WeakEntity<OrbitWindow>,
    path: &VaultPath,
    label: &'static str,
    icon: IconName,
    act: EntryAction,
) -> PopupMenuItem {
    let (view, path) = (view.clone(), path.clone());
    PopupMenuItem::new(label)
        .icon(icon)
        .on_click(move |_, window, cx| {
            let path = path.clone();
            view.update(cx, |this, cx| act(this, path, window, cx)).ok();
        })
}

/// Asks for a new name. A note's extension is kept out of the way: only its
/// name is shown, and the Vault keeps the extension unless another is typed.
pub fn ask_rename(
    _: &mut OrbitWindow,
    path: VaultPath,
    kind: EntryKind,
    window: &mut Window,
    cx: &mut Context<OrbitWindow>,
) {
    let name = path.name().to_string();
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if kind == EntryKind::Note && !stem.is_empty() => stem.to_string(),
        _ => name.clone(),
    };
    let input = cx.new(|cx| InputState::new(window, cx).default_value(stem));
    let view = cx.entity().downgrade();
    let submit = {
        let (input, view, path) = (input.clone(), view.clone(), path.clone());
        move |window: &mut Window, cx: &mut App| {
            let name = input.read(cx).value().trim().to_string();
            let path = path.clone();
            view.update(cx, |this, cx| this.rename_entry(path, &name, window, cx))
                .ok();
            window.close_dialog(cx);
        }
    };
    let on_enter = submit.clone();
    cx.subscribe_in(
        &input,
        window,
        move |_, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                on_enter(window, cx);
            }
        },
    )
    .detach();
    input.update(cx, |input, cx| {
        input.select_all(window, cx);
        input.focus(window, cx);
    });
    window.open_dialog(cx, move |dialog, _, _| {
        let submit = submit.clone();
        dialog
            .title(format!("Rename {name}"))
            .child(Input::new(&input))
            .footer(footer("Rename", move |window, cx| submit(window, cx)))
    });
}

/// Asks which folder to move into: every folder except the entry's own and
/// anything inside the entry.
pub fn ask_move(
    view: WeakEntity<OrbitWindow>,
    path: VaultPath,
    folders: Vec<VaultPath>,
    window: &mut Window,
    cx: &mut App,
) {
    let targets: Vec<VaultPath> = std::iter::once(VaultPath::root())
        .chain(folders)
        .filter(|f| f != &path.parent() && !f.is_within(&path))
        .collect();
    window.open_dialog(cx, move |dialog, _, cx| {
        let rows = targets.iter().enumerate().map(|(ix, folder)| {
            let (view, path, folder) = (view.clone(), path.clone(), folder.clone());
            let label = if folder.is_root() {
                "/ (vault root)".to_string()
            } else {
                folder.to_string()
            };
            Button::new(("move-target", ix))
                .ghost()
                .w_full()
                .justify_start()
                .icon(IconName::FolderClosed)
                .label(label)
                .on_click(move |_, window, cx| {
                    let (path, folder) = (path.clone(), folder.clone());
                    view.update(cx, |this, cx| this.move_entry(path, &folder, window, cx))
                        .ok();
                    window.close_dialog(cx);
                })
        });
        dialog.title(format!("Move {} to…", path.name())).child(
            v_flex()
                .id("move-targets")
                .max_h(rems(24.))
                .overflow_y_scroll()
                .children(rows)
                .when(targets.is_empty(), |this| {
                    this.child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child("There's no other folder to move it to."),
                    )
                }),
        )
    });
}

/// Confirms before sending an entry to the system trash.
pub fn ask_delete(
    _: &mut OrbitWindow,
    path: VaultPath,
    window: &mut Window,
    cx: &mut Context<OrbitWindow>,
) {
    let view = cx.entity().downgrade();
    window.open_dialog(cx, move |dialog, _, _| {
        let (view, path) = (view.clone(), path.clone());
        dialog
            .title(format!("Delete {}?", path.name()))
            .child("It moves to the system trash, where you can restore it.")
            .footer(footer("Move to trash", move |window, cx| {
                let path = path.clone();
                view.update(cx, |this, cx| this.delete_entry(path, window, cx))
                    .ok();
                window.close_dialog(cx);
            }))
    });
}

/// Cancel and a primary button that runs `ok`.
fn footer(
    ok_label: &'static str,
    ok: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_end()
        .gap_2()
        .child(
            Button::new("dialog-cancel")
                .outline()
                .label("Cancel")
                .on_click(|_, window, cx| window.close_dialog(cx)),
        )
        .child(
            Button::new("dialog-ok")
                .primary()
                .label(ok_label)
                .on_click(move |_, window, cx| ok(window, cx)),
        )
}
