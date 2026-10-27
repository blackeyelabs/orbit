//! The main window, laid out as on the design canvas: title bar, file tree,
//! tab strip with the open note, status bar and right panel.

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::list::{ListEvent, ListItem, ListState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tree::{TreeItem, TreeState, tree};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, TitleBar, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use orbit_core::{
    Backlink, EntryKind, IndexingProgress, SearchResults, Session, SessionTab, TagCount, TreeNode,
    Vault, VaultPath, Watch, same_tag,
};

use crate::appearance::{self, AppSettings};
use crate::commands::{
    CloseTab, CommandPalette, DeleteNote, LockAll, MoveNote, NewFolder, OpenAnotherVault,
    OpenSettings, ProtectFolder, QuickSwitcher, RenameNote, SearchVault, ToggleRightPanel,
    ToggleSidebar, Unlock,
};
use crate::file_actions::Protection;
use crate::file_actions::{self, NewNote};
use crate::note_editor::{NoteEditor, NoteEditorEvent};
use crate::protection;
use crate::right_panel;
use crate::search_panel;
use crate::settings_view::AppearancePane;
use crate::switcher::{self, Chosen, Switcher};
use crate::vaults;

// Canvas sizes at 100% (40, 280 and 28 px), in rems so the Interface size
// setting scales them with everything else.
const TITLE_BAR_HEIGHT: f32 = 2.5;
const SIDEBAR_WIDTH: f32 = 17.5;
const STATUS_BAR_HEIGHT: f32 = 1.75;

/// Notes a search lists at most.
const SEARCH_LIMIT: usize = 50;
/// How often indexing progress is shown.
const INDEXING_TICK: std::time::Duration = std::time::Duration::from_millis(250);

/// The key context of the window's own shortcuts.
pub const KEY_CONTEXT: &str = "OrbitWindow";

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
    /// Focused when nothing else is, so the window's shortcuts still work.
    focus: FocusHandle,
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
    /// Vault search's query field, atop the sidebar.
    search_input: Entity<InputState>,
    /// What's searched for and what was found, while the field isn't empty.
    search: Option<(String, SearchResults)>,
    /// Searching, off the UI thread.
    searching: Option<Task<()>>,
    /// While the vault is being indexed.
    indexing: Option<IndexingProgress>,
    /// The Switcher, while it's open.
    switcher: Option<(Entity<ListState<Switcher>>, Subscription)>,
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
            focus: cx.focus_handle(),
            tree: cx.new(|cx| TreeState::new(cx)),
            entries: Rc::default(),
            tabs: Vec::new(),
            active_tab: None,
            left_panel_open: true,
            right_panel_open: false,
            panel_data: PanelData::default(),
            chosen_tag: None,
            refreshing_panel: None,
            switcher: None,
            search_input: cx.new(|cx| InputState::new(window, cx).placeholder("Search the vault")),
            search: None,
            searching: None,
            indexing: None,
            error: None,
            _watch: None,
            restoring: false,
        };
        this.refresh_tree(&HashSet::new(), cx);
        cx.subscribe(&this.search_input, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.run_search(cx);
            }
        })
        .detach();
        this.index_vault(cx);
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

    /// Indexes the vault for links and search, off the UI thread (so the
    /// first preview doesn't wait for it), showing how far it's got.
    fn index_vault(&mut self, cx: &mut Context<Self>) {
        let vault = self.vault.clone();
        let done = Arc::new(AtomicBool::new(false));
        cx.background_spawn({
            let done = done.clone();
            async move {
                vault.index_links();
                vault.index_search();
                done.store(true, Ordering::Release);
            }
        })
        .detach();
        let vault = self.vault.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(INDEXING_TICK).await;
                let finished = done.load(Ordering::Acquire);
                let progress = vault.search_progress();
                let shown = this.update(cx, |this, cx| {
                    this.indexing = progress.filter(|_| !finished);
                    if finished {
                        // Results found meanwhile may have been incomplete.
                        this.run_search(cx);
                    }
                    cx.notify();
                });
                if finished || shown.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Searches the vault for what's in the search field, off the UI thread.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().trim().to_string();
        if query.is_empty() {
            self.search = None;
            self.searching = None;
            cx.notify();
            return;
        }
        let vault = self.vault.clone();
        let found = cx.background_spawn({
            let query = query.clone();
            async move { vault.search(&query, SEARCH_LIMIT) }
        });
        self.searching = Some(cx.spawn(async move |this, cx| {
            let found = found.await;
            this.update(cx, |this, cx| {
                this.search = Some((query, found));
                cx.notify();
            })
            .ok();
        }));
    }

    /// To the search field, the sidebar shown.
    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.left_panel_open = true;
        self.search_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
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
        // Indexed now, off the UI thread, rather than at the next search.
        let vault = self.vault.clone();
        let updated = cx.background_spawn(async move { vault.update_search() });
        cx.spawn(async move |this, cx| {
            updated.await;
            this.update(cx, |this, cx| {
                if this.search.is_some() {
                    this.run_search(cx);
                }
            })
            .ok();
        })
        .detach();
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
        self.open_file(followed.path.clone(), window, cx);
        if let (Some(heading), Some(ix)) = (&followed.heading, self.tab_of(&followed.path, cx)) {
            self.tabs[ix]
                .editor
                .update(cx, |editor, cx| editor.go_to_heading(heading, window, cx));
        }
    }

    /// Opens any file in the vault: a note in a tab; a canvas, which Orbit
    /// can't show yet (ADR 0005), with a word saying so; anything else in
    /// its own app.
    pub(crate) fn open_file(
        &mut self,
        path: VaultPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match EntryKind::of_file(path.name()) {
            EntryKind::Note => self.open_note(path, window, cx),
            EntryKind::Canvas => {
                let file = self.vault.file_on_disk(&path);
                file_actions::canvas_unavailable(&path, file, window, cx);
            }
            _ => cx.open_with_system(&self.vault.file_on_disk(&path)),
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

    /// Creates the note a link typed in `from` names, staying in `from`.
    fn create_linked_note(&mut self, from: &VaultPath, link: &str, cx: &mut Context<Self>) {
        match self.vault.create_linked_note(from, link) {
            Ok(path) => {
                let mut expanded = self.expanded_folders(cx);
                expanded.insert(path.parent());
                self.refresh_tree(&expanded, cx);
                for tab in &self.tabs {
                    tab.editor.update(cx, |editor, cx| editor.links_changed(cx));
                }
            }
            Err(e) => self.report(&format!("create {link}"), e, cx),
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
                            NoteEditorEvent::CreateLinkedNote(link) => {
                                let from = editor.read(cx).path().clone();
                                this.create_linked_note(&from, link, cx);
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

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.left_panel_open = !self.left_panel_open;
        cx.notify();
    }

    fn toggle_right_panel(&mut self, cx: &mut Context<Self>) {
        self.right_panel_open = !self.right_panel_open;
        self.refresh_right_panel(cx);
        cx.notify();
    }

    /// Opens the Switcher on `query`: `""` for notes, `">"` for commands.
    fn open_switcher(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let note_open = self.active().is_some();
        let state = Switcher::open(self.vault.clone(), note_open, query, window, cx);
        let subscription =
            cx.subscribe_in(
                &state,
                window,
                |this, state, event, window, cx| match event {
                    ListEvent::Confirm(_) => {
                        let chosen =
                            state.update(cx, |state, _| state.delegate_mut().take_chosen());
                        this.close_switcher(window, cx);
                        match chosen {
                            Some(Chosen::Note { path, new_tab }) => {
                                this.open_note_from_switcher(path, new_tab, window, cx)
                            }
                            Some(Chosen::Command(command)) => {
                                let action = command.action();
                                window.defer(cx, move |window, cx| {
                                    window.dispatch_action(action, cx)
                                });
                            }
                            None => {}
                        }
                    }
                    ListEvent::Cancel => this.close_switcher(window, cx),
                    ListEvent::Select(_) => {}
                },
            );
        self.switcher = Some((state, subscription));
        cx.notify();
    }

    /// Closes the Switcher, back to the open note.
    fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = None;
        // Before any command chosen is dispatched, which goes where focus is.
        match self.active() {
            Some(tab) => NoteEditor::focus(&tab.editor, window, cx),
            None => self.focus.focus(window, cx),
        }
        cx.notify();
    }

    /// Opens a note chosen in the Switcher: in a new tab, or in place of the
    /// open note (saved first). A note open already is shown where it is.
    fn open_note_from_switcher(
        &mut self,
        path: VaultPath,
        new_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If the open note can't be saved, it stays, and this opens beside it.
        let replace = self
            .active_tab
            .filter(|&old| !new_tab && self.tab_of(&path, cx).is_none() && self.save_tab(old, cx));
        let before = self.tabs.len();
        self.open_note(path, window, cx);
        if let Some(old) = replace
            && self.tabs.len() > before
        {
            let opened = self.tabs.pop().expect("a tab was just opened");
            self.tabs[old] = opened;
            self.show_tab(old, window, cx);
        }
    }

    /// The unlock prompt.
    pub(crate) fn ask_unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        protection::ask_unlock(view, self.vault.clone(), window, cx);
    }

    /// The protect flow, on `folder`.
    pub(crate) fn ask_protect(
        &mut self,
        folder: VaultPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folders = self.vault.protectable_folders();
        if let Err(e) = self.vault.can_protect(&folder) {
            return self.report(&format!("protect {}", folder.name()), e, cx);
        }
        let view = cx.entity().downgrade();
        protection::ask_protect(view, self.vault.clone(), folder, folders, window, cx);
    }

    /// Whether `path` is in an unlocked protected folder.
    pub(crate) fn is_protected(&self, path: &VaultPath) -> bool {
        self.vault.protected_folder_of(path).is_some()
    }

    /// Locks a protected folder. Its open notes are saved first and their
    /// tabs closed; if a save fails, the folder locks anyway (ADR 0004)
    /// and says what was lost.
    pub(crate) fn lock_folder(
        &mut self,
        folder: &VaultPath,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = self.close_tabs_within(folder, cx);
        self.vault.lock(folder);
        if !saved {
            self.error = Some(
                format!(
                    "Locked {}: some edits in it couldn't be saved first, and are gone",
                    folder.name()
                )
                .into(),
            );
        }
        self.protection_changed(cx);
    }

    /// Saves the notes open at or in `folder` and closes their tabs;
    /// whether every save worked.
    pub(crate) fn close_tabs_within(&mut self, folder: &VaultPath, cx: &mut Context<Self>) -> bool {
        let saved = self.save_tabs_within(folder, cx);
        while let Some(ix) = self
            .tabs
            .iter()
            .position(|t| t.editor.read(cx).path().is_within(folder))
        {
            self.tabs[ix]
                .editor
                .update(cx, |editor, _| editor.forget_disk());
            self.remove_tab(ix, cx);
        }
        saved
    }

    /// Locks every unlocked protected folder.
    pub(crate) fn lock_all_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for folder in self.vault.unlocked_folders() {
            self.lock_folder(&folder, window, cx);
        }
    }

    /// A folder was protected, unlocked or locked: the tree, panels and
    /// search show it, or don't any more.
    pub(crate) fn protection_changed(&mut self, cx: &mut Context<Self>) {
        let mut expanded = self.expanded_folders(cx);
        expanded.extend(self.vault.unlocked_folders());
        self.refresh_tree(&expanded, cx);
        self.refresh_right_panel(cx);
        for tab in &self.tabs {
            tab.editor.update(cx, |editor, cx| editor.links_changed(cx));
        }
        if self.vault.search_progress().is_none() {
            // Protecting a folder empties the search index, to be rebuilt.
            self.index_vault(cx);
        }
        if self.search.is_some() {
            self.run_search(cx);
        }
        cx.notify();
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
                cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)),
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
                cx.listener(|this, _, _, cx| this.toggle_right_panel(cx)),
            ))
    }

    fn render_sidebar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let entries = self.entries.clone();
        let (menu_view, menu_entries, folders) =
            (view.clone(), self.entries.clone(), self.folders());
        let (menu_vault, protected): (Arc<Vault>, Rc<Vec<VaultPath>>) =
            (self.vault.clone(), Rc::new(self.vault.unlocked_folders()));
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
                div().px_3().pb_2().child(
                    Input::new(&self.search_input)
                        .small()
                        .prefix(IconName::Search)
                        .when(self.search.is_none(), |input| {
                            input.suffix(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .children(Kbd::binding_for_action(
                                        &SearchVault,
                                        Some(KEY_CONTEXT),
                                        window,
                                    )),
                            )
                        })
                        .cleanable(true),
                ),
            )
            .when_some(self.search.as_ref(), |this, (query, found)| {
                this.child(search_panel::render(query, found, cx))
            })
            .when(self.search.is_none(), |this| {
                this.child(
                    div().flex_1().min_h_0().child(
                        tree(&self.tree, move |ix, entry, _selected, _, cx| {
                            let id = entry.item().id.clone();
                            let entry_info = entries.get(&id);
                            let kind = entry_info.map(|e| e.kind);
                            let icon = match kind {
                                Some(EntryKind::Folder) if entry.is_expanded() => {
                                    IconName::FolderOpen
                                }
                                Some(EntryKind::Folder) => IconName::FolderClosed,
                                Some(EntryKind::Note) => IconName::FileText,
                                Some(EntryKind::Canvas) => IconName::LayoutDashboard,
                                _ => IconName::File,
                            };
                            let is_unsaved = entry_info.is_some_and(|e| unsaved.contains(&e.path));
                            let is_protected =
                                entry_info.is_some_and(|e| protected.contains(&e.path));
                            let mut item = ListItem::new(ix)
                                .pl(px(12. + 16. * entry.depth() as f32))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(icon)
                                        .child(entry.item().label.clone())
                                        .when(is_unsaved, |this| this.child(unsaved_dot(cx)))
                                        .when(is_protected, |this| {
                                            this.child(
                                                div()
                                                    .text_color(cx.theme().success)
                                                    .child(IconName::LockOpen),
                                            )
                                        }),
                                );
                            if let Some(TreeEntryInfo { path, kind }) = entry_info.cloned()
                                && kind != EntryKind::Folder
                            {
                                let view = view.clone();
                                item = item.on_click(move |_, window, cx| {
                                    let path = path.clone();
                                    view.update(cx, |this, cx| this.open_file(path, window, cx))
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
                                    match menu_vault.protected_folder_of(&info.path) {
                                        Some(root) if root == info.path => Protection::Root,
                                        Some(_) => Protection::Inside,
                                        None => Protection::Plain,
                                    },
                                ),
                                None => menu,
                            }
                        }),
                    ),
                )
            })
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
        let right: SharedString = match self.indexing {
            Some(progress) => format!("{} · {right}", search_panel::progress_text(progress)).into(),
            None => right,
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
            .child(
                h_flex()
                    .gap_3()
                    .child(right)
                    .child(protection::status_control(
                        &self.vault.unlocked_folders(),
                        cx,
                    )),
            )
    }
}

impl Render for OrbitWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let grain = AppSettings::get(cx).paper_grain;
        // With nothing focused (the last tab closed), the window's shortcuts
        // would reach nothing: the window takes focus.
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NewNote, window, cx| {
                this.new_note(VaultPath::root(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &Unlock, window, cx| this.ask_unlock(window, cx)))
            .on_action(
                cx.listener(|this, _: &LockAll, window, cx| this.lock_all_folders(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ProtectFolder, window, cx| {
                // The open note's folder, or else the first that can be.
                let protectable = this.vault.protectable_folders();
                let folder = this
                    .active_path(cx)
                    .map(|p| p.parent())
                    .filter(|f| protectable.contains(f))
                    .or_else(|| protectable.into_iter().next());
                match folder {
                    Some(folder) => this.ask_protect(folder, window, cx),
                    None => this.report("protect a folder", "there's no folder to protect", cx),
                }
            }))
            .on_action(
                cx.listener(|this, _: &SearchVault, window, cx| this.focus_search(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &QuickSwitcher, window, cx| {
                    this.open_switcher("", window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &CommandPalette, window, cx| {
                this.open_switcher(">", window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewFolder, window, cx| {
                this.new_folder(VaultPath::root(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &RenameNote, window, cx| {
                if let Some(path) = this.active_path(cx) {
                    file_actions::ask_rename(this, path, EntryKind::Note, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &MoveNote, window, cx| {
                if let Some(path) = this.active_path(cx) {
                    let view = cx.entity().downgrade();
                    file_actions::ask_move(view, path, this.folders(), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteNote, window, cx| {
                if let Some(path) = this.active_path(cx) {
                    file_actions::ask_delete(this, path, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| {
                if let Some(ix) = this.active_tab {
                    this.close_tab(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &ToggleRightPanel, _, cx| this.toggle_right_panel(cx)))
            .on_action(
                cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)),
            )
            .on_action(
                cx.listener(|_, _: &OpenAnotherVault, _, cx| vaults::show_welcome(cx, |_| {})),
            )
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
                        this.child(self.render_sidebar(window, cx))
                    })
                    .child(self.render_centre(cx))
                    .when(self.right_panel_open, |this| {
                        this.child(self.render_right_panel(cx))
                    }),
            )
            .child(self.render_status_bar(cx))
            .when(grain, |this| this.child(appearance::grain(window, cx)))
            .when_some(self.switcher.as_ref(), |this, (state, _)| {
                let view = cx.entity().downgrade();
                this.child(switcher::render(
                    state,
                    move |window, cx| {
                        view.update(cx, |this, cx| this.close_switcher(window, cx))
                            .ok();
                    },
                    cx,
                ))
            })
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
