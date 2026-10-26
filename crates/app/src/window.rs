//! The main window, laid out as on the design canvas: title bar, file tree,
//! tab strip with the open note, status bar and (empty) right panel.

use std::{collections::HashMap, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tree::{TreeItem, TreeState, tree};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, TitleBar};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use orbit_core::{EntryKind, TreeNode, Vault, VaultPath};

const TITLE_BAR_HEIGHT: f32 = 40.;
const SIDEBAR_WIDTH: f32 = 280.;
const RIGHT_PANEL_WIDTH: f32 = 300.;
const STATUS_BAR_HEIGHT: f32 = 28.;

/// What a tree row stands for.
#[derive(Clone)]
struct TreeEntryInfo {
    path: VaultPath,
    kind: EntryKind,
}

/// A note open in a tab, read-only for now (editing arrives in #9).
struct OpenTab {
    path: VaultPath,
    title: SharedString,
    text: SharedString,
}

pub struct OrbitWindow {
    vault: Vault,
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
    pub fn new(vault: Vault, _window: &mut Window, cx: &mut Context<Self>) -> Self {
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
    fn open_note(&mut self, path: VaultPath, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| t.path == path) {
            self.active_tab = Some(ix);
        } else {
            match self.vault.read_note(&path) {
                Ok(text) => {
                    self.tabs.push(OpenTab {
                        title: path.name().to_string().into(),
                        text: text.into(),
                        path,
                    });
                    self.active_tab = Some(self.tabs.len() - 1);
                    self.error = None;
                }
                Err(e) => self.error = Some(format!("Couldn't open {path}: {e}").into()),
            }
        }
        cx.notify();
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
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
            .h(px(TITLE_BAR_HEIGHT))
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
        v_flex()
            .w(px(SIDEBAR_WIDTH))
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
                move |ix, entry, _selected, _, _| {
                    let id = entry.item().id.clone();
                    let entry_info = entries.get(&id);
                    let kind = entry_info.map(|e| e.kind);
                    let icon = match kind {
                        Some(EntryKind::Folder) if entry.is_expanded() => IconName::FolderOpen,
                        Some(EntryKind::Folder) => IconName::FolderClosed,
                        Some(EntryKind::Note) => IconName::FileText,
                        _ => IconName::File,
                    };
                    let mut item = ListItem::new(ix)
                        .pl(px(12. + 16. * entry.depth() as f32))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(icon)
                                .child(entry.item().label.clone()),
                        );
                    if let Some(TreeEntryInfo {
                        path,
                        kind: EntryKind::Note,
                    }) = entry_info.cloned()
                    {
                        let view = view.clone();
                        item = item.on_click(move |_, _, cx| {
                            let path = path.clone();
                            view.update(cx, |this, cx| this.open_note(path, cx)).ok();
                        });
                    }
                    item
                },
            )))
    }

    fn render_centre(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.active_tab.and_then(|ix| self.tabs.get(ix)) {
            Some(tab) => div()
                .id("note")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_8()
                .py_6()
                .child(div().whitespace_normal().child(tab.text.clone()))
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
                            Tab::new().label(tab.title.clone()).suffix(
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
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            this.active_tab = Some(*ix);
                            cx.notify();
                        })),
                )
            })
            .child(body)
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let left = match (
            &self.error,
            self.active_tab.and_then(|ix| self.tabs.get(ix)),
        ) {
            (Some(error), _) => error.clone(),
            (None, Some(tab)) => format!("{} · read-only", tab.path).into(),
            (None, None) => SharedString::from(""),
        };
        h_flex()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .px_3()
            .justify_between()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(left)
            .child(self.vault.name().to_string())
    }
}

impl Render for OrbitWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
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
                                .w(px(RIGHT_PANEL_WIDTH))
                                .flex_none()
                                .h_full()
                                .border_l_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().sidebar),
                        )
                    }),
            )
            .child(self.render_status_bar(cx))
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
