//! The note editor's text view behind one interface: the only place the
//! editors are reached. It's Kit's source editor (Source mode, and the
//! source half of Split view) or, once asked for, Live Preview (the
//! `editor` crate, #35); each keeps its own undo, and switching hands the
//! shown one the other's text.
//!
//! Offsets are UTF-8 bytes into the note's text. Lines and columns, for the
//! status bar and go to line, start at zero.

use std::rc::Rc;

use editor::{LivePreview, LivePreviewEvent};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position};
use gpui_kit::*;
use md_engine::TextEdit;

use crate::clipboard_guard::Copying;
use crate::link_completion::LinkCompletions;

/// Which editor shows the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// Kit's source editor.
    Source,
    /// Live Preview.
    LivePreview,
}

/// Live Preview, once it was first shown, and which editor shows now.
struct Views {
    shown: Shown,
    live: Option<Entity<LivePreview>>,
    _live_edits: Option<Subscription>,
}

/// Live Preview reported an edit (re-emitted for [`TextEditor::on_edit`]).
struct LiveEdited;

impl EventEmitter<LiveEdited> for Views {}

impl Views {
    /// Live Preview, if it's the one shown.
    fn live_shown(&self) -> Option<&Entity<LivePreview>> {
        self.live
            .as_ref()
            .filter(|_| self.shown == Shown::LivePreview)
    }
}

/// A note's text, shown for editing. Cheap to clone: clones are the same
/// editor.
#[derive(Clone)]
pub struct TextEditor {
    state: Entity<EditorState>,
    views: Entity<Views>,
}

impl TextEditor {
    /// An editor over `text`, highlighted as `language`, with wikilink
    /// autocomplete, showing Kit's source editor.
    pub fn new(
        text: &str,
        language: &'static str,
        links: Rc<LinkCompletions>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(language)
                .line_number(false)
                .soft_wrap(true)
                .default_value(text.to_string());
            state.lsp_mut().completion_provider = Some(links);
            state
                .lsp_mut()
                .set_completion_highlighter(LinkCompletions::highlighter());
            state
        });
        let views = cx.new(|_| Views {
            shown: Shown::Source,
            live: None,
            _live_edits: None,
        });
        TextEditor { state, views }
    }

    /// Which editor shows the text.
    pub fn shown(&self, cx: &App) -> Shown {
        self.views.read(cx).shown
    }

    /// Shows the text in `shown`, which takes what the other has: an edit
    /// made in the other comes back as one undo step.
    pub fn show(&self, shown: Shown, window: &mut Window, cx: &mut App) {
        if self.shown(cx) == shown {
            return;
        }
        let text = self.text(cx);
        match shown {
            Shown::LivePreview => {
                let live = self.views.read(cx).live.clone();
                match live {
                    Some(live) => live.update(cx, |live, cx| live.take_text(&text, cx)),
                    None => {
                        let live = cx.new(|cx| LivePreview::new(&text, cx));
                        self.views.update(cx, |views, cx| {
                            views._live_edits =
                                Some(cx.subscribe(&live, |_, _, _: &LivePreviewEvent, cx| {
                                    cx.emit(LiveEdited)
                                }));
                            views.live = Some(live);
                        });
                    }
                }
            }
            Shown::Source => {
                let old = self.state.read(cx).value();
                if old != text {
                    // One replacement of what differs, so Kit's undo has it as
                    // one step and the rest of its history stays.
                    let (range, new) = live_preview::differing(&old, &text);
                    self.state.update(cx, |state, cx| {
                        state.set_selected_range(range, cx);
                        state.replace(new, window, cx);
                    });
                }
            }
        }
        self.views.update(cx, |views, _| views.shown = shown);
    }

    /// Calls `edited` after every edit the person makes, in either editor.
    /// Loading text ([`Self::load`]) isn't an edit.
    pub fn on_edit<T: 'static>(
        &self,
        cx: &mut Context<T>,
        edited: impl FnMut(&mut T, &mut Context<T>) + 'static,
    ) -> Subscription {
        let edited = Rc::new(std::cell::RefCell::new(edited));
        let in_source = edited.clone();
        let source = cx.subscribe(&self.state, move |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                (in_source.borrow_mut())(this, cx);
            }
        });
        let live = cx.subscribe(&self.views, move |this, _, _: &LiveEdited, cx| {
            (edited.borrow_mut())(this, cx);
        });
        Subscription::join(source, live)
    }

    /// The text as it is now.
    pub fn text(&self, cx: &App) -> SharedString {
        match self.views.read(cx).live_shown() {
            Some(live) => live.read(cx).text().to_string().into(),
            None => self.state.read(cx).value(),
        }
    }

    /// Replaces the whole text with `text` from disk, in both editors,
    /// without it counting as an edit.
    pub fn load(&self, text: String, window: &mut Window, cx: &mut App) {
        if let Some(live) = self.views.read(cx).live.clone() {
            live.update(cx, |live, cx| live.load(&text, cx));
        }
        self.state
            .update(cx, |state, cx| state.set_value(text, window, cx));
    }

    /// Highlights the source as `language` from now on.
    pub fn set_language(&self, language: &'static str, cx: &mut App) {
        self.state
            .update(cx, |state, cx| state.set_highlighter(language, cx));
    }

    /// The caret's offset.
    pub fn caret(&self, cx: &App) -> usize {
        match self.views.read(cx).live_shown() {
            Some(live) => live.read(cx).caret(),
            None => self.state.read(cx).cursor(),
        }
    }

    /// The caret's line and column.
    pub fn caret_line_column(&self, cx: &App) -> (u32, u32) {
        match self.views.read(cx).live_shown() {
            Some(live) => live.read(cx).line_column(),
            None => {
                let at = self.state.read(cx).cursor_position();
                (at.line, at.character)
            }
        }
    }

    /// The selection as a range of offsets; empty for a caret.
    pub fn selection(&self, cx: &App) -> std::ops::Range<usize> {
        match self.views.read(cx).live_shown() {
            Some(live) => live.read(cx).selection(),
            None => self.state.read(cx).selected_range(),
        }
    }

    /// Applies an edit made from the selection, as one edit: its range is
    /// replaced by its text, then its selection is selected.
    pub fn apply(&self, edit: TextEdit, window: &mut Window, cx: &mut App) {
        if let Some(live) = self.views.read(cx).live_shown().cloned() {
            return live.update(cx, |live, cx| {
                live.replace(edit.range, &edit.text, edit.selection, cx)
            });
        }
        self.state.update(cx, |state, cx| {
            state.set_selected_range(edit.range, cx);
            state.replace(edit.text, window, cx);
            state.set_selected_range(edit.selection, cx);
        });
    }

    /// Puts the caret at the start of `line`, scrolled into view.
    pub fn go_to_line(&self, line: u32, window: &mut Window, cx: &mut App) {
        if let Some(live) = self.views.read(cx).live_shown().cloned() {
            return live.update(cx, |live, cx| live.go_to_line(line, cx));
        }
        self.state.update(cx, |state, cx| {
            state.set_cursor_position(Position { line, character: 0 }, window, cx)
        });
    }

    /// How far the source editor is scrolled, in pixels from the top;
    /// `None` before it's first laid out, when it hasn't taken a scroll it
    /// was given yet, or while Live Preview shows (its scroll isn't kept
    /// yet, #54).
    pub fn scroll(&self, cx: &App) -> Option<f32> {
        if self.views.read(cx).live_shown().is_some() {
            return None;
        }
        let state = self.state.read(cx);
        state.line_height()?;
        // Offsets grow negative downwards; `+ 0.` turns -0 into 0.
        Some(f32::max(0., -f32::from(state.scroll_offset().y)) + 0.)
    }

    /// Scrolls the source editor, from the next layout on.
    pub fn set_scroll(&self, scroll: f32, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.set_scroll_offset(point(px(0.), px(-scroll)), cx)
        });
    }

    /// The handle that has focus while the text is being edited.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.views.read(cx).live_shown() {
            Some(live) => live.read(cx).focus_handle().clone(),
            None => self.state.read(cx).focus_handle(cx),
        }
    }

    /// Copies or cuts as the shown editor itself would, for the
    /// protected-note guard once it's agreed to.
    pub fn copy(&self, copying: Copying, window: &mut Window, cx: &mut App) {
        let focus = self.focus_handle(cx);
        focus.dispatch_action(copying.action().as_ref(), window, cx);
    }

    /// Forgets Live Preview's undo history: the note's protected folder is
    /// locking.
    pub fn forget_history(&self, cx: &mut App) {
        if let Some(live) = self.views.read(cx).live.clone() {
            live.update(cx, |live, _| live.forget_history());
        }
    }

    /// The shown editor, to place in the note's view.
    pub fn element(&self, cx: &App) -> AnyElement {
        match self.views.read(cx).live_shown() {
            Some(live) => live.clone().into_any_element(),
            None => Editor::new(&self.state)
                .h_full()
                .bordered(false)
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{rc::Rc, sync::Arc};

    // Named, not globbed: Kit's glob brings gpui's `test` macro, which then
    // shadows `#[test]` inside `#[gpui::test]`.
    use gpui::{
        Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Subscription,
        TestAppContext, VisualTestContext, Window, div,
    };
    use md_engine::{Format, format_edit};
    use orbit_core::{Vault, VaultPath};

    use super::{Shown, TextEditor};
    use crate::link_completion::LinkCompletions;

    /// Holds an editor in a window and counts the edits it reports.
    struct Host {
        editor: TextEditor,
        edits: usize,
        _edits: Subscription,
        /// The vault's folder, removed with the host.
        _vault: tempfile::TempDir,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.editor.element(cx))
        }
    }

    /// Opens an editor over `text` in a headless window, focused.
    fn open<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
    ) -> (Entity<Host>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            editor::init(cx);
        });
        let dir = tempfile::tempdir().expect("a temporary vault");
        let vault = Arc::new(Vault::open(dir.path()).expect("the vault opens"));
        let links = Rc::new(LinkCompletions::new(
            vault,
            VaultPath::new("Note.md").unwrap(),
        ));
        let text = text.to_string();
        let (host, cx) = cx.add_window_view(|window, cx| {
            let editor = TextEditor::new(&text, "markdown", links, window, cx);
            let edits = editor.on_edit(cx, |host: &mut Host, _| host.edits += 1);
            Host {
                editor,
                edits: 0,
                _edits: edits,
                _vault: dir,
            }
        });
        cx.update(|window, cx| {
            let focus = host.read(cx).editor.focus_handle(cx);
            focus.focus(window, cx);
        });
        cx.run_until_parked();
        (host, cx)
    }

    /// The host's editor.
    fn editor(host: &Entity<Host>, cx: &mut VisualTestContext) -> TextEditor {
        host.read_with(cx, |host, _| host.editor.clone())
    }

    #[gpui::test]
    fn loading_text_isnt_an_edit_but_typing_is(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "one");
        let editor = editor(&host, cx);
        cx.update(|window, cx| editor.load("two".into(), window, cx));
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "two");
        assert_eq!(host.read_with(cx, |h, _| h.edits), 0);
        cx.simulate_input("x");
        cx.run_until_parked();
        assert_eq!(host.read_with(cx, |h, _| h.edits), 1);
        let text = cx.update(|_, cx| editor.text(cx));
        assert!(text == "xtwo" || text == "twox", "{text}");
    }

    #[gpui::test]
    fn a_format_edit_replaces_the_selection_and_selects_its_result(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "make this bold");
        let editor = editor(&host, cx);
        cx.update(|window, cx| editor.go_to_line(0, window, cx));
        // Select "this" with the keyboard: right ×5, then shift-right ×4.
        cx.simulate_keystrokes("right right right right right");
        cx.simulate_keystrokes("shift-right shift-right shift-right shift-right");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.selection(cx)), 5..9);
        cx.update(|window, cx| {
            let edit = format_edit(&editor.text(cx), editor.selection(cx), Format::Bold);
            editor.apply(edit, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "make **this** bold");
        assert_eq!(cx.update(|_, cx| editor.selection(cx)), 7..11);
        assert_eq!(host.read_with(cx, |h, _| h.edits), 1, "one edit");
    }

    #[gpui::test]
    fn go_to_line_puts_the_caret_at_its_start(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "first\nsecond\nthird line");
        let editor = editor(&host, cx);
        cx.update(|window, cx| editor.go_to_line(2, window, cx));
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), "first\nsecond\n".len());
        assert_eq!(cx.update(|_, cx| editor.caret_line_column(cx)), (2, 0));
        cx.simulate_keystrokes("right right right");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.caret_line_column(cx)), (2, 3));
    }

    /// The host's editor, showing Live Preview.
    fn live(host: &Entity<Host>, cx: &mut VisualTestContext) -> TextEditor {
        let editor = editor(host, cx);
        cx.update(|window, cx| {
            editor.show(Shown::LivePreview, window, cx);
            editor.focus_handle(cx).focus(window, cx);
        });
        cx.run_until_parked();
        editor
    }

    #[gpui::test]
    fn typing_in_live_preview_edits_the_note(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "Some **bold** text");
        let editor = live(&host, cx);
        cx.simulate_keystrokes("end");
        cx.simulate_input("!");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "Some **bold** text!");
        assert_eq!(host.read_with(cx, |h, _| h.edits), 1);
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), 19);
        assert_eq!(cx.update(|_, cx| editor.caret_line_column(cx)), (0, 19));
    }

    #[gpui::test]
    fn switching_views_keeps_the_text_and_each_view_s_undo(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "one");
        let editor = live(&host, cx);
        cx.simulate_keystrokes("end");
        cx.simulate_input(" two");
        cx.run_until_parked();
        // To Source: it has the text typed in Live Preview.
        cx.update(|window, cx| {
            editor.show(Shown::Source, window, cx);
            editor.focus_handle(cx).focus(window, cx);
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "one two");
        cx.simulate_keystrokes("ctrl-end");
        cx.simulate_input("!");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "one two!");
        // Back to Live Preview: it has Source's edit, and undo takes it
        // back as one step, then the typing before the switch.
        let editor = live(&host, cx);
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "one two!");
        cx.simulate_keystrokes("ctrl-z");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "one two");
        cx.simulate_keystrokes("ctrl-z");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "one");
    }

    #[gpui::test]
    fn a_format_edit_applies_in_live_preview_too(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "make this bold");
        let editor = live(&host, cx);
        cx.simulate_keystrokes("home ctrl-right ctrl-right ctrl-shift-left");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.selection(cx)), 5..9);
        cx.update(|window, cx| {
            let edit = format_edit(&editor.text(cx), editor.selection(cx), Format::Bold);
            editor.apply(edit, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "make **this** bold");
        assert_eq!(cx.update(|_, cx| editor.selection(cx)), 7..11);
        assert_eq!(host.read_with(cx, |h, _| h.edits), 1, "one edit");
    }

    #[gpui::test]
    fn loading_from_disk_reaches_the_hidden_view_too(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "old");
        let editor = live(&host, cx);
        cx.update(|window, cx| editor.load("new from disk".into(), window, cx));
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "new from disk");
        cx.update(|window, cx| editor.show(Shown::Source, window, cx));
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "new from disk");
        assert_eq!(host.read_with(cx, |h, _| h.edits), 0);
    }

    #[gpui::test]
    fn go_to_line_works_in_live_preview(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "# Head\n\nfirst\nsecond");
        let editor = live(&host, cx);
        cx.update(|window, cx| editor.go_to_line(3, window, cx));
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| editor.caret(cx)),
            "# Head\n\nfirst\n".len()
        );
        assert_eq!(cx.update(|_, cx| editor.caret_line_column(cx)), (3, 0));
    }

    #[gpui::test]
    fn a_list_shown_as_source_keeps_its_lines(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "- one\n- two\n\nend");
        let editor = live(&host, cx);
        cx.simulate_keystrokes("right right right");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.caret_line_column(cx)), (0, 3));
        // Down goes to the list's next line, laid out below the first.
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.caret_line_column(cx)), (1, 3));
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.caret_line_column(cx)).0, 3);
    }
}
