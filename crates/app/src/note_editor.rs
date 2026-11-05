//! One open note: the Split view (the text editor beside the rendered
//! preview), Source mode (just the text editor, always for `.txt`), or,
//! with the development setting on, Live Preview; with autosave. The text
//! editor is reached through `text_editor`.

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
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use md_engine::{Format, Heading, format_edit, headings, word_count};
use orbit_core::{
    DiskCheck, OpenNote, Trouble, Vault, VaultPath, Zeroizing, is_large_note, is_plain_text_note,
};

use crate::appearance::AppSettings;
use crate::clipboard_guard::{self, Copying};
use crate::link_completion::LinkCompletions;
use crate::preview::{FollowLink, PreviewContext, PreviewPane};
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
        /// Ctrl+E: Live Preview or Source mode, with the development
        /// setting on.
        ToggleLivePreview
    ]
);

pub const KEY_CONTEXT: &str = "NoteEditor";

/// How long typing must pause before the note saves itself.
const AUTOSAVE_DELAY: Duration = Duration::from_millis(1500);
/// The pause in typing after which the preview, Outline and word count
/// catch up: redrawing the preview of a long note on every key is what
/// typing spent its time on (docs/research/performance.md).
const DERIVED_PAUSE: Duration = Duration::from_millis(150);
/// While typing goes on, they still catch up at least this often.
const DERIVED_AT_LEAST: Duration = Duration::from_secs(1);

pub fn init(cx: &mut App) {
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
    /// Markdown source beside the rendered preview.
    Split,
    /// The note formatted, its syntax showing where the caret is (#35).
    LivePreview,
    /// Source only: always for `.txt` notes, and very large notes until
    /// the preview is asked for.
    Source,
}

impl Mode {
    /// The mode a note opens in: Source when it's plain text or too large
    /// for anything else.
    fn of(source_only: bool) -> Self {
        if source_only {
            Mode::Source
        } else {
            Mode::Split
        }
    }

    /// The editor that shows the text in this mode.
    fn shown(self) -> Shown {
        match self {
            Mode::LivePreview => Shown::LivePreview,
            Mode::Split | Mode::Source => Shown::Source,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Split => "Split view",
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
    /// A link was clicked in the preview: its target, `Note#Heading`.
    FollowLink(String),
    /// "Create … as a new note" was chosen for a link being typed: the
    /// link, whose note is wanted now.
    CreateLinkedNote(String),
}

impl EventEmitter<NoteEditorEvent> for NoteEditor {}

pub struct NoteEditor {
    vault: Arc<Vault>,
    path: VaultPath,
    mode: Mode,
    source: TextEditor,
    preview: PreviewContext,
    /// The preview beside the source, drawn only when its text changes.
    pane: Entity<PreviewPane>,
    /// Tells the window a link was clicked; kept for a new path's preview.
    follow: FollowLink,
    /// Wikilink autocomplete, which links from this note.
    links: Rc<LinkCompletions>,
    /// The source as of the last edit.
    text: SharedString,
    /// What the preview renders: the source with its links rewritten.
    shown: SharedString,
    words: usize,
    /// The source's headings, for the Outline.
    outline: Rc<[Heading]>,
    saves: Saves,
    /// Text from disk for the editor to show, on the next frame.
    reload: Option<String>,
    /// Shown with a failed save: where Save elsewhere put a copy.
    saved_elsewhere: Option<SharedString>,
    autosave: Option<Task<()>>,
    /// The preview, Outline and word count catching up with typing, and
    /// since when they've been behind.
    catching_up: Option<(Task<()>, std::time::Instant)>,
    /// The scroll restored from the session, until the editor is laid out.
    restored_scroll: Option<f32>,
    /// The note's size, while it's opened as source for being very large.
    large: Option<usize>,
    /// While a Copy or Cut agreed to in a protected note runs: it goes
    /// ahead.
    copy_agreed: bool,
    /// Held by the note's whole view, so the protected preview's selection
    /// goes when focus leaves the note (else Copy elsewhere would take it).
    focus: FocusHandle,
    _focus_out: Subscription,
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
        let subscription = source.on_edit(cx, |this, cx| this.edited(cx));
        let this = cx.entity().downgrade();
        let follow: FollowLink = Arc::new(move |target, _, cx| {
            this.update(cx, |_, cx| {
                cx.emit(NoteEditorEvent::FollowLink(target.to_string()))
            })
            .ok();
        });
        let preview = PreviewContext::new(vault.clone(), path.clone(), follow.clone());
        let focus = cx.focus_handle();
        let focus_out = cx.on_focus_out(&focus, window, |this, _, window, cx| {
            if this.is_protected() {
                TextSelection::clear(window, cx);
            }
        });
        let shown: SharedString = preview.rewrite_links(&text).into();
        let pane = cx.new(|_| PreviewPane::new(preview_id(&path), shown.clone(), preview.clone()));
        NoteEditor {
            preview,
            pane,
            follow,
            links,
            shown,
            saves: Saves::new(note),
            vault,
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
            _subscription: subscription,
        }
    }

    /// Copy or Cut in a protected note, before the editor or preview has
    /// it: asked about first, then cleared from the clipboard later
    /// (clipboard_guard). In the editor, once agreed, the editor copies as
    /// it would have (every cursor's selection); even with nothing
    /// selected it asks, as other cursors may have some. The preview's
    /// selection is taken now, since a dialog clears it.
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
        // The preview has nothing to cut.
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
        self.preview = PreviewContext::new(self.vault.clone(), path.clone(), self.follow.clone());
        self.path = path;
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

    /// Shows the note in `mode`: the editor's highlighting, the Outline and
    /// the preview follow, and the editor shown does on the next frame.
    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.source
            .set_language(language(self.is_source_only()), cx);
        self.mode = mode;
        self.outline = outline_of(self.is_source_only(), &self.text);
        self.links_changed(cx);
    }

    /// Ctrl+E, with the development setting on: Live Preview, or back to
    /// Source mode. A note shown only as source stays so.
    fn toggle_live_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !AppSettings::get(cx).live_preview_in_development || self.is_source_only() {
            return;
        }
        let mode = match self.mode {
            Mode::LivePreview => Mode::Source,
            Mode::Split | Mode::Source => Mode::LivePreview,
        };
        self.set_mode(mode, cx);
        self.source.show(mode.shown(), window, cx);
        self.source.focus_handle(cx).focus(window, cx);
    }

    /// Notes were added, removed or renamed: links may resolve differently.
    pub fn links_changed(&mut self, cx: &mut Context<Self>) {
        if self.mode == Mode::Split {
            self.shown = self.preview.rewrite_links(&self.text).into();
            let (id, shown) = (preview_id(&self.path), self.shown.clone());
            let context = &self.preview;
            self.pane
                .update(cx, |pane, cx| pane.show(id, shown, context, cx));
        }
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
        NoteEditor::focus(&cx.entity(), window, cx);
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

    /// How far the source is scrolled, in pixels from the top.
    pub fn scroll(&self, cx: &App) -> f32 {
        // Before its first layout (a restored tab never shown), the editor
        // hasn't taken the scroll it was given yet.
        self.source
            .scroll(cx)
            .or(self.restored_scroll)
            .unwrap_or(0.)
    }

    /// Scrolls the source, from the next layout on.
    pub fn set_scroll(&mut self, scroll: f32, cx: &mut Context<Self>) {
        self.restored_scroll = Some(scroll);
        self.source.set_scroll(scroll, cx);
    }

    /// Moves keyboard focus to the source pane.
    pub fn focus(entity: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let handle = entity.read(cx).source.focus_handle(cx);
        handle.focus(window, cx);
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        let _span = tracing::trace_span!("editor.edited").entered();
        self.text = self.source.text(cx);
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

    /// The preview, Outline and word count, brought up to date with the
    /// source.
    fn catch_up(&mut self, cx: &mut Context<Self>) {
        self.catching_up = None;
        self.words = word_count(&self.text);
        self.outline = outline_of(self.is_source_only(), &self.text);
        self.links_changed(cx);
    }

    /// Brings the preview, Outline and word count up to date once typing
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

    /// A very large note, opened as source: says so, with the preview a
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
                        .on_click(cx.listener(|this, _, _, cx| this.show_preview_anyway(cx))),
                )
                .into_any_element(),
        )
    }

    /// Shows a very large note's preview after all.
    fn show_preview_anyway(&mut self, cx: &mut Context<Self>) {
        self.large = None;
        self.set_mode(Mode::of(self.is_source_only()), cx);
    }

    fn apply_format(&mut self, kind: Format, window: &mut Window, cx: &mut Context<Self>) {
        let edit = format_edit(&self.source.text(cx), self.source.selection(cx), kind);
        self.source.apply(edit, window, cx);
    }
}

impl Render for NoteEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(text) = self.reload.take() {
            // Loading emits no change event, so this isn't an edit.
            self.source.load(text, window, cx);
        }
        // The development setting turned off: back to the mode it opens in.
        if self.mode == Mode::LivePreview && !AppSettings::get(cx).live_preview_in_development {
            self.set_mode(Mode::of(self.is_source_only()), cx);
        }
        if self.source.shown(cx) != self.mode.shown() {
            self.source.show(self.mode.shown(), window, cx);
        }
        let border = cx.theme().border;
        let editor = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.source.element(cx));
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
            .child(editor)
            .when(self.mode == Mode::Split, |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .border_l_1()
                        .border_color(border)
                        .child(
                            self.pane
                                .clone()
                                .cached(StyleRefinement::default().size_full()),
                        ),
                )
            });
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

/// The preview's element id: per note, so another note's preview doesn't
/// inherit its state.
fn preview_id(path: &VaultPath) -> SharedString {
    format!("preview-{path}").into()
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
