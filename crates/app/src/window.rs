//! The main window, laid out as on the design canvas: title bar, file tree,
//! tab strip with the open note, status bar and (empty) right panel.

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tree::{TreeItem, TreeState, tree};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, TitleBar, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use orbit_core::{EntryKind, TreeNode, Vault, VaultPath};

use crate::appearance::{self, AppSettings};
use crate::note_editor::NoteEditor;
use crate::settings_view::AppearancePane;

// Canvas sizes at 100% (40, 280, 300 and 28 px), in rems so the
// Interface size setting scales them with everything else.
const TITLE_BAR_HEIGHT: f32 = 2.5;
const SIDEBAR_WIDTH: f32 = 17.5;
const RIGHT_PANEL_WIDTH: f32 = 18.75;
const STATUS_BAR_HEIGHT: f32 = 1.75;

/// What a tree row stands for.
#[derive(Clone)]
struct TreeEntryInfo {
    path: VaultPath,
    kind: EntryKind,
}

/// A note open in a tab.
struct OpenTab {
    title: SharedString,
    editor: Entity<NoteEditor>,
}

pub struct OrbitWindow {
    vault: Arc<Vault>,
    tree: Entity<TreeState>,
    /// Tree item id → the entry it stands for. Shared with the tree's row
    /// renderer, which runs every frame.
    entries: Rc<HashMap<SharedString, TreeEntryInfo>>,
    tabs: Vec<OpenTab>,
    active_tab: Option<usize>,
    left_panel_open: bool,
    right_panel_open: bool,
    error: Option<SharedString>,
}

impl OrbitWindow {
    pub fn new(vault: Arc<Vault>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Match system follows the desktop while Orbit runs.
        cx.observe_window_appearance(window, |_, window, cx| appearance::apply(window, cx))
            .detach();
        // Closing the window saves every note; if one can't be saved, the
        // window stays open with the error in the status bar.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            this.update(cx, |this, cx| this.save_all(cx))
                .unwrap_or(true)
        });
        let mut entries = HashMap::new();
        let (items, error) = match vault.tree() {
            Ok(nodes) => (tree_items(&nodes, &mut entries), None),
            Err(e) => (
                Vec::new(),
                Some(format!("Couldn't read the vault: {e}").into()),
            ),
        };
        let tree = cx.new(|cx| TreeState::new(cx).items(items));
        OrbitWindow {
            vault,
            tree,
            entries: Rc::new(entries),
            tabs: Vec::new(),
            active_tab: None,
            left_panel_open: true,
            right_panel_open: false,
            error,
        }
    }

    /// Opens a note in a tab, or focuses its tab if it's already open.
    fn open_note(&mut self, path: VaultPath, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self
            .tabs
            .iter()
            .position(|t| t.editor.read(cx).path() == &path)
        {
            self.active_tab = Some(ix);
        } else {
            match self.vault.read_note(&path) {
                Ok(text) => {
                    let vault = self.vault.clone();
                    let title = path.name().to_string().into();
                    let editor = cx.new(|cx| NoteEditor::new(vault, path, text, window, cx));
                    // Unsaved markers, the caret and word count live in the
                    // editor. A save that worked clears an old save error.
                    cx.observe(&editor, |this, editor, cx| {
                        if !editor.read(cx).is_dirty() && this.error.is_some() {
                            this.error = None;
                        }
                        cx.notify();
                    })
                    .detach();
                    self.tabs.push(OpenTab { title, editor });
                    self.active_tab = Some(self.tabs.len() - 1);
                    self.error = None;
                }
                Err(e) => self.error = Some(format!("Couldn't open {path}: {e}").into()),
            }
        }
        if let Some(ix) = self.active_tab {
            self.show_tab(ix, window, cx);
        }
        cx.notify();
    }

    fn show_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(ix) {
            self.active_tab = Some(ix);
            NoteEditor::focus(&tab.editor, window, cx);
            cx.notify();
        }
    }

    /// Saves a tab's note now; false, with the error shown, if it couldn't.
    fn save_tab(&mut self, ix: usize, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.tabs.get(ix) else {
            return true;
        };
        let result = tab.editor.update(cx, |editor, cx| editor.save_blocking(cx));
        if let Err(e) = &result {
            self.error = Some(format!("Couldn't save {}: {e}", tab.title).into());
            cx.notify();
        }
        result.is_ok()
    }

    /// Saves every unsaved note; false if any couldn't be saved.
    fn save_all(&mut self, cx: &mut Context<Self>) -> bool {
        // Every tab is tried, even after one fails.
        let failures = (0..self.tabs.len())
            .filter(|&ix| !self.save_tab(ix, cx))
            .count();
        failures == 0
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|_| AppearancePane);
        window.open_dialog(cx, move |dialog, window, _| {
            dialog
                .title("Appearance")
                .w(window.rem_size() * 40.)
                .child(view.clone())
        });
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        // A note that can't be saved stays open, so its edits aren't lost.
        if ix >= self.tabs.len() || !self.save_tab(ix, cx) {
            return;
        }
        self.tabs.remove(ix);
        self.active_tab = match self.active_tab {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > ix => Some(a - 1),
            Some(a) => Some(a.min(self.tabs.len() - 1)),
            None => None,
        };
        cx.notify();
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match self.active_tab.and_then(|ix| self.tabs.get(ix)) {
            Some(tab) => format!("{} · {}", tab.title, self.vault.name()),
            None => self.vault.name().to_string(),
        };
        TitleBar::new()
            .h(rems(TITLE_BAR_HEIGHT))
            .child(panel_toggle(
                "toggle-left-panel",
                "Toggle sidebar",
                self.left_panel_open,
                (IconName::PanelLeftClose, IconName::PanelLeft),
                cx.listener(|this, _, _, cx| {
                    this.left_panel_open = !this.left_panel_open;
                    cx.notify();
                }),
            ))
            .child(div().flex_1().text_center().text_sm().child(title))
            .child(
                Button::new("open-settings")
                    .ghost()
                    .icon(IconName::Settings)
                    .tooltip("Settings")
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
            .child(panel_toggle(
                "toggle-right-panel",
                "Toggle right panel",
                self.right_panel_open,
                (IconName::PanelRightClose, IconName::PanelRight),
                cx.listener(|this, _, _, cx| {
                    this.right_panel_open = !this.right_panel_open;
                    cx.notify();
                }),
            ))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let entries = self.entries.clone();
        let unsaved: Rc<HashSet<VaultPath>> = Rc::new(
            self.tabs
                .iter()
                .map(|t| t.editor.read(cx))
                .filter(|e| e.is_dirty())
                .map(|e| e.path().clone())
                .collect(),
        );
        v_flex()
            .w(rems(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.vault.name().to_string()),
            )
            .child(div().flex_1().min_h_0().child(tree(
                &self.tree,
                move |ix, entry, _selected, _, cx| {
                    let id = entry.item().id.clone();
                    let entry_info = entries.get(&id);
                    let kind = entry_info.map(|e| e.kind);
                    let icon = match kind {
                        Some(EntryKind::Folder) if entry.is_expanded() => IconName::FolderOpen,
                        Some(EntryKind::Folder) => IconName::FolderClosed,
                        Some(EntryKind::Note) => IconName::FileText,
                        _ => IconName::File,
                    };
                    let is_unsaved = entry_info.is_some_and(|e| unsaved.contains(&e.path));
                    let mut item = ListItem::new(ix)
                        .pl(px(12. + 16. * entry.depth() as f32))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(icon)
                                .child(entry.item().label.clone())
                                .when(is_unsaved, |this| this.child(unsaved_dot(cx))),
                        );
                    if let Some(TreeEntryInfo {
                        path,
                        kind: EntryKind::Note,
                    }) = entry_info.cloned()
                    {
                        let view = view.clone();
                        item = item.on_click(move |_, window, cx| {
                            let path = path.clone();
                            view.update(cx, |this, cx| this.open_note(path, window, cx))
                                .ok();
                        });
                    }
                    item
                },
            )))
    }

    fn render_centre(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.active_tab.and_then(|ix| self.tabs.get(ix)) {
            Some(tab) => div()
                .flex_1()
                .min_h_0()
                .child(tab.editor.clone())
                .into_any_element(),
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("Open a note from the sidebar")
                .into_any_element(),
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .when(!self.tabs.is_empty(), |this| {
                this.child(
                    TabBar::new("tabs")
                        .selected_index(self.active_tab.unwrap_or(0))
                        .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                            let unsaved = tab.editor.read(cx).is_dirty();
                            Tab::new()
                                .label(tab.title.clone())
                                .when(unsaved, |this| this.prefix(unsaved_dot(cx)))
                                .suffix(
                                    Button::new(("close-tab", ix))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip("Close tab")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            // Don't let the click also select the tab being closed.
                                            cx.stop_propagation();
                                            this.close_tab(ix, cx);
                                        })),
                                )
                        }))
                        .on_click(cx.listener(|this, ix: &usize, window, cx| {
                            this.show_tab(*ix, window, cx)
                        })),
                )
            })
            .child(body)
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self
            .active_tab
            .and_then(|ix| self.tabs.get(ix))
            .map(|tab| tab.editor.read(cx));
        let left = match (&self.error, editor) {
            (Some(error), _) => error.clone(),
            (None, Some(editor)) => editor.status_text(),
            (None, None) => SharedString::default(),
        };
        let right = match editor {
            Some(editor) => editor.summary_text(cx),
            None => self.vault.name().to_string().into(),
        };
        let failed = self.error.is_some() || editor.is_some_and(|e| e.has_failed());
        h_flex()
            .h(rems(STATUS_BAR_HEIGHT))
            .flex_none()
            .px_3()
            .justify_between()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .when(failed, |this| this.text_color(cx.theme().danger))
                    .child(left),
            )
            .child(right)
    }
}

impl Render for OrbitWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let grain = AppSettings::get(cx).paper_grain;
        v_flex()
            .relative()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.left_panel_open, |this| {
                        this.child(self.render_sidebar(cx))
                    })
                    .child(self.render_centre(cx))
                    .when(self.right_panel_open, |this| {
                        // Outline and backlinks arrive in #14.
                        this.child(
                            div()
                                .w(rems(RIGHT_PANEL_WIDTH))
                                .flex_none()
                                .h_full()
                                .border_l_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().sidebar),
                        )
                    }),
            )
            .child(self.render_status_bar(cx))
            .when(grain, |this| this.child(appearance::grain(window, cx)))
    }
}

/// A title-bar button that shows or hides a side panel.
fn panel_toggle(
    id: &'static str,
    tooltip: &'static str,
    open: bool,
    (close_icon, open_icon): (IconName, IconName),
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .ghost()
        .icon(if open { close_icon } else { open_icon })
        .tooltip(tooltip)
        .on_click(on_click)
}

/// Builds tree items (id = vault path) and records what each one is.
fn tree_items(
    nodes: &[TreeNode],
    entries: &mut HashMap<SharedString, TreeEntryInfo>,
) -> Vec<TreeItem> {
    nodes
        .iter()
        .map(|node| {
            let id: SharedString = node.path.as_str().to_string().into();
            entries.insert(
                id.clone(),
                TreeEntryInfo {
                    path: node.path.clone(),
                    kind: node.kind,
                },
            );
            let item = TreeItem::new(id, node.name.clone());
            if node.kind == EntryKind::Folder {
                item.children(tree_items(&node.children, entries))
            } else {
                item
            }
        })
        .collect()
}

/// The unsaved-changes marker on tabs and tree rows.
fn unsaved_dot(cx: &App) -> impl IntoElement {
    div()
        .size(rems(0.4))
        .rounded_full()
        .bg(cx.theme().muted_foreground)
}
