//! One open note, under its inline title (its name, edited to rename it):
//! in Live Preview, or Source mode (always for `.txt` and very large
//! notes); with autosave. The text editor is reached through
//! `text_editor`.

use std::{
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use gpui_kit::assets::IconName;
use gpui_kit::base::input::{Copy as CopyText, Cut};
use gpui_kit::base::{TextSelection, h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Icon;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use md_engine::{
    Format, Heading, PropertyEdit, PropertyType, format_edit, headings, property_edit, word_count,
};
use orbit_core::{
    DiskCheck, EntryKind, OpenNote, Scroll, Trouble, Vault, VaultPath, Zeroizing, is_large_note,
    is_plain_text_note,
};

use crate::clipboard_guard::{self, Copying};
use crate::file_actions::editable_name;
use crate::link_completion::LinkCompletions;
use crate::properties::{PropertiesEvent, PropertiesPanel};
use crate::text_editor::{Shown, TextEditor};

actions!(
    note_editor,
    [
        /// Save the note now.
        Save,
        /// Ctrl+B.
        ToggleBold,
        /// Ctrl+I.
        ToggleItalic,
        /// Ctrl+K.
        InsertLink,
        /// Ctrl+E: Live Preview or Source mode.
        ToggleLivePreview
    ]
);

pub const KEY_CONTEXT: &str = "NoteEditor";

/// How long typing must pause before the note saves itself.
const AUTOSAVE_DELAY: Duration = Duration::from_millis(1500);
/// The pause in typing after which the Outline and word count catch up,
/// rather than on every key (docs/research/performance.md).
const DERIVED_PAUSE: Duration = Duration::from_millis(150);
/// While typing goes on, they still catch up at least this often.
const DERIVED_AT_LEAST: Duration = Duration::from_secs(1);

pub fn init(cx: &mut App) {
    crate::properties::init(cx);
    cx.bind_keys([
        KeyBinding::new("ctrl-s", Save, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-b", ToggleBold, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-i", ToggleItalic, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-k", InsertLink, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-e", ToggleLivePreview, Some(KEY_CONTEXT)),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The note formatted, its syntax showing where the caret is (#35).
    LivePreview,
    /// The markdown as typed: always for `.txt` notes, and very large
    /// notes until Live Preview is asked for.
    Source,
}

impl Mode {
    /// The mode a note opens in: Source when it's plain text or too large
    /// for anything else.
    fn of(source_only: bool) -> Self {
        if source_only {
            Mode::Source
        } else {
            Mode::LivePreview
        }
    }

    /// The editor that shows the text in this mode.
    fn shown(self) -> Shown {
        match self {
            Mode::LivePreview => Shown::LivePreview,
            Mode::Source => Shown::Source,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::LivePreview => "Live Preview",
            Mode::Source => "Source mode",
        }
    }
}

/// The language the source editor highlights: none for a note shown only
/// as source.
fn language(source_only: bool) -> &'static str {
    if source_only { "plaintext" } else { "markdown" }
}

/// Where the note's edits stand against the disk.
struct Saves {
    /// Bumped on every edit; the note is saved when `saved` equals it.
    revision: u64,
    saved: u64,
    writing: bool,
    /// Another save was asked for while one was writing.
    again: bool,
    /// The watcher reported a change while a write held the note: check
    /// again once it's done.
    recheck: bool,
    /// The note's trouble as of its last save or check, for rendering.
    trouble: Option<Trouble>,
    /// Shared with writes on background threads, which run one at a time.
    disk: Arc<Mutex<Disk>>,
    /// Set once Orbit deleted the note or its folder locked: writes that
    /// haven't started are skipped. Unlike `disk`, never waits on a write.
    forgotten: Arc<AtomicBool>,
}

/// What a write needs to know when its turn comes, rather than when it was
/// queued: the note may have moved, been deleted, or been saved newer.
struct Disk {
    /// `None` once Orbit deleted the note: nothing more is written.
    note: Option<OpenNote>,
    /// The newest revision on disk; an older write is skipped.
    written: u64,
}

impl Disk {
    /// Lets go of the note once the editor forgot it.
    fn forget_if(&mut self, forgotten: &AtomicBool) {
        if forgotten.load(Ordering::Acquire) {
            self.note = None;
        }
    }
}

impl Saves {
    fn new(note: OpenNote) -> Self {
        Saves {
            revision: 0,
            saved: 0,
            writing: false,
            again: false,
            recheck: false,
            trouble: None,
            disk: Arc::new(Mutex::new(Disk {
                note: Some(note),
                written: 0,
            })),
            forgotten: Arc::default(),
        }
    }

    fn disk(&self) -> std::sync::MutexGuard<'_, Disk> {
        let mut disk = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        disk.forget_if(&self.forgotten);
        disk
    }

    fn is_dirty(&self) -> bool {
        self.revision != self.saved
    }

    fn is_blocked(&self) -> bool {
        matches!(
            self.trouble,
            Some(Trouble::Changed(_) | Trouble::Deleted | Trouble::Unreadable(_))
        )
    }
}

pub enum NoteEditorEvent {
    /// The user chose to close the note without saving (it was deleted).
    Close,
    /// A link was clicked in the preview or Live Preview: its target,
    /// `Note#Heading`.
    FollowLink(String),
    /// "Create … as a new note" was chosen for a link being typed: the
    /// link, whose note is wanted now.
    CreateLinkedNote(String),
    /// The inline title was edited and left (or Enter pressed): the note's
    /// new name (its extension kept unless another is typed).
    Rename(String),
}

impl EventEmitter<NoteEditorEvent> for NoteEditor {}

pub struct NoteEditor {
    vault: Arc<Vault>,
    path: VaultPath,
    mode: Mode,
    source: TextEditor,
    /// Wikilink autocomplete, which links from this note.
    links: Rc<LinkCompletions>,
    /// The inline title: the note's name, edited to rename it. Unless
    /// it's being edited, it shows the name the note has.
    title: Entity<InputState>,
    /// The name the title was last given, to see the note renamed.
    title_name: String,
    /// The note's properties, edited under the title in Live Preview.
    properties: Entity<PropertiesPanel>,
    /// The source as of the last edit.
    text: SharedString,
    words: usize,
    /// The source's headings, for the Outline.
    outline: Rc<[Heading]>,
    saves: Saves,
    /// Text from disk for the editor to show, on the next frame.
    reload: Option<String>,
    /// Shown with a failed save: where Save elsewhere put a copy.
    saved_elsewhere: Option<SharedString>,
    autosave: Option<Task<()>>,
    /// The Outline and word count catching up with typing, and
    /// since when they've been behind.
    catching_up: Option<(Task<()>, std::time::Instant)>,
    /// The scroll restored from the session, until the editor is laid out.
    restored_scroll: Option<Scroll>,
    /// The note's size, while it's opened as source for being very large.
    large: Option<usize>,
    /// While a Copy or Cut agreed to in a protected note runs: it goes
    /// ahead.
    copy_agreed: bool,
    /// Held by the note's whole view, so a selection of its rendered text
    /// goes when focus leaves the note (else Copy elsewhere would take it).
    focus: FocusHandle,
    _focus_out: Subscription,
    /// Enter in the inline title, and leaving it.
    _title_events: Subscription,
    _title_blur: Subscription,
    _properties_events: Subscription,
    _subscription: Subscription,
}

impl NoteEditor {
    pub fn new(
        vault: Arc<Vault>,
        note: OpenNote,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let path = note.path().clone();
        let large = is_large_note(&text).then_some(text.len());
        let source_only = large.is_some() || is_plain_text_note(&path);
        let mode = Mode::of(source_only);
        let links = Rc::new(LinkCompletions::new(vault.clone(), path.clone()));
        let source = TextEditor::new(&text, language(source_only), links.clone(), window, cx);
        source.show(mode.shown(), window, cx);
        let subscription = Subscription::join(
            source.on_edit(window, cx, |this, window, cx| this.edited(window, cx)),
            source.on_follow_link(cx, |_, target, cx| {
                cx.emit(NoteEditorEvent::FollowLink(target.to_string()))
            }),
        );
        let focus = cx.focus_handle();
        let focus_out = cx.on_focus_out(&focus, window, |this, _, window, cx| {
            if this.is_protected() {
                TextSelection::clear(window, cx);
            }
        });
        let title = cx.new(|cx| {
            InputState::new(window, cx).default_value(editable_name(&path, EntryKind::Note))
        });
        let title_events = cx.subscribe_in(&title, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                // Leaving the title renames.
                this.source.focus_handle(cx).focus(window, cx);
            }
        });
        let properties = cx.new(|cx| {
            let mut panel = PropertiesPanel::new(vault.clone(), path.clone());
            panel.set_modified(vault.modified(&path), cx);
            panel
        });
        let properties_events = cx.subscribe_in(
            &properties,
            window,
            |this, _, event, window, cx| match event {
                PropertiesEvent::Edit(edit) => this.edit_property(edit, window, cx),
                PropertiesEvent::EditInSource(offset) => this.edit_in_source(*offset, window, cx),
                PropertiesEvent::SetType { key, kind } => this.set_type(key, *kind, window, cx),
            },
        );
        let title_focus = title.read(cx).focus_handle(cx);
        let title_blur = cx.on_blur(&title_focus, window, |this, window, cx| {
            this.rename_from_title(window, cx)
        });
        NoteEditor {
            links,
            saves: Saves::new(note),
            vault,
            title,
            title_name: editable_name(&path, EntryKind::Note),
            properties,
            path,
            mode,
            source,
            words: word_count(&text),
            outline: outline_of(source_only, &text),
            text: text.into(),
            reload: None,
            saved_elsewhere: None,
            autosave: None,
            catching_up: None,
            restored_scroll: None,
            large,
            copy_agreed: false,
            focus,
            _focus_out: focus_out,
            _title_events: title_events,
            _title_blur: title_blur,
            _properties_events: properties_events,
            _subscription: subscription,
        }
    }

    /// Copy or Cut in a protected note, before the editor has it: asked
    /// about first, then cleared from the clipboard later
    /// (clipboard_guard). In the editor, once agreed, the editor copies as
    /// it would have (every cursor's selection); even with nothing
    /// selected it asks, as other cursors may have some. A selection of
    /// rendered text outside the editor is taken now, since a dialog
    /// clears it.
    fn copy_protected(&mut self, copying: Copying, window: &mut Window, cx: &mut Context<Self>) {
        if self.copy_agreed {
            return;
        }
        if self.source.focus_handle(cx).contains_focused(window, cx) {
            cx.stop_propagation();
            let this = cx.entity().downgrade();
            let source = self.source.clone();
            let copy = move |window: &mut Window, cx: &mut App| {
                this.update(cx, |this, _| this.copy_agreed = true).ok()?;
                source.copy(copying, window, cx);
                this.update(cx, |this, _| this.copy_agreed = false).ok()?;
                clipboard_guard::clipboard_text(cx)
            };
            return clipboard_guard::ask(copying, copy, window, cx);
        }
        // Rendered text has nothing to cut.
        if copying == Copying::Cut {
            return;
        }
        let selected = Zeroizing::new(TextSelection::selected_text(window, cx));
        let selected = Zeroizing::new(selected.trim().to_string());
        if selected.is_empty() {
            return;
        }
        cx.stop_propagation();
        let copy = move |_: &mut Window, cx: &mut App| {
            cx.write_to_clipboard(ClipboardItem::new_string(selected.to_string()));
            Some(selected.clone())
        };
        clipboard_guard::ask(copying, copy, window, cx);
    }

    /// Leaving the inline title, as in Obsidian: asks for the rename (the
    /// window does it, as from the file tree). Whether it worked or not,
    /// the title then shows the name the note has.
    fn rename_from_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.title.read(cx).value().trim().to_string();
        let unchanged = name == self.name();
        if !name.is_empty() && !unchanged {
            cx.emit(NoteEditorEvent::Rename(name));
        }
        // Renamed, it then shows the new name (see `render`).
        let name = self.name();
        self.title
            .update(cx, |title, cx| title.set_value(name, window, cx));
    }

    /// The note's name as the inline title shows it: without its extension.
    fn name(&self) -> String {
        editable_name(&self.path, EntryKind::Note)
    }

    /// Whether the note is in a protected folder (unlocked, as it's open).
    fn is_protected(&self) -> bool {
        self.vault.protected_folder_of(&self.path).is_some()
    }

    pub fn path(&self) -> &VaultPath {
        &self.path
    }

    /// Follows the note to a new path: after Orbit renames or moves it, or
    /// saves a copy. Writes still queued go to the new path. A new extension
    /// can change the mode.
    pub fn set_path(&mut self, path: VaultPath, cx: &mut Context<Self>) {
        if let Some(note) = self.saves.disk().note.as_mut() {
            note.set_path(path.clone());
        }
        self.follow_path(path, cx);
    }

    fn follow_path(&mut self, path: VaultPath, cx: &mut Context<Self>) {
        let was_source_only = self.is_source_only();
        self.links.set_note(path.clone());
        self.path = path.clone();
        self.properties.update(cx, |panel, _| panel.set_note(path));
        self.refresh_modified(cx);
        // A new extension can make it source only, or no longer.
        let mode = if was_source_only != self.is_source_only() {
            Mode::of(self.is_source_only())
        } else {
            self.mode
        };
        self.set_mode(mode, cx);
    }

    /// Whether the note shows only as source: plain text, or opened as
    /// source for being very large.
    fn is_source_only(&self) -> bool {
        self.large.is_some() || is_plain_text_note(&self.path)
    }

    /// Shows the note in `mode`: the editor's highlighting and the Outline
    /// follow, and the editor shown does on the next frame.
    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.source
            .set_language(language(self.is_source_only()), cx);
        self.mode = mode;
        self.outline = outline_of(self.is_source_only(), &self.text);
        self.links_changed(cx);
    }

    /// Ctrl+E: Live Preview, or Source mode. A note shown only as source
    /// stays so.
    fn toggle_live_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_source_only() {
            return;
        }
        let mode = match self.mode {
            Mode::LivePreview => Mode::Source,
            Mode::Source => Mode::LivePreview,
        };
        self.set_mode(mode, cx);
        self.source.show(mode.shown(), window, cx);
        self.source.focus_handle(cx).focus(window, cx);
    }

    /// Notes were added, removed or renamed: links may resolve differently,
    /// so Live Preview draws them again.
    pub fn links_changed(&mut self, cx: &mut Context<Self>) {
        self.source.redraw(cx);
        cx.notify();
    }

    /// Puts the caret on the heading a link named, if the note has it.
    pub fn go_to_heading(&mut self, subpath: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(offset) = md_engine::heading_offset(&self.text, subpath) {
            self.go_to_line_of(offset, window, cx);
        }
    }

    /// Puts the caret at the start of the line holding `offset`, scrolled
    /// into view.
    pub fn go_to_line_of(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        let offset = offset.min(self.text.len());
        let line = self.text.as_bytes()[..offset]
            .iter()
            .filter(|&&b| b == b'\n')
            .count() as u32;
        self.source.go_to_line(line, window, cx);
        // Not `NoteEditor::focus`: that reads this editor, mid-update here.
        self.source.focus_handle(cx).focus(window, cx);
    }

    /// Puts the caret at the start of the note's last line (in a new note,
    /// the one below its front matter), scrolled into view.
    pub fn go_to_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go_to_line_of(self.text.len(), window, cx);
    }

    /// The note's headings as of the last edit; none in Source mode.
    pub fn outline(&self) -> Rc<[Heading]> {
        self.outline.clone()
    }

    /// After Orbit deletes the note, or as its folder locks: writes still
    /// queued are dropped, so none can bring it back. Doesn't wait for one
    /// that's writing.
    pub fn forget_disk(&mut self, cx: &mut App) {
        self.autosave = None;
        self.saves.forgotten.store(true, Ordering::Release);
        // No decrypted text is kept in Live Preview's undo either.
        self.source.forget_history(cx);
    }

    pub fn is_dirty(&self) -> bool {
        self.saves.is_dirty()
    }

    /// The note's text if some of it isn't saved (or its last save failed).
    pub fn unsaved_text(&self) -> Option<Zeroizing<String>> {
        (self.is_dirty() || self.has_failed()).then(|| Zeroizing::new(self.text.to_string()))
    }

    pub fn has_failed(&self) -> bool {
        self.saves.trouble.is_some()
    }

    /// The status bar's left side: Saved, Unsaved, Saving… or the trouble.
    pub fn status_text(&self) -> SharedString {
        match &self.saves.trouble {
            Some(Trouble::Changed(_) | Trouble::Unreadable(_)) => "Changed by another app".into(),
            Some(Trouble::Deleted) => "Deleted by another app".into(),
            Some(Trouble::SaveFailed(error)) => format!("Save failed: {error}").into(),
            None if self.saves.writing => "Saving…".into(),
            None if self.is_dirty() => "Unsaved".into(),
            None => "Saved".into(),
        }
    }

    /// The status bar's right side: word count, caret and mode.
    pub fn summary_text(&self, cx: &App) -> SharedString {
        let (line, column) = self.source.caret_line_column(cx);
        let words = self.words;
        format!(
            "{words} word{} · Ln {}, Col {} · {}",
            if words == 1 { "" } else { "s" },
            line + 1,
            column + 1,
            self.mode.label()
        )
        .into()
    }

    /// Where the note is scrolled.
    pub fn scroll(&self, cx: &App) -> Scroll {
        // Before its first layout (a restored tab never shown), the source
        // editor hasn't taken the scroll it was given yet.
        self.source
            .scroll(cx)
            .or(self.restored_scroll)
            .unwrap_or_default()
    }

    /// Scrolls the note, from the next layout on.
    pub fn set_scroll(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        self.restored_scroll = Some(scroll);
        self.source.set_scroll(scroll, cx);
    }

    /// Moves keyboard focus to the source pane.
    pub fn focus(entity: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let handle = entity.read(cx).source.focus_handle(cx);
        handle.focus(window, cx);
    }

    fn edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _span = tracing::trace_span!("editor.edited").entered();
        self.text = self.source.text(cx);
        self.sync_properties(window, cx);
        self.catch_up_later(cx);
        let caret = self.source.caret(cx);
        if let Some(link) = self.links.accepted_create(&self.text, caret) {
            cx.emit(NoteEditorEvent::CreateLinkedNote(link));
        }
        self.saves.revision += 1;
        self.autosave = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            this.update(cx, |this, cx| this.save(cx)).ok();
        }));
        cx.notify();
    }

    /// The source as of now, and everything derived from it, at once.
    fn set_text(&mut self, text: SharedString, cx: &mut Context<Self>) {
        self.text = text;
        self.catch_up(cx);
    }

    /// The Outline and word count, brought up to date with the source.
    fn catch_up(&mut self, cx: &mut Context<Self>) {
        self.catching_up = None;
        self.words = word_count(&self.text);
        self.outline = outline_of(self.is_source_only(), &self.text);
        self.links_changed(cx);
    }

    /// Brings the Outline and word count up to date once typing
    /// pauses, or after a second of it at most.
    fn catch_up_later(&mut self, cx: &mut Context<Self>) {
        let since = self
            .catching_up
            .as_ref()
            .map_or_else(std::time::Instant::now, |(_, since)| *since);
        let wait = DERIVED_PAUSE.min(DERIVED_AT_LEAST.saturating_sub(since.elapsed()));
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| this.catch_up(cx)).ok();
        });
        self.catching_up = Some((task, since));
    }

    /// The watcher saw the note's path change: the note reloads if it has no
    /// unsaved edits, and otherwise shows what the user has to decide.
    /// Orbit's own saves compare equal and change nothing. While a write
    /// holds the note, the check waits for it rather than the UI.
    pub fn disk_changed(&mut self, cx: &mut Context<Self>) {
        let has_edits = self.is_dirty();
        let checked = match self.saves.disk.try_lock() {
            Ok(mut guard) => {
                let disk = &mut *guard;
                disk.forget_if(&self.saves.forgotten);
                disk.note.as_mut().map(|note| {
                    let check = note.check_disk(&self.vault, has_edits);
                    if let DiskCheck::Reload(_) = check {
                        disk.written = self.saves.revision;
                    }
                    (check, note.trouble().cloned())
                })
            }
            Err(_) => {
                self.saves.recheck = true;
                return;
            }
        };
        let Some((check, trouble)) = checked else {
            return;
        };
        let was_blocked = self.saves.is_blocked();
        self.set_trouble(trouble);
        match check {
            DiskCheck::Reload(text) => self.show_saved_text(text, cx),
            // Back to the version Orbit knows: saving resumes.
            DiskCheck::Unchanged if was_blocked && !self.saves.is_blocked() => self.save(cx),
            _ => {}
        }
        cx.notify();
    }

    fn set_trouble(&mut self, trouble: Option<Trouble>) {
        if trouble
            .as_ref()
            .is_none_or(|t| !matches!(t, Trouble::SaveFailed(_)))
        {
            self.saved_elsewhere = None;
        }
        if trouble.is_some() {
            self.autosave = None;
        }
        self.saves.trouble = trouble;
    }

    /// Shows text that's what's on disk, with nothing unsaved. The editor
    /// takes it on the next frame, which has the window it needs.
    fn show_saved_text(&mut self, text: String, cx: &mut Context<Self>) {
        // Grown very large meanwhile (in another app, say): source only.
        if self.large.is_none() && self.mode != Mode::Source && is_large_note(&text) {
            self.large = Some(text.len());
            self.set_mode(Mode::Source, cx);
        }
        self.reload = Some(text.clone());
        self.set_text(text.into(), cx);
        self.autosave = None;
        self.saves.saved = self.saves.revision;
        self.refresh_modified(cx);
        cx.notify();
    }

    /// Saves in the background, one write at a time. Paused while another
    /// app's change or deletion waits for the user.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.autosave = None;
        if self.saves.writing {
            self.saves.again = true;
            return;
        }
        if self.saves.is_blocked() || (!self.is_dirty() && !self.has_failed()) {
            return;
        }
        let write = self.writer(cx);
        self.saves.writing = true;
        cx.notify();
        let write = cx.background_spawn(async move { write() });
        cx.spawn(async move |this, cx| {
            let (revision, saved, trouble) = write.await;
            this.update(cx, |this, cx| {
                this.saves.writing = false;
                if saved {
                    this.saves.saved = this.saves.saved.max(revision);
                    this.refresh_modified(cx);
                }
                this.set_trouble(trouble);
                if std::mem::take(&mut this.saves.recheck) {
                    this.disk_changed(cx);
                }
                if std::mem::take(&mut this.saves.again) {
                    this.save(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Saves on the spot, for closing a tab or the window. It waits for any
    /// background write first, so the newest text lands last.
    pub fn save_blocking(&mut self, cx: &mut Context<Self>) -> Result<(), SharedString> {
        self.autosave = None;
        if !self.saves.is_blocked() && (self.is_dirty() || self.has_failed()) {
            let (revision, saved, trouble) = self.writer(cx)();
            if saved {
                self.saves.saved = self.saves.saved.max(revision);
            }
            self.set_trouble(trouble);
            cx.notify();
        }
        match &self.saves.trouble {
            None => Ok(()),
            Some(Trouble::SaveFailed(error)) => Err(error.clone().into()),
            Some(_) => Err("another app changed or deleted it; choose what to keep first".into()),
        }
    }

    /// A write of the current text, serialised with every other write of
    /// this note. When its turn comes it writes to the note's path then, and
    /// not at all if a newer revision is on disk already or Orbit deleted the
    /// note. It never replaces another app's change. Returns the revision,
    /// whether it's on disk, and the note's trouble afterwards.
    fn writer(&self, cx: &App) -> impl FnOnce() -> (u64, bool, Option<Trouble>) + Send + 'static {
        let (vault, disk) = (self.vault.clone(), self.saves.disk.clone());
        let forgotten = self.saves.forgotten.clone();
        let (text, revision) = (self.source.text(cx), self.saves.revision);
        move || {
            let mut disk = disk.lock().unwrap_or_else(|e| e.into_inner());
            disk.forget_if(&forgotten);
            let written = disk.written;
            let Some(note) = disk.note.as_mut() else {
                return (revision, false, None);
            };
            if revision <= written {
                return (revision, true, note.trouble().cloned());
            }
            let saved = note.save(&vault, &text).is_ok();
            let trouble = note.trouble().cloned();
            if saved {
                disk.written = revision;
            }
            (revision, saved, trouble)
        }
    }

    /// Runs one of the user's choices on the note, with the editor's text,
    /// and takes the note's trouble from it afterwards. Writing by choice is
    /// rare enough to do while the UI waits.
    fn choose(
        &mut self,
        cx: &mut Context<Self>,
        choice: impl FnOnce(&mut OpenNote, &Vault, &str) -> Option<VaultPath>,
    ) {
        let text = self.source.text(cx);
        let (moved, trouble) = {
            let mut disk = self.saves.disk();
            let Some(note) = disk.note.as_mut() else {
                return;
            };
            let moved = choice(note, &self.vault, &text);
            let trouble = note.trouble().cloned();
            if trouble.is_none() {
                disk.written = self.saves.revision;
            }
            (moved, trouble)
        };
        if trouble.is_none() {
            self.saves.saved = self.saves.revision;
        }
        self.set_trouble(trouble);
        if let Some(path) = moved {
            self.follow_path(path, cx);
        }
        cx.notify();
    }

    /// Load theirs: the other app's text replaces the unsaved edits.
    fn load_theirs(&mut self, cx: &mut Context<Self>) {
        let theirs = {
            let mut disk = self.saves.disk();
            let theirs = disk.note.as_mut().and_then(OpenNote::load_theirs);
            if theirs.is_some() {
                disk.written = self.saves.revision;
            }
            theirs
        };
        if let Some(text) = theirs {
            self.set_trouble(None);
            self.show_saved_text(text, cx);
        }
    }

    /// Save elsewhere: asks where, then writes the edits there. The note
    /// itself stays unsaved until a save into the vault works.
    fn save_elsewhere(&mut self, cx: &mut Context<Self>) {
        let directory = std::env::home_dir().unwrap_or_default();
        let chosen = cx.prompt_for_new_path(&directory, Some(self.path.name()));
        let text = self.source.text(cx);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(file))) = chosen.await else {
                return;
            };
            let result = orbit_core::save_elsewhere(&file, &text);
            this.update(cx, |this, cx| {
                this.saved_elsewhere = Some(match result {
                    Ok(()) => format!("A copy is saved at {}.", file.display()).into(),
                    Err(e) => format!("Couldn't save at {} either: {e}", file.display()).into(),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let trouble = self.saves.trouble.as_ref()?;
        let theme = cx.theme();
        let keep_mine = || {
            Button::new("keep-mine")
                .outline()
                .label("Keep mine")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.choose(cx, |note, vault, text| {
                        note.keep_mine(vault, text).ok();
                        None
                    })
                }))
        };
        let (colour, message, actions): (Hsla, SharedString, Vec<Button>) = match trouble {
            Trouble::Changed(_) => (
                theme.warning,
                "Another app changed this note.".into(),
                vec![
                    Button::new("load-theirs")
                        .primary()
                        .label("Load theirs")
                        .on_click(cx.listener(|this, _, _, cx| this.load_theirs(cx))),
                    keep_mine(),
                ],
            ),
            Trouble::Unreadable(reason) => (
                theme.warning,
                format!("Another app changed this note: {reason}.").into(),
                vec![keep_mine()],
            ),
            Trouble::Deleted => (
                theme.warning,
                "This note was deleted by another app.".into(),
                vec![
                    Button::new("save-copy")
                        .primary()
                        .label("Save a copy")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.choose(cx, |note, vault, text| note.save_copy(vault, text).ok())
                        })),
                    Button::new("close-deleted")
                        .outline()
                        .label("Close")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(NoteEditorEvent::Close))),
                ],
            ),
            Trouble::SaveFailed(error) => (
                theme.danger,
                match &self.saved_elsewhere {
                    Some(note) => format!("Couldn't save: {error}. {note}").into(),
                    None => format!("Couldn't save: {error}").into(),
                },
                vec![
                    Button::new("try-again")
                        .primary()
                        .label("Try again")
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    Button::new("save-elsewhere")
                        .outline()
                        .label("Save elsewhere")
                        .on_click(cx.listener(|this, _, _, cx| this.save_elsewhere(cx))),
                ],
            ),
        };
        Some(
            h_flex()
                .flex_none()
                .gap_3()
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(colour)
                .bg(colour.opacity(0.1))
                .child(div().flex_1().text_sm().child(message))
                .children(actions.into_iter().map(|b| b.small()))
                .into_any_element(),
        )
    }

    /// A very large note, opened as source: says so, with Live Preview a
    /// click away.
    fn render_large_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bytes = self.large?;
        let warning = cx.theme().warning;
        // The canvas's: 12 × 18 px padding, a 1.5 px rule.
        Some(
            h_flex()
                .flex_none()
                .gap_3()
                .px(rems(1.125))
                .py(rems(0.75))
                .border_b(px(1.5))
                .border_color(warning)
                .bg(warning.opacity(0.22))
                .child(Icon::new(IconName::Info).text_color(warning))
                .child(
                    v_flex()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(format!("Large note · {}", megabytes(bytes))),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Opened as source to keep typing fast."),
                        ),
                )
                .child(
                    Button::new("use-live-preview")
                        .outline()
                        .small()
                        .label("Use live preview")
                        .on_click(cx.listener(|this, _, _, cx| this.show_live_preview_anyway(cx))),
                )
                .into_any_element(),
        )
    }

    /// Shows a very large note in Live Preview after all.
    fn show_live_preview_anyway(&mut self, cx: &mut Context<Self>) {
        self.large = None;
        self.set_mode(Mode::of(self.is_source_only()), cx);
    }

    /// Bold, Italic or Link on the text's selection; nothing while the
    /// inline title has focus.
    fn apply_format(&mut self, kind: Format, window: &mut Window, cx: &mut Context<Self>) {
        if !self.source.focus_handle(cx).contains_focused(window, cx) {
            return;
        }
        let edit = format_edit(&self.source.text(cx), self.source.selection(cx), kind);
        self.source.apply(edit, window, cx);
    }

    /// A change made in the properties panel: only its key's lines change,
    /// as one edit to undo. One that can't be made (a name another key
    /// has) leaves the panel showing what the note says.
    fn edit_property(&mut self, edit: &PropertyEdit, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.source.text(cx);
        match property_edit(&text, edit, self.source.selection(cx)) {
            Ok(edit) => self.source.apply(edit, window, cx),
            Err(_) => {
                self.properties.update(cx, |panel, _| panel.forget_shown());
                self.sync_properties(window, cx);
            }
        }
        cx.notify();
    }

    /// The properties panel shows the front matter as the text has it.
    fn sync_properties(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text.clone();
        self.properties
            .update(cx, |panel, cx| panel.sync(&text, window, cx));
    }

    /// A type chosen in the properties panel: saved for the vault (or
    /// the note's protected folder), and the value converted to it if
    /// that loses nothing.
    fn set_type(
        &mut self,
        key: &str,
        kind: PropertyType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = self.vault.set_property_type(&self.path, key, kind) {
            tracing::warn!(error = e.kind_name(), "couldn't save a property type");
            return;
        }
        let value = md_engine::properties(&self.text)
            .into_iter()
            .find(|p| p.key == key)
            .and_then(|p| md_engine::convert(&p.value, kind));
        if let Some(value) = value {
            let key = key.to_string();
            self.edit_property(&PropertyEdit::Set { key, value }, window, cx);
        }
        self.properties.update(cx, |panel, _| panel.forget_shown());
        self.sync_properties(window, cx);
    }

    /// "Edit in Source" in the properties panel: Source mode, with the
    /// caret on the key's line.
    fn edit_in_source(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Source {
            self.set_mode(Mode::Source, cx);
            self.source.show(Mode::Source.shown(), window, cx);
        }
        self.go_to_line_of(offset, window, cx);
    }

    /// The file's modified time, as the panel shows it, read again.
    fn refresh_modified(&mut self, cx: &mut Context<Self>) {
        let modified = self.vault.modified(&self.path);
        self.properties
            .update(cx, |panel, cx| panel.set_modified(modified, cx));
    }
}

impl Render for NoteEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(text) = self.reload.take() {
            // Loading emits no change event, so this isn't an edit.
            self.source.load(text, window, cx);
        }
        if self.source.shown(cx) != self.mode.shown() {
            self.source.show(self.mode.shown(), window, cx);
        }
        // Renamed (from the title or elsewhere): the title shows the new
        // name, unless it's being edited.
        let name = self.name();
        let editing = self.title.read(cx).focus_handle(cx).is_focused(window);
        if !editing && self.title_name != name {
            self.title_name = name.clone();
            self.title
                .update(cx, |title, cx| title.set_value(name, window, cx));
        }
        // The inline title, shown as a heading above the text.
        let title = div().flex_none().px_4().pt_4().pb_1().child(
            Input::new(&self.title)
                .appearance(false)
                .text_size(rems(1.75))
                .font_weight(FontWeight::BOLD),
        );
        // The properties panel, in Live Preview only: Source mode shows the
        // front matter as written.
        let properties = (self.mode == Mode::LivePreview).then(|| {
            // Text loaded from disk isn't an edit: it's shown from here.
            self.sync_properties(window, cx);
            self.properties.clone()
        });
        let editor = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(title)
            .children(properties)
            .child(div().flex_1().min_h_0().child(self.source.element(cx)));
        let banner = self.render_banner(cx);
        let body = h_flex()
            .flex_1()
            .min_h_0()
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save(cx)))
            .on_action(cx.listener(|this, _: &ToggleBold, window, cx| {
                this.apply_format(Format::Bold, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleItalic, window, cx| {
                this.apply_format(Format::Italic, window, cx)
            }))
            .on_action(cx.listener(|this, _: &InsertLink, window, cx| {
                this.apply_format(Format::Link, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleLivePreview, window, cx| {
                this.toggle_live_preview(window, cx)
            }))
            .child(editor);
        let protected = self.is_protected();
        v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .when(protected, |this| {
                this.capture_action(cx.listener(|this, _: &CopyText, window, cx| {
                    this.copy_protected(Copying::Copy, window, cx)
                }))
                .capture_action(cx.listener(|this, _: &Cut, window, cx| {
                    this.copy_protected(Copying::Cut, window, cx)
                }))
            })
            .children(banner)
            .children(self.render_large_banner(cx))
            .child(body)
    }
}

/// "4.2 MB": a large note's size.
fn megabytes(bytes: usize) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.)
}

/// A note's Outline: its headings, unless it shows only as source.
fn outline_of(source_only: bool, text: &str) -> Rc<[Heading]> {
    if source_only {
        Rc::new([])
    } else {
        headings(text).into()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    // Named, not globbed: Kit's glob brings gpui's `test` macro, which then
    // shadows `#[test]` inside `#[gpui::test]`.
    use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext, Window};
    use orbit_core::{Vault, VaultPath};

    use super::NoteEditor;
    use crate::appearance::AppSettings;

    /// Opens `name` (holding `text`) in a note editor in a headless window,
    /// its text focused.
    fn open<'a>(cx: &'a mut TestAppContext, name: &str, text: &str) -> Opened<'a> {
        open_with(cx, name, text, |_| {}, |_| {})
    }

    /// A note editor in a headless window, and the vault's folder.
    type Opened<'a> = (
        Entity<NoteEditor>,
        &'a mut VisualTestContext,
        tempfile::TempDir,
    );

    /// As [`open`], with the settings changed by `settings` and the vault
    /// by `vault_setup` first.
    fn open_with<'a>(
        cx: &'a mut TestAppContext,
        name: &str,
        text: &str,
        settings: impl FnOnce(&mut orbit_core::Settings),
        vault_setup: impl FnOnce(&Vault),
    ) -> Opened<'a> {
        cx.update(|cx| {
            gpui_kit::init(cx);
            editor::init(cx);
            super::init(cx);
            let mut app = AppSettings::for_tests();
            settings(&mut app.settings);
            cx.set_global(app);
        });
        let dir = tempfile::tempdir().expect("a temporary vault");
        let file = dir.path().join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
        let vault = Arc::new(Vault::open(dir.path()).expect("the vault opens"));
        vault_setup(&vault);
        let path = VaultPath::new(name).unwrap();
        let (note, text) = vault.open_note(&path).expect("the note opens");
        // Under Kit's root, as in the app, so dialogs can open.
        let editor = std::rc::Rc::new(std::cell::RefCell::new(None));
        let held = editor.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            let editor = cx.new(|cx| NoteEditor::new(vault, note, text, window, cx));
            *held.borrow_mut() = Some(editor.clone());
            gpui_kit::component::Root::new(editor, window, cx)
        });
        let editor = editor.take().expect("the window made the editor");
        cx.update(|window, cx| NoteEditor::focus(&editor, window, cx));
        cx.run_until_parked();
        (editor, cx, dir)
    }

    /// The mode the status bar names.
    fn mode(editor: &Entity<NoteEditor>, cx: &mut VisualTestContext) -> String {
        let summary = cx.update(|_, cx| editor.read(cx).summary_text(cx));
        summary.rsplit(" · ").next().unwrap().to_string()
    }

    #[gpui::test]
    fn a_note_opens_in_live_preview_and_ctrl_e_switches_to_source_and_back(
        cx: &mut TestAppContext,
    ) {
        let (editor, cx, _dir) = open(cx, "Note.md", "# Hi\n\nSome **bold** text");
        assert_eq!(mode(&editor, cx), "Live Preview");
        cx.simulate_keystrokes("ctrl-e");
        cx.run_until_parked();
        assert_eq!(mode(&editor, cx), "Source mode");
        // Typed in Source mode, it's there back in Live Preview.
        cx.simulate_keystrokes("ctrl-end");
        cx.simulate_input("!");
        cx.simulate_keystrokes("ctrl-e");
        cx.run_until_parked();
        assert_eq!(mode(&editor, cx), "Live Preview");
        let text = cx.update(|_, cx| editor.read(cx).source.text(cx));
        assert_eq!(text, "# Hi\n\nSome **bold** text!");
    }

    #[gpui::test]
    fn a_plain_text_note_stays_in_source_mode(cx: &mut TestAppContext) {
        let (editor, cx, _dir) = open(cx, "List.txt", "milk\neggs");
        assert_eq!(mode(&editor, cx), "Source mode");
        cx.simulate_keystrokes("ctrl-e");
        cx.run_until_parked();
        assert_eq!(mode(&editor, cx), "Source mode");
    }

    #[gpui::test]
    fn go_to_heading_puts_the_caret_on_it_in_live_preview(cx: &mut TestAppContext) {
        let text = "# Top\n\nIntro\n\n## Part two\n\nMore";
        let (editor, cx, _dir) = open(cx, "Note.md", text);
        cx.update(|window, cx| editor.update(cx, |e, cx| e.go_to_heading("Part two", window, cx)));
        cx.run_until_parked();
        let caret = cx.update(|_, cx| editor.read(cx).source.caret(cx));
        assert_eq!(caret, text.find("## Part two").unwrap());
        assert_eq!(mode(&editor, cx), "Live Preview");
    }

    #[gpui::test]
    fn copying_from_a_protected_note_in_live_preview_is_cleared_later(cx: &mut TestAppContext) {
        // Agreed to for good: copying goes straight on.
        let (editor, cx, _dir) = open_protected(cx, |s| {
            s.ask_before_protected_copy = false;
            s.clear_clipboard_seconds = 5;
        });
        assert_eq!(mode(&editor, cx), "Live Preview");
        cx.simulate_keystrokes("ctrl-a ctrl-c");
        cx.run_until_parked();
        let held = |cx: &mut VisualTestContext| cx.read_from_clipboard().and_then(|c| c.text());
        assert_eq!(held(cx).as_deref(), Some("the zeppelin plan"));
        // The guard clears what it let through.
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(6));
        cx.run_until_parked();
        assert_eq!(held(cx).unwrap_or_default(), "", "cleared");
    }

    /// A note in a protected folder, `Secret/Plan.md`, opened with the
    /// settings changed by `settings`.
    fn open_protected<'a>(
        cx: &'a mut TestAppContext,
        settings: impl FnOnce(&mut orbit_core::Settings),
    ) -> Opened<'a> {
        open_with(
            cx,
            "Secret/Plan.md",
            "the zeppelin plan",
            settings,
            |vault| {
                vault.set_kdf_params(orbit_crypto::KdfParams::INSECURE_TEST);
                let keys = vault.prepare_protection("pass").unwrap();
                let folder = VaultPath::new("Secret").unwrap();
                vault.protect_folder(&folder, keys).unwrap();
            },
        )
    }

    #[gpui::test]
    fn copying_from_a_protected_note_in_live_preview_asks_first(cx: &mut TestAppContext) {
        use gpui_kit::component::WindowExt as _;
        let (editor, cx, _dir) = open_protected(cx, |_| {});
        assert_eq!(mode(&editor, cx), "Live Preview");
        cx.simulate_keystrokes("ctrl-a ctrl-c");
        cx.run_until_parked();
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), None);
    }

    #[gpui::test]
    fn cutting_from_a_protected_note_in_live_preview_is_cleared_later(cx: &mut TestAppContext) {
        let (editor, cx, _dir) = open_protected(cx, |s| {
            s.ask_before_protected_copy = false;
            s.clear_clipboard_seconds = 5;
        });
        cx.simulate_keystrokes("ctrl-a ctrl-x");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.read(cx).source.text(cx)), "");
        let held = |cx: &mut VisualTestContext| cx.read_from_clipboard().and_then(|c| c.text());
        assert_eq!(held(cx).as_deref(), Some("the zeppelin plan"));
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(6));
        cx.run_until_parked();
        assert_eq!(held(cx).unwrap_or_default(), "", "cleared");
    }

    #[gpui::test]
    fn accepting_the_create_row_in_live_preview_asks_for_the_note(cx: &mut TestAppContext) {
        let (editor, cx, _dir) = open(cx, "Note.md", "");
        let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = asked.clone();
        cx.update(|_, cx| {
            cx.subscribe(&editor, move |_, event: &super::NoteEditorEvent, _| {
                if let super::NoteEditorEvent::CreateLinkedNote(link) = event {
                    seen.borrow_mut().push(link.clone());
                }
            })
            .detach();
        });
        cx.simulate_input("[[Brand new");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| editor.read(cx).source.text(cx)),
            "[[Brand new]]"
        );
        assert_eq!(*asked.borrow(), ["Brand new"]);
    }

    /// The note's text as the editor has it.
    fn text_of(editor: &Entity<NoteEditor>, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| editor.read(cx).source.text(cx).to_string())
    }

    /// Each property row: its key, what its value field shows, whether
    /// it's flagged.
    fn rows(
        editor: &Entity<NoteEditor>,
        cx: &mut VisualTestContext,
    ) -> Vec<(String, String, bool)> {
        cx.update(|_, cx| editor.read(cx).properties.read(cx).shown_rows(cx))
    }

    /// Focuses `key`'s value field in the properties panel.
    fn focus_value(editor: &Entity<NoteEditor>, key: &str, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let panel = editor.read(cx).properties.clone();
            panel.update(cx, |panel, cx| panel.focus_value(key, window, cx));
        });
        cx.run_until_parked();
    }

    /// Clicks something in the properties panel.
    fn click(
        editor: &Entity<NoteEditor>,
        cx: &mut VisualTestContext,
        f: impl FnOnce(
            &mut crate::properties::PropertiesPanel,
            &mut Window,
            &mut gpui::Context<crate::properties::PropertiesPanel>,
        ),
    ) {
        cx.update(|window, cx| {
            let panel = editor.read(cx).properties.clone();
            panel.update(cx, |panel, cx| f(panel, window, cx));
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_property_edited_in_the_panel_changes_only_its_line_and_undo_takes_it_back(
        cx: &mut TestAppContext,
    ) {
        let note = "---\n# why\nstatus: draft # for now\ncount: 1\n---\nBody";
        let (editor, cx, _dir) = open(cx, "Note.md", note);
        assert_eq!(
            rows(&editor, cx),
            [
                ("status".into(), "draft".into(), false),
                ("count".into(), "1".into(), false)
            ]
        );
        focus_value(&editor, "status", cx);
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("done");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(text_of(&editor, cx), note.replace("draft", "done"));
        // Undo in the note takes it back, and the panel shows it.
        cx.update(|window, cx| NoteEditor::focus(&editor, window, cx));
        cx.simulate_keystrokes("ctrl-z");
        cx.run_until_parked();
        assert_eq!(text_of(&editor, cx), note);
        assert_eq!(rows(&editor, cx)[0].1, "draft");
    }

    #[gpui::test]
    fn tags_are_chips_suggested_from_the_vaults_tags(cx: &mut TestAppContext) {
        let note = "---\ntags: [Productive]\n---\nBody";
        let (editor, cx, _dir) = open_with(
            cx,
            "Note.md",
            note,
            |_| {},
            |vault| {
                let other = vault.root().join("Other.md");
                std::fs::write(other, "#project/alpha #Prowl #improve #productive #work").unwrap();
            },
        );
        focus_value(&editor, "tags", cx);
        cx.simulate_input("pro");
        cx.run_until_parked();
        // Those starting with it first; not one the note has (in any case).
        let suggested = cx.update(|_, cx| editor.read(cx).properties.read(cx).suggestions("tags"));
        assert_eq!(suggested, ["project", "project/alpha", "Prowl", "improve"]);
        cx.simulate_keystrokes("down down enter");
        cx.run_until_parked();
        assert_eq!(
            text_of(&editor, cx),
            "---\ntags: [Productive, project/alpha]\n---\nBody"
        );
        // A tag of its own, typed with its `#`.
        cx.simulate_input("#new");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(
            text_of(&editor, cx),
            "---\ntags: [Productive, project/alpha, new]\n---\nBody"
        );
    }

    #[gpui::test]
    fn yaml_the_panel_cannot_edit_offers_edit_in_source(cx: &mut TestAppContext) {
        let note = "---\nok: yes\nnested:\n  a: 1\n---\nBody";
        let (editor, cx, _dir) = open(cx, "Note.md", note);
        assert_eq!(
            rows(&editor, cx),
            [
                ("ok".into(), "yes".into(), false),
                ("nested".into(), "Edit in Source".into(), false)
            ]
        );
        click(&editor, cx, |panel, _, cx| {
            panel.click_edit_in_source("nested", cx)
        });
        assert_eq!(mode(&editor, cx), "Source mode");
        let line = cx.update(|_, cx| editor.read(cx).source.caret_line_column(cx));
        assert_eq!(line, (2, 0), "on the key's line");
    }

    #[gpui::test]
    fn a_value_that_does_not_fit_its_type_is_flagged_and_left_as_written(cx: &mut TestAppContext) {
        let note = "---\ndue: 2026-13-40\ntags: 5\n---\nBody";
        let (editor, cx, _dir) = open(cx, "Note.md", note);
        assert_eq!(
            rows(&editor, cx),
            [
                ("due".into(), "2026-13-40".into(), true),
                ("tags".into(), "5".into(), true)
            ]
        );
        assert_eq!(text_of(&editor, cx), note, "never rewritten");
        // Flagged as it's typed, and set as typed.
        focus_value(&editor, "due", cx);
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("2026-12-01");
        cx.run_until_parked();
        assert!(!rows(&editor, cx)[0].2, "it fits now");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(
            text_of(&editor, cx),
            note.replace("2026-13-40", "2026-12-01")
        );
    }

    #[gpui::test]
    fn properties_are_added_toggled_and_removed(cx: &mut TestAppContext) {
        let (editor, cx, _dir) = open(cx, "Note.md", "Body");
        click(&editor, cx, |panel, window, cx| panel.click_add(window, cx));
        cx.simulate_input("status");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(text_of(&editor, cx), "---\nstatus:\n---\nBody");
        // Its value field has focus.
        cx.simulate_input("draft");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(text_of(&editor, cx), "---\nstatus: draft\n---\nBody");
        click(&editor, cx, |panel, _, cx| panel.click_remove("status", cx));
        assert_eq!(text_of(&editor, cx), "---\n---\nBody");
    }

    #[gpui::test]
    fn committing_a_field_unchanged_rewrites_nothing(cx: &mut TestAppContext) {
        // Spaces only quotes keep: what the field shows isn't what's written.
        let note = "---\na: \"  x \"\n---\nBody";
        let (editor, cx, _dir) = open(cx, "Note.md", note);
        focus_value(&editor, "a", cx);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(text_of(&editor, cx), note);
    }

    #[gpui::test]
    fn a_checkbox_property_toggles(cx: &mut TestAppContext) {
        let (editor, cx, _dir) = open(cx, "Note.md", "---\ndone: false\n---\n");
        click(&editor, cx, |panel, _, cx| panel.click_checkbox("done", cx));
        assert_eq!(text_of(&editor, cx), "---\ndone: true\n---\n");
    }

    #[gpui::test]
    fn renaming_a_key_and_a_name_another_key_has_is_refused(cx: &mut TestAppContext) {
        let (editor, cx, _dir) = open(cx, "Note.md", "---\na: 1\nb: 2\n---\n");
        let rename = |key: &str, to: &str, cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let panel = editor.read(cx).properties.clone();
                panel.update(cx, |panel, cx| panel.focus_name(key, window, cx));
            });
            cx.simulate_keystrokes("ctrl-a");
            cx.simulate_input(to);
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
        };
        rename("a", "c", cx);
        assert_eq!(text_of(&editor, cx), "---\nc: 1\nb: 2\n---\n");
        rename("b", "c", cx);
        assert_eq!(text_of(&editor, cx), "---\nc: 1\nb: 2\n---\n");
        let names: Vec<String> = rows(&editor, cx).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["c", "b"], "the refused name isn't kept");
    }

    #[gpui::test]
    fn the_files_modified_time_shows_read_only(cx: &mut TestAppContext) {
        // 2026-01-15 12:00 UTC: the 15th in any time zone within 12 hours.
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_768_478_400);
        let (editor, cx, _dir) = open_with(
            cx,
            "Note.md",
            "Body",
            |_| {},
            |vault| {
                let file = std::fs::File::options()
                    .write(true)
                    .open(vault.root().join("Note.md"))
                    .unwrap();
                file.set_modified(time).unwrap();
            },
        );
        let shown = cx.update(|_, cx| editor.read(cx).properties.read(cx).modified());
        assert!(shown.unwrap().starts_with("2026-01-15 "));
    }

    #[gpui::test]
    fn a_type_chosen_in_the_panel_is_saved_and_other_notes_follow(cx: &mut TestAppContext) {
        use md_engine::PropertyType;
        let note = "---\ncount: \"5\"\nlabel: abc\n---\nBody";
        let (editor, cx, dir) = open_with(
            cx,
            "A.md",
            note,
            |_| {},
            |vault| {
                std::fs::write(vault.root().join("B.md"), "---\ncount: 7\nlabel: x\n---\n")
                    .unwrap();
            },
        );
        // Another note, open already.
        let other = cx.update(|window, cx| {
            let vault = editor.read(cx).vault.clone();
            let path = VaultPath::new("B.md").unwrap();
            let (note, text) = vault.open_note(&path).unwrap();
            cx.new(|cx| NoteEditor::new(vault, note, text, window, cx))
        });
        let kind = |e: &Entity<NoteEditor>, key: &str, cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                e.update(cx, |e, cx| e.sync_properties(window, cx));
                e.read(cx).properties.read(cx).kind(key)
            })
        };
        assert_eq!(kind(&editor, "count", cx), Some(PropertyType::Text));
        assert_eq!(kind(&other, "label", cx), Some(PropertyType::Text));
        // Text `"5"` as a number: converted, as nothing is lost.
        click(&editor, cx, |panel, _, cx| {
            panel.click_type("count", PropertyType::Number, cx)
        });
        assert_eq!(text_of(&editor, cx), note.replace("\"5\"", "5"));
        assert_eq!(kind(&editor, "count", cx), Some(PropertyType::Number));
        // `abc` can't be a number: left as written, flagged.
        click(&editor, cx, |panel, _, cx| {
            panel.click_type("label", PropertyType::Number, cx)
        });
        assert_eq!(text_of(&editor, cx), note.replace("\"5\"", "5"));
        assert_eq!(rows(&editor, cx)[1], ("label".into(), "abc".into(), true));
        assert_eq!(
            kind(&other, "label", cx),
            Some(PropertyType::Number),
            "the other note follows"
        );
        let saved = std::fs::read_to_string(dir.path().join(".vault/types.json")).unwrap();
        assert!(saved.contains("\"count\": \"number\""), "{saved}");
        // Typed into the flagged field, a number is written as one.
        focus_value(&editor, "label", cx);
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("12");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(text_of(&editor, cx).contains("label: 12\n"));
        assert!(!rows(&editor, cx)[1].2, "it fits now");
    }

    #[gpui::test]
    fn an_empty_property_set_to_checkbox_is_ticked_by_a_click(cx: &mut TestAppContext) {
        use md_engine::PropertyType;
        let (editor, cx, _dir) = open(cx, "A.md", "---\ndone:\n---\n");
        click(&editor, cx, |panel, _, cx| {
            panel.click_type("done", PropertyType::Checkbox, cx)
        });
        click(&editor, cx, |panel, _, cx| panel.click_checkbox("done", cx));
        assert_eq!(text_of(&editor, cx), "---\ndone: true\n---\n");
    }

    #[gpui::test]
    fn tags_and_aliases_keep_their_types(cx: &mut TestAppContext) {
        use md_engine::PropertyType;
        let note = "---\ntags: [a]\naliases: [b]\n---\n";
        let (editor, cx, _dir) = open_with(
            cx,
            "A.md",
            note,
            |_| {},
            |vault| {
                let a = VaultPath::new("A.md").unwrap();
                vault
                    .set_property_type(&a, "tags", PropertyType::Text)
                    .unwrap();
                vault
                    .set_property_type(&a, "aliases", PropertyType::Number)
                    .unwrap();
            },
        );
        let panel = cx.update(|_, cx| editor.read(cx).properties.clone());
        let kinds =
            cx.update(|_, cx| (panel.read(cx).kind("tags"), panel.read(cx).kind("aliases")));
        assert_eq!(
            kinds,
            (Some(PropertyType::Tags), Some(PropertyType::Aliases))
        );
    }
}
