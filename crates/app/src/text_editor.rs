//! The note editor's text view behind one interface: the only place Kit's
//! editor is reached. Today that's Kit's source editor (Source mode, and the
//! source half of Split view); Live Preview is to plug in behind the same
//! interface (#35).
//!
//! Offsets are UTF-8 bytes into the note's text. Lines and columns, for the
//! status bar and go to line, start at zero.

use std::rc::Rc;

use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position};
use gpui_kit::*;
use md_engine::TextEdit;

use crate::clipboard_guard::Copying;
use crate::link_completion::LinkCompletions;

/// A note's text, shown for editing. Cheap to clone: clones are the same
/// editor.
#[derive(Clone)]
pub struct TextEditor {
    state: Entity<EditorState>,
}

impl TextEditor {
    /// An editor over `text`, highlighted as `language`, with wikilink
    /// autocomplete.
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
        TextEditor { state }
    }

    /// Calls `edited` after every edit the person makes. Loading text
    /// ([`Self::load`]) isn't an edit.
    pub fn on_edit<T: 'static>(
        &self,
        cx: &mut Context<T>,
        mut edited: impl FnMut(&mut T, &mut Context<T>) + 'static,
    ) -> Subscription {
        cx.subscribe(&self.state, move |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                edited(this, cx);
            }
        })
    }

    /// The text as it is now.
    pub fn text(&self, cx: &App) -> SharedString {
        self.state.read(cx).value()
    }

    /// Replaces the whole text with `text` from disk, without it counting
    /// as an edit.
    pub fn load(&self, text: String, window: &mut Window, cx: &mut App) {
        self.state
            .update(cx, |state, cx| state.set_value(text, window, cx));
    }

    /// Highlights the text as `language` from now on.
    pub fn set_language(&self, language: &'static str, cx: &mut App) {
        self.state
            .update(cx, |state, cx| state.set_highlighter(language, cx));
    }

    /// The caret's offset.
    pub fn caret(&self, cx: &App) -> usize {
        self.state.read(cx).cursor()
    }

    /// The caret's line and column.
    pub fn caret_line_column(&self, cx: &App) -> (u32, u32) {
        let at = self.state.read(cx).cursor_position();
        (at.line, at.character)
    }

    /// The selection as a range of offsets; empty for a caret.
    pub fn selection(&self, cx: &App) -> std::ops::Range<usize> {
        self.state.read(cx).selected_range()
    }

    /// Applies an edit made from the selection, as one edit: its range is
    /// replaced by its text, then its selection is selected.
    pub fn apply(&self, edit: TextEdit, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.set_selected_range(edit.range, cx);
            state.replace(edit.text, window, cx);
            state.set_selected_range(edit.selection, cx);
        });
    }

    /// Puts the caret at the start of `line`, scrolled into view.
    pub fn go_to_line(&self, line: u32, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.set_cursor_position(Position { line, character: 0 }, window, cx)
        });
    }

    /// How far the text is scrolled, in pixels from the top; `None` before
    /// it's first laid out, when it hasn't taken a scroll it was given yet.
    pub fn scroll(&self, cx: &App) -> Option<f32> {
        let state = self.state.read(cx);
        state.line_height()?;
        // Offsets grow negative downwards; `+ 0.` turns -0 into 0.
        Some(f32::max(0., -f32::from(state.scroll_offset().y)) + 0.)
    }

    /// Scrolls the text, from the next layout on.
    pub fn set_scroll(&self, scroll: f32, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.set_scroll_offset(point(px(0.), px(-scroll)), cx)
        });
    }

    /// The handle that has focus while the text is being edited.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx)
    }

    /// Copies or cuts as the editor itself would (every cursor's
    /// selection), for the protected-note guard once it's agreed to.
    pub fn copy(&self, copying: Copying, window: &mut Window, cx: &mut App) {
        let focus = self.focus_handle(cx);
        focus.dispatch_action(copying.action().as_ref(), window, cx);
    }

    /// The editor, to place in the note's view.
    pub fn element(&self) -> impl IntoElement + use<> {
        Editor::new(&self.state).h_full().bordered(false)
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

    use super::TextEditor;
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
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.editor.element())
        }
    }

    /// Opens an editor over `text` in a headless window, focused.
    fn open<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
    ) -> (Entity<Host>, &'a mut VisualTestContext) {
        cx.update(gpui_kit::init);
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
}
