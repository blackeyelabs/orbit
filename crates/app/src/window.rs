//! The main window, laid out as on the design canvas: title bar, file tree,
//! tab strip with the open note, status bar and right panel.

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
use orbit_core::{
    Backlink, EntryKind, Session, SessionTab, TagCount, TreeNode, Vault, VaultPath, Watch, same_tag,
};

use crate::appearance::{self, AppSettings};
use crate::file_actions::{self, NewNote};
use crate::note_editor::{NoteEditor, NoteEditorEvent};
use crate::right_panel;
use crate::settings_view::AppearancePane;
use crate::vaults;

// Canvas sizes at 100% (40, 280 and 28 px), in rems so the Interface size
// setting scales them with everything else.
const TITLE_BAR_HEIGHT: f32 = 2.5;
const SIDEBAR_WIDTH: f32 = 17.5;
const STATUS_BAR_HEIGHT: f32 = 1.75;

/// What a tree row stands for.
#[derive(Clone)]
struct TreeEntryInfo {
    path: VaultPath,
    kind: EntryKind,
}

/// A note open in a tab.
struct OpenTab {
    editor: Entity<NoteEditor>,
}

impl OpenTab {
    fn title(&self, cx: &App) -> SharedString {
        self.editor.read(cx).path().name().to_string().into()
    }
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
    /// What the right panel shows from the vault, while it's open.
    panel_data: PanelData,
    /// The tag chosen in the right panel, to list its notes.
    chosen_tag: Option<String>,
    /// Finding what the right panel shows, off the UI thread.
    refreshing_panel: Option<Task<()>>,
    error: Option<SharedString>,
    /// Changes by other apps (and Orbit) arrive while this is held.
    _watch: Option<Watch>,
    /// Tabs are reopening from the session, which isn't saved meanwhile.
    restoring: bool,
}

impl OrbitWindow {
    pub fn new(vault: Arc<Vault>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Match system follows the desktop while Orbit runs.
        cx.observe_window_appearance(window, |_, window, cx| appearance::apply(window, cx))
            .detach();
        // Closing the window saves every note and the session; if a note
        // can't be saved, the window stays open with the error in the status
        // bar.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            this.update(cx, |this, cx| {
                let saved = this.save_all(cx);
                this.save_session(cx);
                saved
            })
            .unwrap_or(true)
        });
        let mut this = OrbitWindow {
            vault,
            tree: cx.new(|cx| TreeState::new(cx)),
            entries: Rc::default(),
            tabs: Vec::new(),
            active_tab: None,
            left_panel_open: true,
            right_panel_open: false,
            panel_data: PanelData::default(),
            chosen_tag: None,
            refreshing_panel: None,
            error: None,
            _watch: None,
            restoring: false,
        };
        this.refresh_tree(&HashSet::new(), cx);
        // Off the UI thread, so the first preview doesn't wait for it.
        let vault = this.vault.clone();
        cx.background_spawn(async move { vault.index_links() })
            .detach();
        this.watch_vault(cx);
        this.restore_session(window, cx);
        this
    }

    /// Reopens the tabs open when the vault last closed, scrolled where they
    /// were, with the same one active.
    fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let session = self.vault.session();
        self.restoring = true;
        for SessionTab { path, scroll } in &session.tabs {
            self.open_note(path.clone(), window, cx);
            if let Some(tab) = self.tab_of(path, cx) {
                self.tabs[tab]
                    .editor
                    .update(cx, |editor, cx| editor.set_scroll(*scroll, cx));
            }
        }
        self.restoring = false;
        let active = session.active.map(|ix| &session.tabs[ix].path);
        if let Some(ix) = active.and_then(|path| self.tab_of(path, cx)) {
            self.show_tab(ix, window, cx);
        }
    }

    /// Remembers the open tabs for next time: when they change, so a crash
    /// loses little, and as the window closes, with where each is scrolled.
    fn save_session(&self, cx: &App) {
        if self.restoring {
            return;
        }
        let session = Session {
            tabs: self
                .tabs
                .iter()
                .map(|tab| {
                    let editor = tab.editor.read(cx);
                    SessionTab {
                        path: editor.path().clone(),
                        scroll: editor.scroll(cx),
                    }
                })
                .collect(),
            active: self.active_tab,
        };
        if let Err(e) = self.vault.save_session(&session) {
            eprintln!(
                "orbit: couldn't save the session of {}: {e}",
                self.vault.name()
            );
        }
    }

    /// The tab showing the note at `path`.
    fn tab_of(&self, path: &VaultPath, cx: &App) -> Option<usize> {
        self.tabs
            .iter()
            .position(|t| t.editor.read(cx).path() == path)
    }

    /// Follows changes other apps, git or sync make while the vault is open.
    fn watch_vault(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::unbounded();
        match self.vault.watch(move |paths| {
            tx.send_blocking(paths).ok();
        }) {
            Ok(watch) => self._watch = Some(watch),
            Err(e) => return self.report("watch the vault for changes", e, cx),
        }
        cx.spawn(async move |this, cx| {
            while let Ok(paths) = rx.recv().await {
                if this
                    .update(cx, |this, cx| this.changed_on_disk(&paths, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// A batch of changed paths: the tree reloads, open notes at or inside
    /// them check what's on disk now, and links may resolve differently.
    fn changed_on_disk(&mut self, paths: &[VaultPath], cx: &mut Context<Self>) {
        let expanded = self.expanded_folders(cx);
        self.refresh_tree(&expanded, cx);
        self.refresh_right_panel(cx);
        for tab in &self.tabs {
            let affected = paths
                .iter()
                .any(|p| tab.editor.read(cx).path().is_within(p));
            tab.editor.update(cx, |editor, cx| {
                if affected {
                    editor.disk_changed(cx);
                }
                editor.links_changed(cx);
            });
        }
    }

    /// Follows a link clicked in `from`'s preview: a note opens in a tab, at
    /// the heading the link names; an Attachment opens in its app; and an
    /// unresolved link creates its note first.
    fn follow_link(
        &mut self,
        from: &VaultPath,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let followed = match self.vault.follow_link(from, target) {
            Ok(followed) => followed,
            Err(e) => return self.report(&format!("follow {target}"), e, cx),
        };
        if followed.created {
            let mut expanded = self.expanded_folders(cx);
            expanded.insert(followed.path.parent());
            self.refresh_tree(&expanded, cx);
        }
        match EntryKind::of_file(followed.path.name()) {
            EntryKind::Note => {
                self.open_note(followed.path.clone(), window, cx);
                if let (Some(heading), Some(ix)) =
                    (&followed.heading, self.tab_of(&followed.path, cx))
                {
                    self.tabs[ix]
                        .editor
                        .update(cx, |editor, cx| editor.go_to_heading(heading, window, cx));
                }
            }
            _ => cx.open_with_system(&self.vault.file_on_disk(&followed.path)),
        }
    }

    fn active(&self) -> Option<&OpenTab> {
        self.tabs.get(self.active_tab?)
    }

    /// The active note's path.
    fn active_path(&self, cx: &App) -> Option<VaultPath> {
        Some(self.active()?.editor.read(cx).path().clone())
    }

    /// Finds what the right panel shows again, in the background, if it's
    /// open: the active note's backlinks, the tags and the chosen tag's
    /// notes. Meanwhile it shows what was last found.
    fn refresh_right_panel(&mut self, cx: &mut Context<Self>) {
        if !self.right_panel_open {
            self.panel_data = PanelData::default();
            self.refreshing_panel = None;
            return;
        }
        let (vault, path, tag) = (
            self.vault.clone(),
            self.active_path(cx),
            self.chosen_tag.clone(),
        );
        let found = cx.background_spawn(async move {
            PanelData {
                backlinks: path.map(|path| {
                    let backlinks = vault.backlinks(&path);
                    (path, backlinks)
                }),
                tags: vault.tags(),
                tagged: tag.map(|tag| {
                    let notes = vault.notes_tagged(&tag);
                    (tag, notes)
                }),
            }
        });
        self.refreshing_panel = Some(cx.spawn(async move |this, cx| {
            let found = found.await;
            this.update(cx, |this, cx| {
                // A chosen tag no note has any more is let go.
                if let Some(chosen) = &this.chosen_tag
                    && !found.tags.iter().any(|tag| same_tag(&tag.name, chosen))
                {
                    this.chosen_tag = None;
                }
                this.panel_data = found;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Lists the notes of the tag `name` in the right panel, or, for the
    /// tag already chosen or none, all the tags again.
    pub(crate) fn choose_tag(&mut self, name: Option<&str>, cx: &mut Context<Self>) {
        let chosen = self.chosen_tag.as_deref();
        let same = name.zip(chosen).is_some_and(|(a, b)| same_tag(a, b));
        self.chosen_tag = name.filter(|_| !same).map(str::to_string);
        self.refresh_right_panel(cx);
        cx.notify();
    }

    /// Puts the active note's caret on the line at `offset`.
    pub(crate) fn go_to_line_of(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.active() {
            tab.editor
                .update(cx, |editor, cx| editor.go_to_line_of(offset, window, cx));
        }
    }

    /// Reloads the tree from the vault, with the folders in `expanded` open.
    fn refresh_tree(&mut self, expanded: &HashSet<VaultPath>, cx: &mut Context<Self>) {
        let mut entries = HashMap::new();
        let items = match self.vault.tree() {
            Ok(nodes) => tree_items(&nodes, expanded, &mut entries),
            Err(e) => {
                self.error = Some(format!("Couldn't read the vault: {e}").into());
                Vec::new()
            }
        };
        self.entries = Rc::new(entries);
        self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
        cx.notify();
    }

    /// The folders open in the tree now.
    fn expanded_folders(&self, cx: &App) -> HashSet<VaultPath> {
        let tree = self.tree.read(cx);
        (0..)
            .map_while(|ix| tree.entry(ix))
            .filter(|entry| entry.is_expanded())
            .filter_map(|entry| self.entries.get(&entry.item().id))
            .map(|info| info.path.clone())
            .collect()
    }

    /// Every folder in the vault, in tree order.
    fn folders(&self) -> Vec<VaultPath> {
        let mut folders: Vec<VaultPath> = self
            .entries
            .values()
            .filter(|e| e.kind == EntryKind::Folder)
            .map(|e| e.path.clone())
            .collect();
        folders.sort_by_key(|f| f.as_str().to_lowercase());
        folders
    }

    /// Shows a failed file operation in the status bar.
    fn report(&mut self, what: &str, error: impl std::fmt::Display, cx: &mut Context<Self>) {
        self.error = Some(format!("Couldn't {what}: {error}").into());
        cx.notify();
    }

    /// A new empty note in `folder`, opened in a tab and ready for typing.
    pub fn new_note(&mut self, folder: VaultPath, window: &mut Window, cx: &mut Context<Self>) {
        match self.vault.create_note(&folder) {
            Ok(path) => {
                let mut expanded = self.expanded_folders(cx);
                expanded.insert(folder);
                self.refresh_tree(&expanded, cx);
                self.open_note(path, window, cx);
            }
            Err(e) => self.report("create a note", e, cx),
        }
    }

    /// A new folder in `parent`, then asks for its name.
    pub fn new_folder(&mut self, parent: VaultPath, window: &mut Window, cx: &mut Context<Self>) {
        match self.vault.create_folder(&parent) {
            Ok(path) => {
                let mut expanded = self.expanded_folders(cx);
                expanded.insert(parent);
                self.refresh_tree(&expanded, cx);
                file_actions::ask_rename(self, path, EntryKind::Folder, window, cx);
            }
            Err(e) => self.report("create a folder", e, cx),
        }
    }

    pub fn rename_entry(
        &mut self,
        path: VaultPath,
        new_name: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vault = self.vault.clone();
        self.relocate(&path, "rename", || vault.rename(&path, new_name), cx);
    }

    pub fn move_entry(
        &mut self,
        path: VaultPath,
        folder: &VaultPath,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vault = self.vault.clone();
        self.relocate(&path, "move", || vault.move_to(&path, folder), cx);
    }

    /// Renames or moves `path` with `relocate`, its open notes saved first.
    /// Afterwards open tabs follow their notes, and the tree keeps its open
    /// folders, under their new paths too.
    fn relocate(
        &mut self,
        path: &VaultPath,
        verb: &str,
        relocate: impl FnOnce() -> Result<VaultPath, orbit_core::Error>,
        cx: &mut Context<Self>,
    ) {
        if !self.save_tabs_within(path, cx) {
            return;
        }
        let to = match relocate() {
            Ok(to) => to,
            Err(e) => return self.report(&format!("{verb} {}", path.name()), e, cx),
        };
        let expanded = self
            .expanded_folders(cx)
            .into_iter()
            .map(|folder| folder.rebase(path, &to))
            .chain(std::iter::once(to.parent()))
            .collect();
        for tab in &self.tabs {
            tab.editor.update(cx, |editor, cx| {
                if editor.path().is_within(path) {
                    let moved = editor.path().rebase(path, &to);
                    editor.set_path(moved, cx);
                }
            });
        }
        self.error = None;
        self.refresh_tree(&expanded, cx);
        self.refresh_right_panel(cx);
    }

    /// Sends an entry to the trash and closes the tabs of notes inside it.
    /// Their notes are saved first, so the trash holds the latest text; if
    /// one can't be, nothing is deleted.
    pub fn delete_entry(&mut self, path: VaultPath, _: &mut Window, cx: &mut Context<Self>) {
        if !self.save_tabs_within(&path, cx) {
            return;
        }
        if let Err(e) = self.vault.delete(&path) {
            return self.report(&format!("delete {}", path.name()), e, cx);
        }
        while let Some(ix) = self
            .tabs
            .iter()
            .position(|t| t.editor.read(cx).path().is_within(&path))
        {
            self.tabs[ix]
                .editor
                .update(cx, |editor, _| editor.forget_disk());
            self.remove_tab(ix, cx);
        }
        self.error = None;
        let expanded = self.expanded_folders(cx);
        self.refresh_tree(&expanded, cx);
        self.refresh_right_panel(cx);
    }

    /// Saves the open notes at or inside `path`; false if any couldn't be.
    fn save_tabs_within(&mut self, path: &VaultPath, cx: &mut Context<Self>) -> bool {
        let affected: Vec<usize> = (0..self.tabs.len())
            .filter(|&ix| self.tabs[ix].editor.read(cx).path().is_within(path))
            .collect();
        affected
            .into_iter()
            .filter(|&ix| !self.save_tab(ix, cx))
            .count()
            == 0
    }

    /// Opens a note in a tab, or focuses its tab if it's already open.
    pub(crate) fn open_note(
        &mut self,
        path: VaultPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.tab_of(&path, cx) {
            self.active_tab = Some(ix);
        } else {
            match self.vault.open_note(&path) {
                Ok((note, text)) => {
                    let vault = self.vault.clone();
                    let editor = cx.new(|cx| NoteEditor::new(vault, note, text, window, cx));
                    cx.subscribe_in(
                        &editor,
                        window,
                        |this, editor, event, window, cx| match event {
                            NoteEditorEvent::Close => {
                                if let Some(ix) = this.tabs.iter().position(|t| &t.editor == editor)
                                {
                                    editor.update(cx, |editor, _| editor.forget_disk());
                                    this.remove_tab(ix, cx);
                                }
                            }
                            NoteEditorEvent::FollowLink(target) => {
                                let from = editor.read(cx).path().clone();
                                this.follow_link(&from, target, window, cx);
                            }
                        },
                    )
                    .detach();
                    // Unsaved markers, the caret and word count live in the
                    // editor. A save that worked clears an old save error.
                    cx.observe(&editor, |this, editor, cx| {
                        if !editor.read(cx).is_dirty() && this.error.is_some() {
                            this.error = None;
                        }
                        cx.notify();
                    })
                    .detach();
                    self.tabs.push(OpenTab { editor });
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
            self.save_session(cx);
            self.refresh_right_panel(cx);
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
            let what = format!("save {}", tab.title(cx));
            self.report(&what, e, cx);
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
        if ix < self.tabs.len() && self.save_tab(ix, cx) {
            self.remove_tab(ix, cx);
        }
    }

    fn remove_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.tabs.remove(ix);
        self.active_tab = match self.active_tab {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > ix => Some(a - 1),
            Some(a) => Some(a.min(self.tabs.len() - 1)),
            None => None,
        };
        self.save_session(cx);
        self.refresh_right_panel(cx);
        cx.notify();
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match self.active() {
            Some(tab) => format!("{} · {}", tab.title(cx), self.vault.name()),
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
                    this.refresh_right_panel(cx);
                    cx.notify();
                }),
            ))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let entries = self.entries.clone();
        let (menu_view, menu_entries, folders) =
            (view.clone(), self.entries.clone(), self.folders());
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
                h_flex()
                    .pl_3()
                    .pr_1()
                    .py_1()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.vault.name().to_string()),
                    )
                    .child(
                        Button::new("open-another-vault")
                            .ghost()
                            .small()
                            .icon(IconName::FolderOpen)
                            .tooltip("Open another vault")
                            .on_click(|_, _, cx| vaults::show_welcome(cx, |_| {})),
                    )
                    .child(
                        Button::new("new-note")
                            .ghost()
                            .small()
                            .icon(IconName::FilePlus)
                            .tooltip("New note (Ctrl+N)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_note(VaultPath::root(), window, cx)
                            })),
                    )
                    .child(
                        Button::new("new-folder")
                            .ghost()
                            .small()
                            .icon(IconName::FolderPlus)
                            .tooltip("New folder")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_folder(VaultPath::root(), window, cx)
                            })),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    tree(&self.tree, move |ix, entry, _selected, _, cx| {
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
                    })
                    .context_menu(move |_, entry, menu, _, _| {
                        match menu_entries.get(&entry.item().id) {
                            Some(info) => file_actions::tree_menu(
                                menu,
                                menu_view.clone(),
                                info.path.clone(),
                                info.kind,
                                folders.clone(),
                            ),
                            None => menu,
                        }
                    }),
                ),
            )
    }

    fn render_centre(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.active() {
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
                                .label(tab.title(cx))
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

    fn render_right_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let outline = self.active().map(|tab| tab.editor.read(cx).outline());
        // Until what's current is found, nothing rather than another note's
        // backlinks or another tag's notes.
        let backlinks = match (&self.panel_data.backlinks, self.active_path(cx)) {
            (Some((path, backlinks)), Some(active)) if *path == active => backlinks.as_slice(),
            _ => &[],
        };
        let tagged = match (&self.panel_data.tagged, &self.chosen_tag) {
            (Some((tag, notes)), Some(chosen)) if tag == chosen => {
                Some((tag.as_str(), notes.as_slice()))
            }
            (_, Some(chosen)) => Some((chosen.as_str(), &[][..])),
            _ => None,
        };
        let contents = right_panel::Contents {
            outline: outline.as_deref(),
            backlinks,
            tags: &self.panel_data.tags,
            tagged,
        };
        right_panel::render(contents, cx)
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.active().map(|tab| tab.editor.read(cx));
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
            .key_context(file_actions::KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &NewNote, window, cx| {
                this.new_note(VaultPath::root(), window, cx)
            }))
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
                        this.child(self.render_right_panel(cx))
                    }),
            )
            .child(self.render_status_bar(cx))
            .when(grain, |this| this.child(appearance::grain(window, cx)))
    }
}

/// What the right panel shows from the vault.
#[derive(Default)]
struct PanelData {
    /// The active note's backlinks, with its path.
    backlinks: Option<(VaultPath, Vec<Backlink>)>,
    tags: Vec<TagCount>,
    /// The chosen tag's notes, with the tag.
    tagged: Option<(String, Vec<VaultPath>)>,
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
    expanded: &HashSet<VaultPath>,
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
                item.expanded(expanded.contains(&node.path))
                    .children(tree_items(&node.children, expanded, entries))
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
