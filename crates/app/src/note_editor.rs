//! One open note: the Split view (Kit's Editor beside the rendered preview,
//! or just the Editor in Source mode for `.txt`), with autosave.

use std::{
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use md_engine::{Format, Heading, TextEdit, format_edit, headings, word_count};
use orbit_core::{DiskCheck, OpenNote, Trouble, Vault, VaultPath, is_plain_text_note};

use crate::link_completion::LinkCompletions;
use crate::preview::{FollowLink, PreviewContext, preview};

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
        InsertLink
    ]
);

pub const KEY_CONTEXT: &str = "NoteEditor";

/// How long typing must pause before the note saves itself.
const AUTOSAVE_DELAY: Duration = Duration::from_millis(1500);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-s", Save, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-b", ToggleBold, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-i", ToggleItalic, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-k", InsertLink, Some(KEY_CONTEXT)),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Markdown source beside the rendered preview.
    Split,
    /// Source only: `.txt` notes.
    Source,
}

impl Mode {
    fn of(path: &VaultPath) -> Self {
        if is_plain_text_note(path) {
            Mode::Source
        } else {
            Mode::Split
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Split => "Split view",
            Mode::Source => "Source mode",
        }
    }
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
}

/// What a write needs to know when its turn comes, rather than when it was
/// queued: the note may have moved, been deleted, or been saved newer.
struct Disk {
    /// `None` once Orbit deleted the note: nothing more is written.
    note: Option<OpenNote>,
    /// The newest revision on disk; an older write is skipped.
    written: u64,
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
        }
    }

    fn disk(&self) -> std::sync::MutexGuard<'_, Disk> {
        self.disk.lock().unwrap_or_else(|e| e.into_inner())
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
    source: Entity<EditorState>,
    preview: PreviewContext,
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
    /// The scroll restored from the session, until the editor is laid out.
    restored_scroll: Option<f32>,
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
        let mode = Mode::of(&path);
        let links = Rc::new(LinkCompletions::new(vault.clone(), path.clone()));
        let source = cx.new(|cx| {
            let mut source = EditorState::new(window, cx)
                .language(match mode {
                    Mode::Split => "markdown",
                    Mode::Source => "plaintext",
                })
                .line_number(false)
                .soft_wrap(true)
                .default_value(text.clone());
            source.lsp_mut().completion_provider = Some(links.clone());
            source
                .lsp_mut()
                .set_completion_highlighter(LinkCompletions::highlighter());
            source
        });
        let subscription = cx.subscribe(&source, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.edited(cx);
            }
        });
        let this = cx.entity().downgrade();
        let follow: FollowLink = Arc::new(move |target, _, cx| {
            this.update(cx, |_, cx| {
                cx.emit(NoteEditorEvent::FollowLink(target.to_string()))
            })
            .ok();
        });
        let preview = PreviewContext::new(vault.clone(), path.clone(), follow.clone());
        let shown = preview.rewrite_links(&text).into();
        NoteEditor {
            preview,
            follow,
            links,
            shown,
            saves: Saves::new(note),
            vault,
            path,
            mode,
            source,
            words: word_count(&text),
            outline: outline_of(mode, &text),
            text: text.into(),
            reload: None,
            saved_elsewhere: None,
            autosave: None,
            restored_scroll: None,
            _subscription: subscription,
        }
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
        self.links.set_note(path.clone());
        self.preview = PreviewContext::new(self.vault.clone(), path.clone(), self.follow.clone());
        self.mode = Mode::of(&path);
        self.outline = outline_of(self.mode, &self.text);
        self.path = path;
        self.links_changed(cx);
    }

    /// Notes were added, removed or renamed: links may resolve differently.
    pub fn links_changed(&mut self, cx: &mut Context<Self>) {
        if self.mode == Mode::Split {
            self.shown = self.preview.rewrite_links(&self.text).into();
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
        self.source.update(cx, |source, cx| {
            source.set_cursor_position(Position { line, character: 0 }, window, cx)
        });
        NoteEditor::focus(&cx.entity(), window, cx);
    }

    /// The note's headings as of the last edit; none in Source mode.
    pub fn outline(&self) -> Rc<[Heading]> {
        self.outline.clone()
    }

    /// After Orbit deletes the note: writes still queued are dropped, so
    /// none can bring it back.
    pub fn forget_disk(&mut self) {
        self.autosave = None;
        self.saves.disk().note = None;
    }

    pub fn is_dirty(&self) -> bool {
        self.saves.is_dirty()
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
        let caret = self.source.read(cx).cursor_position();
        let words = self.words;
        format!(
            "{words} word{} · Ln {}, Col {} · {}",
            if words == 1 { "" } else { "s" },
            caret.line + 1,
            caret.character + 1,
            self.mode.label()
        )
        .into()
    }

    /// How far the source is scrolled, in pixels from the top.
    pub fn scroll(&self, cx: &App) -> f32 {
        let source = self.source.read(cx);
        // Before its first layout (a restored tab never shown), the editor
        // hasn't taken the scroll it was given yet.
        match (source.line_height(), self.restored_scroll) {
            (None, Some(scroll)) => scroll,
            // Offsets grow negative downwards; `+ 0.` turns -0 into 0.
            _ => f32::max(0., -f32::from(source.scroll_offset().y)) + 0.,
        }
    }

    /// Scrolls the source, from the next layout on.
    pub fn set_scroll(&mut self, scroll: f32, cx: &mut Context<Self>) {
        self.restored_scroll = Some(scroll);
        self.source.update(cx, |source, cx| {
            source.set_scroll_offset(point(px(0.), px(-scroll)), cx)
        });
    }

    /// Moves keyboard focus to the source pane.
    pub fn focus(entity: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let handle = entity.read(cx).source.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        self.set_text(self.source.read(cx).value(), cx);
        let caret = self.source.read(cx).cursor();
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

    /// The source as of now, and everything derived from it.
    fn set_text(&mut self, text: SharedString, cx: &mut Context<Self>) {
        self.words = word_count(&text);
        self.outline = outline_of(self.mode, &text);
        self.text = text;
        self.links_changed(cx);
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
        let (text, revision) = (self.source.read(cx).value(), self.saves.revision);
        move || {
            let mut disk = disk.lock().unwrap_or_else(|e| e.into_inner());
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
        let text = self.source.read(cx).value();
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
        let text = self.source.read(cx).value();
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

    fn apply_format(&mut self, kind: Format, window: &mut Window, cx: &mut Context<Self>) {
        self.source.update(cx, |source, cx| {
            let text = source.value();
            let TextEdit {
                range,
                text: replacement,
                selection,
            } = format_edit(&text, source.selected_range(), kind);
            source.set_selected_range(range, cx);
            source.replace(replacement, window, cx);
            source.set_selected_range(selection, cx);
        });
    }
}

impl Render for NoteEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(text) = self.reload.take() {
            // `set_value` emits no change event, so this isn't an edit.
            self.source
                .update(cx, |source, cx| source.set_value(text, window, cx));
        }
        let border = cx.theme().border;
        let editor = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(Editor::new(&self.source).h_full().bordered(false));
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
            .child(editor)
            .when(self.mode == Mode::Split, |this| {
                this.child(
                    div()
                        .id("preview")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .border_l_1()
                        .border_color(border)
                        .px_6()
                        .py_4()
                        .child(preview(
                            SharedString::from(format!("preview-{}", self.path)),
                            self.shown.clone(),
                            &self.preview,
                            window,
                            cx,
                        )),
                )
            });
        v_flex()
            .key_context(KEY_CONTEXT)
            .size_full()
            .children(banner)
            .child(body)
    }
}

/// A note's Outline: its headings, unless it's plain text.
fn outline_of(mode: Mode, text: &str) -> Rc<[Heading]> {
    match mode {
        Mode::Split => headings(text).into(),
        Mode::Source => Rc::new([]),
    }
}
