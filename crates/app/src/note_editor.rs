//! One open note: the Split view (Kit's Editor beside the rendered preview,
//! or just the Editor in Source mode for `.txt`), with autosave.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use md_engine::{Format, TextEdit, format_edit, word_count};
use orbit_core::{Error, Vault, VaultPath, is_plain_text_note};

use crate::preview::{PreviewContext, preview};

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

const KEY_CONTEXT: &str = "NoteEditor";

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
    /// The edits are still in memory; the next save retries.
    failure: Option<SharedString>,
    /// Shared with writes on background threads, which run one at a time.
    disk: Arc<Mutex<Disk>>,
}

/// What a write needs to know when its turn comes, rather than when it was
/// queued: the note may have moved, been deleted, or been saved newer.
struct Disk {
    /// `None` once the note is deleted: nothing more is written.
    path: Option<VaultPath>,
    /// The newest revision on disk; an older write is skipped.
    written: u64,
}

impl Saves {
    fn new(path: VaultPath) -> Self {
        Saves {
            revision: 0,
            saved: 0,
            writing: false,
            again: false,
            failure: None,
            disk: Arc::new(Mutex::new(Disk {
                path: Some(path),
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

    /// Records how saving `revision` went. A result older than one already
    /// recorded changes nothing.
    fn finished(&mut self, revision: u64, result: Result<(), Error>) {
        if revision < self.saved {
            return;
        }
        match result {
            Ok(()) => {
                self.saved = revision;
                self.failure = None;
            }
            Err(e) => self.failure = Some(e.to_string().into()),
        }
    }
}

pub struct NoteEditor {
    vault: Arc<Vault>,
    path: VaultPath,
    mode: Mode,
    source: Entity<EditorState>,
    preview: PreviewContext,
    /// The source as of the last edit, for the preview.
    text: SharedString,
    words: usize,
    saves: Saves,
    autosave: Option<Task<()>>,
    _subscription: Subscription,
}

impl NoteEditor {
    pub fn new(
        vault: Arc<Vault>,
        path: VaultPath,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mode = Mode::of(&path);
        let source = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(match mode {
                    Mode::Split => "markdown",
                    Mode::Source => "plaintext",
                })
                .line_number(false)
                .soft_wrap(true)
                .default_value(text.clone())
        });
        let subscription = cx.subscribe(&source, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.edited(cx);
            }
        });
        NoteEditor {
            preview: PreviewContext::new(vault.clone(), path.clone()),
            saves: Saves::new(path.clone()),
            vault,
            path,
            mode,
            source,
            words: word_count(&text),
            text: text.into(),
            autosave: None,
            _subscription: subscription,
        }
    }

    pub fn path(&self) -> &VaultPath {
        &self.path
    }

    /// Follows the note after a rename or move; writes still queued go to
    /// the new path. A rename can change the extension, and with it the mode.
    pub fn set_path(&mut self, path: VaultPath, cx: &mut Context<Self>) {
        self.saves.disk().path = Some(path.clone());
        self.preview = PreviewContext::new(self.vault.clone(), path.clone());
        self.mode = Mode::of(&path);
        self.path = path;
        cx.notify();
    }

    /// After the note is deleted: writes still queued are dropped, so none
    /// can bring it back.
    pub fn forget_disk(&mut self) {
        self.autosave = None;
        self.saves.disk().path = None;
    }

    pub fn is_dirty(&self) -> bool {
        self.saves.is_dirty()
    }

    pub fn has_failed(&self) -> bool {
        self.saves.failure.is_some()
    }

    /// The status bar's left side: Saved, Unsaved, Saving… or the failure.
    pub fn status_text(&self) -> SharedString {
        match &self.saves.failure {
            Some(error) => format!("Save failed: {error}").into(),
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

    /// Moves keyboard focus to the source pane.
    pub fn focus(entity: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let handle = entity.read(cx).source.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        self.text = self.source.read(cx).value();
        self.words = word_count(&self.text);
        self.saves.revision += 1;
        self.autosave = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            this.update(cx, |this, cx| this.save(cx)).ok();
        }));
        cx.notify();
    }

    /// Saves in the background, one write at a time.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.autosave = None;
        if self.saves.writing {
            self.saves.again = true;
            return;
        }
        if !self.is_dirty() && !self.has_failed() {
            return;
        }
        let write = self.writer(cx);
        self.saves.writing = true;
        cx.notify();
        let write = cx.background_spawn(async move { write() });
        cx.spawn(async move |this, cx| {
            let (revision, result) = write.await;
            this.update(cx, |this, cx| {
                this.saves.writing = false;
                this.saves.finished(revision, result);
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
        if !self.is_dirty() && !self.has_failed() {
            return Ok(());
        }
        let (revision, result) = self.writer(cx)();
        self.saves.finished(revision, result);
        cx.notify();
        match &self.saves.failure {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    /// A write of the current text, serialised with every other write of
    /// this note, returning the revision it saved. When its turn comes, it
    /// writes to the note's path then, and not at all if a newer revision
    /// is already on disk or the note was deleted.
    fn writer(&self, cx: &App) -> impl FnOnce() -> (u64, Result<(), Error>) + Send + 'static {
        let (vault, disk) = (self.vault.clone(), self.saves.disk.clone());
        let (text, revision) = (self.source.read(cx).value(), self.saves.revision);
        move || {
            let mut disk = disk.lock().unwrap_or_else(|e| e.into_inner());
            let Some(path) = disk.path.clone().filter(|_| revision > disk.written) else {
                return (revision, Ok(()));
            };
            let result = vault.save_note(&path, &text);
            if result.is_ok() {
                disk.written = revision;
            }
            (revision, result)
        }
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
        let border = cx.theme().border;
        let editor = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(Editor::new(&self.source).h_full().bordered(false));
        h_flex()
            .key_context(KEY_CONTEXT)
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
            .size_full()
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
                            self.text.clone(),
                            &self.preview,
                            window,
                            cx,
                        )),
                )
            })
    }
}
