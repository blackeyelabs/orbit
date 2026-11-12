//! The note editor's text view behind one interface: the only place the
//! editors are reached. It's Live Preview (the `editor` crate, #35) or
//! Kit's source editor (Source mode); each keeps its own undo, and
//! switching hands the shown one the other's text.
//!
//! Offsets are UTF-8 bytes into the note's text. Lines and columns, for the
//! status bar and go to line, start at zero.

use std::rc::Rc;

use editor::Links as _;
use editor::{LivePreview, LivePreviewEvent};
use gpui_kit::component::input::{
    Editor, EditorState, Enter, IndentInline, InputEvent, OutdentInline, Position,
};
use gpui_kit::*;
use live_preview::Document;
use live_preview::conceal::Selection;
use md_engine::TextEdit;
use orbit_core::Scroll;

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

actions!(
    text_editor,
    [
        /// Enter in Source mode: continues or ends a list, else Kit's.
        SourceEnter,
        /// Tab in Source mode: nests list items, else Kit's.
        SourceTab,
        /// Shift+Tab in Source mode: unnests list items, else Kit's.
        SourceShiftTab
    ]
);

/// The key context around Kit's source editor.
const SOURCE_CONTEXT: &str = "SourceEditor";

/// Binds Enter, Tab and Shift+Tab in Source mode over Kit's own, so they
/// edit lists as Live Preview does. Call after Kit's `init`.
pub fn init(cx: &mut App) {
    let context = format!("{SOURCE_CONTEXT} > Input");
    let context = Some(context.as_str());
    cx.bind_keys([
        KeyBinding::new("enter", SourceEnter, context),
        KeyBinding::new("tab", SourceTab, context),
        KeyBinding::new("shift-tab", SourceShiftTab, context),
    ]);
}

/// A list key in Source mode.
#[derive(Clone, Copy)]
enum ListKey {
    Enter,
    Tab,
    ShiftTab,
}

impl ListKey {
    /// The list edit Live Preview makes for the key, made in `doc`; says
    /// whether there was one.
    fn edit(self, doc: &mut Document) -> bool {
        match self {
            ListKey::Enter => doc.continue_list(),
            ListKey::Tab => doc.indent(),
            ListKey::ShiftTab => doc.outdent(),
        }
    }

    /// What Kit's editor does with the key.
    fn kit_action(self) -> Box<dyn Action> {
        match self {
            ListKey::Enter => Box::new(Enter {
                secondary: false,
                shift: false,
            }),
            ListKey::Tab => Box::new(IndentInline),
            ListKey::ShiftTab => Box::new(OutdentInline),
        }
    }
}

/// Live Preview, once it was first shown, and which editor shows now.
struct Views {
    shown: Shown,
    live: Option<Entity<LivePreview>>,
    _live_events: Option<Subscription>,
    /// For Live Preview's links: which targets resolve.
    links: Rc<LinkCompletions>,
}

/// Live Preview's events are re-emitted, for [`TextEditor::on_edit`] and
/// [`TextEditor::on_follow_link`].
impl EventEmitter<LivePreviewEvent> for Views {}

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
            state.lsp_mut().completion_provider = Some(links.clone());
            state
                .lsp_mut()
                .set_completion_highlighter(LinkCompletions::highlighter());
            state
        });
        let views = cx.new(|_| Views {
            shown: Shown::Source,
            live: None,
            _live_events: None,
            links,
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
                        let links = self.views.read(cx).links.clone();
                        let live = cx.new(|cx| LivePreview::new(&text, links, cx));
                        self.views.update(cx, |views, cx| {
                            views._live_events =
                                Some(cx.subscribe(&live, |_, _, event: &LivePreviewEvent, cx| {
                                    cx.emit(event.clone())
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
        window: &mut Window,
        cx: &mut Context<T>,
        edited: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let edited = Rc::new(std::cell::RefCell::new(edited));
        let in_source = edited.clone();
        let source = cx.subscribe_in(
            &self.state,
            window,
            move |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    (in_source.borrow_mut())(this, window, cx);
                }
            },
        );
        let live = cx.subscribe_in(
            &self.views,
            window,
            move |this, _, event: &LivePreviewEvent, window, cx| {
                if let LivePreviewEvent::Edited = event {
                    (edited.borrow_mut())(this, window, cx);
                }
            },
        );
        Subscription::join(source, live)
    }

    /// Calls `follow` with a link's target (`Note#Heading`) when one is
    /// clicked in Live Preview. (Kit's source editor follows none.)
    pub fn on_follow_link<T: 'static>(
        &self,
        cx: &mut Context<T>,
        mut follow: impl FnMut(&mut T, &str, &mut Context<T>) + 'static,
    ) -> Subscription {
        cx.subscribe(&self.views, move |this, _, event: &LivePreviewEvent, cx| {
            if let LivePreviewEvent::FollowLink(target) = event {
                follow(this, target, cx);
            }
        })
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

    /// Draws Live Preview again, as something it shows changed outside
    /// the text: which links resolve, the images it reads.
    pub fn redraw(&self, cx: &mut App) {
        self.views.read(cx).links.forget_images();
        if let Some(live) = self.views.read(cx).live.clone() {
            live.update(cx, |_, cx| cx.notify());
        }
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

    /// Where the shown editor is scrolled; `None` for the source editor
    /// before it's first laid out, when it hasn't taken a scroll it was
    /// given yet.
    pub fn scroll(&self, cx: &App) -> Option<Scroll> {
        if let Some(live) = self.views.read(cx).live_shown() {
            let (top, within) = live.read(cx).scroll_top();
            let within = f32::from(within);
            return Some(Scroll::LivePreview { top, within });
        }
        let state = self.state.read(cx);
        state.line_height()?;
        // Offsets grow negative downwards; `+ 0.` turns -0 into 0.
        let pixels = f32::max(0., -f32::from(state.scroll_offset().y)) + 0.;
        Some(Scroll::Source(pixels))
    }

    /// Scrolls the editor `scroll` came from: Live Preview, or the source
    /// editor from its next layout on. Live Preview's scroll is dropped
    /// if it was never shown (the note now opens as source only). The
    /// note shows in the view `scroll` came from first (see
    /// `NoteEditor::set_scroll`).
    pub fn set_scroll(&self, scroll: Scroll, cx: &mut App) {
        match scroll {
            Scroll::LivePreview { top, within } => {
                if let Some(live) = self.views.read(cx).live.clone() {
                    live.update(cx, |live, cx| live.scroll_to_top(top, px(within), cx));
                }
            }
            Scroll::Source(pixels) => self.state.update(cx, |state, cx| {
                state.set_scroll_offset(point(px(0.), px(-pixels)), cx)
            }),
        }
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
            None => {
                let on = |key: ListKey| {
                    let this = self.clone();
                    move |window: &mut Window, cx: &mut App| this.list_key(key, window, cx)
                };
                let (enter, tab, shift_tab) =
                    (on(ListKey::Enter), on(ListKey::Tab), on(ListKey::ShiftTab));
                div()
                    .size_full()
                    .key_context(SOURCE_CONTEXT)
                    .on_action(move |_: &SourceEnter, window, cx| enter(window, cx))
                    .on_action(move |_: &SourceTab, window, cx| tab(window, cx))
                    .on_action(move |_: &SourceShiftTab, window, cx| shift_tab(window, cx))
                    .child(Editor::new(&self.state).h_full().bordered(false))
                    .into_any_element()
            }
        }
    }

    /// A list key in Source mode: the list edit Live Preview makes for it,
    /// worked out by the same Document facade, as one edit; or, outside a
    /// list (or with a link being typed, whose suggestions take Enter and
    /// Tab), what Kit's editor does with the key.
    fn list_key(&self, key: ListKey, window: &mut Window, cx: &mut App) {
        let text = self.state.read(cx).value().to_string();
        let selection = self.state.read(cx).selected_range();
        let caret = self.state.read(cx).cursor();
        let suggesting =
            selection.is_empty() && self.views.read(cx).links.suggest(&text, caret).is_some();
        let mut doc = Document::new(&text);
        let anchor = if caret == selection.start {
            selection.end
        } else {
            selection.start
        };
        doc.set_selection(Selection {
            anchor,
            head: caret,
        });
        if suggesting || !key.edit(&mut doc) {
            return window.dispatch_action(key.kit_action(), cx);
        }
        let (range, new) = live_preview::differing(&text, doc.text());
        let edit = TextEdit {
            range,
            text: new.to_string(),
            selection: doc.selection(),
        };
        self.apply(edit, window, cx);
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
    use orbit_core::{Scroll, Vault, VaultPath};

    use super::{Shown, TextEditor};
    use crate::link_completion::LinkCompletions;

    /// Holds an editor in a window and counts the edits it reports.
    struct Host {
        editor: TextEditor,
        edits: usize,
        _edits: Subscription,
        /// The links Live Preview followed.
        followed: Vec<String>,
        _follows: Subscription,
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
        open_among(cx, text, &[])
    }

    /// As [`open`], in a vault that also holds the notes `others`.
    fn open_among<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
        others: &[&str],
    ) -> (Entity<Host>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            editor::init(cx);
            super::init(cx);
        });
        let dir = tempfile::tempdir().expect("a temporary vault");
        for name in others {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let vault = Arc::new(Vault::open(dir.path()).expect("the vault opens"));
        let links = Rc::new(LinkCompletions::new(
            vault,
            VaultPath::new("Note.md").unwrap(),
        ));
        let text = text.to_string();
        let (host, cx) = cx.add_window_view(|window, cx| {
            let editor = TextEditor::new(&text, "markdown", links, window, cx);
            let edits = editor.on_edit(window, cx, |host: &mut Host, _, _| host.edits += 1);
            let follows = editor.on_follow_link(cx, |host: &mut Host, target, _| {
                host.followed.push(target.to_string())
            });
            Host {
                editor,
                edits: 0,
                _edits: edits,
                followed: Vec::new(),
                _follows: follows,
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
    fn down_steps_through_a_list_item_by_item(cx: &mut TestAppContext) {
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

    #[gpui::test]
    fn a_click_on_a_link_in_live_preview_follows_it(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "[[Target#Part|go there]] or stay\n\nEnd");
        live(&host, cx);
        // Away from the link, its brackets are hidden.
        cx.simulate_keystrokes("ctrl-end");
        cx.run_until_parked();
        // The text starts past the editor's padding (24 px left, 16 top).
        cx.simulate_click(
            gpui::point(gpui::px(30.), gpui::px(26.)),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        assert_eq!(
            host.read_with(cx, |h, _| h.followed.clone()),
            ["Target#Part"]
        );
        let editor = editor(&host, cx);
        assert_eq!(
            cx.update(|_, cx| editor.caret(cx)),
            37,
            "the caret stays at the end"
        );
        // Beside it, past the line's end: the caret goes there.
        cx.simulate_click(
            gpui::point(gpui::px(600.), gpui::px(26.)),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        assert_eq!(host.read_with(cx, |h, _| h.followed.len()), 1);
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), 32);
        // Arrowing in reveals it instead: then a click there edits it.
        cx.simulate_keystrokes("ctrl-home");
        cx.simulate_click(
            gpui::point(gpui::px(30.), gpui::px(26.)),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        assert_eq!(host.read_with(cx, |h, _| h.followed.len()), 1);
    }

    #[gpui::test]
    fn a_click_on_a_task_s_checkbox_ticks_it(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "- [ ] buy milk\n\nEnd");
        live(&host, cx);
        let editor = editor(&host, cx);
        cx.simulate_keystrokes("ctrl-end");
        cx.run_until_parked();
        // The box comes first on the line, past the editor's padding (24 px
        // left, 16 top): x = 31 is inside it, 15 px wide; the text starts
        // past its 24 px column, so x = 600 is beyond the line's end.
        let click = |x: f32, cx: &mut VisualTestContext| {
            cx.simulate_click(
                gpui::point(gpui::px(x), gpui::px(26.)),
                gpui::Modifiers::none(),
            );
            cx.run_until_parked();
        };
        click(31., cx);
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "- [x] buy milk\n\nEnd");
        assert_eq!(host.read_with(cx, |h, _| h.edits), 1);
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), 19, "the caret stays");
        click(31., cx);
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "- [ ] buy milk\n\nEnd");
        // A click on its line places the caret there instead.
        click(600., cx);
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "- [ ] buy milk\n\nEnd");
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), 14, "the line's end");
    }

    #[gpui::test]
    fn up_and_down_step_through_a_code_block_that_ends_the_note(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "Intro\n\n```\none\ntwo\n```");
        let editor = live(&host, cx);
        let line = |cx: &mut VisualTestContext| {
            cx.run_until_parked();
            cx.update(|_, cx| editor.caret_line_column(cx))
        };
        cx.simulate_keystrokes("down");
        assert_eq!(line(cx), (3, 0), "into the code's first line");
        cx.simulate_keystrokes("down");
        assert_eq!(line(cx), (4, 0));
        // Past the closing fence: the empty line that ends the note.
        cx.simulate_keystrokes("down");
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), 22);
        cx.simulate_input("x");
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| editor.text(cx)),
            "Intro\n\n```\none\ntwo\n```\nx"
        );
        cx.simulate_keystrokes("up");
        assert_eq!(line(cx).0, 4, "back into the code");
        cx.simulate_keystrokes("up up");
        assert_eq!(line(cx), (2, 0), "the opening fence, shown");
    }

    #[gpui::test]
    fn the_language_picker_writes_the_language_chosen(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "```rust\nlet x = 1;\n```\n\nEnd");
        let editor = live(&host, cx);
        cx.simulate_keystrokes("ctrl-end");
        cx.run_until_parked();
        // The button sits over the block's top-left corner, past the
        // editor's padding (24 px left, 16 top).
        let open_picker = |cx: &mut VisualTestContext| {
            cx.simulate_click(
                gpui::point(gpui::px(30.), gpui::px(24.)),
                gpui::Modifiers::none(),
            );
            cx.run_until_parked();
        };
        let text = |cx: &mut VisualTestContext| cx.update(|_, cx| editor.text(cx));
        open_picker(cx);
        cx.simulate_input("pyth");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(text(cx), "```python\nlet x = 1;\n```\n\nEnd");
        assert_eq!(host.read_with(cx, |h, _| h.edits), 1);
        // A name not in the list, as typed.
        open_picker(cx);
        cx.simulate_input("mylang");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(text(cx), "```mylang\nlet x = 1;\n```\n\nEnd");
        // A custom name runs to 20 characters; a longer one isn't offered.
        open_picker(cx);
        cx.simulate_input("abcdefghijklmnopqrstu");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(text(cx), "```mylang\nlet x = 1;\n```\n\nEnd");
        open_picker(cx);
        cx.simulate_input("abcdefghijklmnopqrst");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(text(cx), "```abcdefghijklmnopqrst\nlet x = 1;\n```\n\nEnd");
        // Down steps through the list: txt, javascript, python.
        open_picker(cx);
        cx.simulate_keystrokes("down down enter");
        cx.run_until_parked();
        assert_eq!(text(cx), "```python\nlet x = 1;\n```\n\nEnd");
        // Escape changes nothing, and typing goes to the note again.
        open_picker(cx);
        cx.simulate_keystrokes("escape");
        cx.simulate_input("!");
        cx.run_until_parked();
        assert_eq!(text(cx), "```python\nlet x = 1;\n```\n\nEnd!");
    }

    #[gpui::test]
    fn a_reload_under_an_open_picker_leaves_the_new_text_alone(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "```rust\nx\n```\n\nEnd");
        let editor = live(&host, cx);
        cx.simulate_keystrokes("ctrl-end");
        cx.run_until_parked();
        cx.simulate_click(
            gpui::point(gpui::px(30.), gpui::px(24.)),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        cx.simulate_input("go");
        // From disk, another code block where that one was.
        let new = "```py\ny\n```\n\n```rust\nx\n```";
        cx.update(|window, cx| editor.load(new.into(), window, cx));
        // It closes: Enter goes to the note, and the language isn't written.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let text = cx.update(|_, cx| editor.text(cx));
        assert!(text.starts_with("```py\ny\n```"), "{text:?}");
        assert!(!text.contains("go"), "{text:?}");
    }

    #[gpui::test]
    fn arrows_and_tab_move_between_a_table_s_cells_as_drawn(cx: &mut TestAppContext) {
        // a at 2, b at 6; the delimiter row; c at 22, d at 26.
        let (host, cx) = open(cx, "| a | b |\n|---|---|\n| c | d |\n\nEnd");
        let editor = live(&host, cx);
        let caret = |cx: &mut VisualTestContext| {
            cx.run_until_parked();
            cx.update(|_, cx| editor.caret(cx))
        };
        cx.simulate_keystrokes("ctrl-home");
        assert_eq!(caret(cx), 2, "in the first cell");
        cx.simulate_keystrokes("right right");
        assert_eq!(caret(cx), 6, "on into the next cell");
        cx.simulate_keystrokes("down");
        assert_eq!(caret(cx), 26, "the cell below, not beside");
        cx.simulate_keystrokes("left");
        assert_eq!(caret(cx), 23, "back to the cell before");
        // Tab in the last cell adds a row; typing goes into its first cell.
        cx.simulate_keystrokes("right right tab");
        cx.simulate_input("e");
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| editor.text(cx)),
            "| a | b |\n|---|---|\n| c | d |\n| e |  |\n\nEnd"
        );
        // Drawn again after the edit: Up finds the cell above it.
        cx.simulate_keystrokes("up");
        assert_eq!(caret(cx), 23, "the end of c, above e");
        cx.simulate_keystrokes("shift-tab");
        assert_eq!(caret(cx), 7, "back a cell, to the end of b");
    }

    #[gpui::test]
    fn an_image_is_drawn_at_its_size_and_a_click_on_it_shows_its_source(cx: &mut TestAppContext) {
        let src = "![[pic.png|200x100]]\n\nEnd";
        let (host, cx) = open_among(cx, src, &["pic.png"]);
        let editor = live(&host, cx);
        cx.simulate_keystrokes("ctrl-end");
        cx.run_until_parked();
        // The image is 100 px tall from the editor's top padding (16 px):
        // y = 80 is on it, well below a line of text.
        cx.simulate_click(
            gpui::point(gpui::px(100.), gpui::px(80.)),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        // The caret goes to the embed's end, revealing its source: Left
        // steps into the source, not past an image.
        let embed = "![[pic.png|200x100]]".len();
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), embed);
        cx.simulate_keystrokes("left");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), embed - 1);
        assert_eq!(cx.update(|_, cx| editor.text(cx)), src);
    }

    #[gpui::test]
    fn math_is_typeset_at_its_size_and_a_click_on_it_shows_its_source(cx: &mut TestAppContext) {
        let block = "$$\\sum_{i=1}^{n} \\frac{a_i}{b_i}$$";
        let src = format!("{block}\n\nEnd");
        let (host, cx) = open(cx, &src);
        let editor = live(&host, cx);
        cx.simulate_keystrokes("ctrl-end");
        cx.run_until_parked();
        // Typeset, the sum stands well over a line of text from the top
        // padding (16 px): y = 64 is on it, where `End` would be if it
        // weren't drawn.
        cx.simulate_click(
            gpui::point(gpui::px(30.), gpui::px(64.)),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), block.len());
        cx.simulate_keystrokes("left");
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| editor.caret(cx)),
            block.len() - 1,
            "in its source"
        );
    }

    /// What Live Preview's diagram in block `block` shows once `pending`
    /// says it's done, waiting (up to 10 s) for the background renderer.
    fn diagram_when(
        editor: &TextEditor,
        block: usize,
        cx: &mut VisualTestContext,
        pending: impl Fn(&Option<editor::DiagramShows>) -> bool,
    ) -> Option<editor::DiagramShows> {
        let live = cx.update(|_, cx| editor.views.read(cx).live.clone().expect("Live Preview"));
        for _ in 0..500 {
            // The editor looks again for a drawing on a timer.
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            let shows = cx.update(|_, cx| live.read(cx).diagram(block, cx));
            if !pending(&shows) {
                return shows;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the diagram wasn't drawn");
    }

    #[gpui::test]
    fn a_mermaid_block_shows_its_diagram_until_the_caret_is_in_it(cx: &mut TestAppContext) {
        let src = "```mermaid\nflowchart TD\n  A --> B\n```\n\nEnd";
        let (host, cx) = open(cx, src);
        let editor = live(&host, cx);
        cx.simulate_keystrokes("ctrl-end");
        let drawing = |s: &Option<editor::DiagramShows>| *s == Some(editor::DiagramShows::Drawing);
        let Some(editor::DiagramShows::Drawn { height, .. }) =
            diagram_when(&editor, 0, cx, drawing)
        else {
            panic!("drawn");
        };
        // A click on it puts the caret into its source, which shows.
        cx.simulate_click(
            gpui::point(gpui::px(60.), gpui::px(40.)),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), "```mermaid\n".len());
        assert_eq!(diagram_when(&editor, 0, cx, |_| false), None, "its source");
        // Edited, then left: the new drawing replaces the old once it lands.
        cx.simulate_keystrokes("down end");
        cx.simulate_input("\n  B --> C");
        cx.simulate_keystrokes("ctrl-end");
        let taller = diagram_when(
            &editor,
            0,
            cx,
            |s| !matches!(s, Some(editor::DiagramShows::Drawn { height: h, .. }) if *h > height),
        );
        assert!(matches!(taller, Some(editor::DiagramShows::Drawn { .. })));
        // Up from the line below goes into its source too.
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        assert_eq!(diagram_when(&editor, 0, cx, |_| false), None, "its source");
        let (line, _) = cx.update(|_, cx| editor.caret_line_column(cx));
        assert!((1..=3).contains(&line), "in its body: line {line}");
    }

    #[gpui::test]
    fn up_and_down_go_into_a_diagram_s_source(cx: &mut TestAppContext) {
        let src = "Intro\n\n```mermaid\nflowchart TD\n  A --> B\n```\n\nEnd";
        let (host, cx) = open(cx, src);
        let editor = live(&host, cx);
        cx.simulate_keystrokes("ctrl-end");
        let drawing = |s: &Option<editor::DiagramShows>| *s == Some(editor::DiagramShows::Drawing);
        assert!(diagram_when(&editor, 1, cx, drawing).is_some());
        // Up from the line below: into its source, which shows.
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        assert_eq!(diagram_when(&editor, 1, cx, |_| false), None);
        assert_eq!(cx.update(|_, cx| editor.caret_line_column(cx)).0, 3);
        // Down from the line above, once it's drawn again.
        cx.simulate_keystrokes("ctrl-home");
        assert!(diagram_when(&editor, 1, cx, drawing).is_some());
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        assert_eq!(diagram_when(&editor, 1, cx, |_| false), None);
        assert_eq!(cx.update(|_, cx| editor.caret_line_column(cx)).0, 3);
    }

    #[gpui::test]
    fn a_click_on_an_empty_diagram_shows_its_source(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "```mermaid\n```\n\nEnd");
        let editor = live(&host, cx);
        cx.simulate_keystrokes("ctrl-end");
        cx.run_until_parked();
        cx.simulate_click(
            gpui::point(gpui::px(60.), gpui::px(30.)),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), "```mermaid".len());
    }

    #[gpui::test]
    fn a_click_on_a_web_address_opens_it_and_on_a_note_link_follows_it(cx: &mut TestAppContext) {
        let click = |cx: &mut VisualTestContext| {
            cx.simulate_click(
                gpui::point(gpui::px(30.), gpui::px(26.)),
                gpui::Modifiers::none(),
            );
            cx.run_until_parked();
        };
        let (host, cx) = open(cx, "[the web](https://a.b/c) or not\n\nEnd");
        live(&host, cx);
        cx.simulate_keystrokes("ctrl-end");
        click(cx);
        assert_eq!(cx.opened_url().as_deref(), Some("https://a.b/c"));
        assert!(host.read_with(cx, |h, _| h.followed.is_empty()));
        let editor = editor(&host, cx);
        cx.update(|window, cx| {
            editor.load(
                "[a note](Other%20note.md#Top) here\n\nEnd".into(),
                window,
                cx,
            )
        });
        cx.simulate_keystrokes("ctrl-end");
        click(cx);
        assert_eq!(
            host.read_with(cx, |h, _| h.followed.clone()),
            ["Other note.md#Top"]
        );
    }

    #[gpui::test]
    fn live_preview_says_where_it_s_scrolled_and_comes_back_there(cx: &mut TestAppContext) {
        let text: Vec<String> = (0..300).map(|i| format!("Paragraph {i}")).collect();
        let text = text.join("\n\n");
        let (host, cx) = open(cx, &text);
        let editor = live(&host, cx);
        assert_eq!(
            cx.update(|_, cx| editor.scroll(cx)),
            Some(Scroll::LivePreview { top: 0, within: 0. })
        );
        // The wheel scrolls it down.
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(gpui::px(100.), gpui::px(100.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-600.))),
            ..Default::default()
        });
        cx.run_until_parked();
        let scrolled = cx.update(|_, cx| editor.scroll(cx));
        assert!(
            matches!(scrolled, Some(Scroll::LivePreview { top, .. }) if top > 0),
            "{scrolled:?}"
        );
        // Given a place (Paragraph 150's block, 3 px in), it's there.
        let top = text.find("Paragraph 150").unwrap();
        let place = Scroll::LivePreview { top, within: 3. };
        cx.update(|_, cx| editor.set_scroll(place, cx));
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.scroll(cx)), Some(place));
    }

    #[gpui::test]
    fn scrolling_a_long_note_keeps_the_caret_and_selection(cx: &mut TestAppContext) {
        let text: Vec<String> = (0..500)
            .map(|i| format!("- item {i} of the list"))
            .collect();
        let text = text.join("\n");
        // The same keys, in a note left where it is and one scrolled away
        // to the top and back in between: the same selection, the same
        // edit.
        let mut results = Vec::new();
        for scroll_away in [false, true] {
            let mut app = cx.new_app();
            let (host, cx) = open(&mut app, &text);
            let editor = live(&host, cx);
            cx.simulate_keystrokes("ctrl-end shift-up shift-up shift-left");
            let selected = cx.update(|_, cx| editor.selection(cx));
            if scroll_away {
                for dy in [40_000., -40_000.] {
                    cx.simulate_event(gpui::ScrollWheelEvent {
                        position: gpui::point(gpui::px(100.), gpui::px(100.)),
                        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(dy))),
                        ..Default::default()
                    });
                    cx.run_until_parked();
                    let at = cx.update(|_, cx| editor.scroll(cx));
                    let top = matches!(at, Some(Scroll::LivePreview { top: 0, .. }));
                    assert_eq!(top, dy > 0., "scrolled to the top, then back: {at:?}");
                }
                assert_eq!(cx.update(|_, cx| editor.selection(cx)), selected);
            }
            cx.simulate_keystrokes("shift-down");
            let extended = cx.update(|_, cx| editor.selection(cx));
            cx.simulate_input("Z");
            results.push((selected, extended, cx.update(|_, cx| editor.text(cx))));
        }
        assert_eq!(results[0], results[1]);
        assert!(
            results[0].0.start > 0,
            "a selection near the end: {:?}",
            results[0].0
        );
    }

    #[gpui::test]
    fn ctrl_f_finds_in_live_preview_and_enter_steps_through_the_matches(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "Plan the plan.\n\nA PLAN, **planned**");
        let editor = live(&host, cx);
        cx.simulate_keystrokes("ctrl-f");
        cx.simulate_input("plan");
        cx.run_until_parked();
        let selection = |cx: &mut VisualTestContext| cx.update(|_, cx| editor.selection(cx));
        // The match nearest the caret (at the note's start) is selected.
        assert_eq!(selection(cx), 0..4);
        cx.simulate_keystrokes("enter");
        assert_eq!(selection(cx), 9..13);
        cx.simulate_keystrokes("enter enter");
        // Inside the bold: selected, so its markers show.
        assert_eq!(selection(cx), 26..30);
        cx.simulate_keystrokes("enter");
        assert_eq!(selection(cx), 0..4, "wraps");
        cx.simulate_keystrokes("shift-enter");
        assert_eq!(selection(cx), 26..30);
        // Escape goes back to the note, the match still selected.
        cx.simulate_keystrokes("escape");
        cx.simulate_input("X");
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| editor.text(cx)),
            "Plan the plan.\n\nA PLAN, **Xned**"
        );
    }

    #[gpui::test]
    fn ctrl_h_replaces_one_match_with_enter_and_all_at_once(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "plan, plan and plan");
        let editor = live(&host, cx);
        cx.simulate_keystrokes("ctrl-h");
        cx.simulate_input("plan");
        cx.simulate_keystrokes("tab");
        cx.simulate_input("trip");
        // Enter in the replace field: the current match, then the next is
        // current.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "trip, plan and plan");
        assert_eq!(cx.update(|_, cx| editor.selection(cx)), 6..10);
        cx.simulate_keystrokes("ctrl-alt-enter");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "trip, trip and trip");
        // Replace all is one undo step.
        cx.simulate_keystrokes("escape ctrl-z");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "trip, plan and plan");
    }

    #[gpui::test]
    fn the_find_bar_finds_afresh_when_the_note_reloads(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "Plan the plan, a long plan");
        let editor = live(&host, cx);
        cx.simulate_keystrokes("ctrl-h");
        cx.simulate_input("plan");
        cx.simulate_keystrokes("tab");
        cx.simulate_input("trip");
        cx.update(|window, cx| editor.load("a plan".into(), window, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-alt-enter");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "a trip");
    }

    /// Notes for the link tests: typing `[[orbit r` suggests the first,
    /// then offers to create "orbit r".
    const NOTES: &[&str] = &["Orbit roadmap.md", "Recipes.md"];

    /// The editor `host` shows, in Source mode, focused.
    fn source(host: &Entity<Host>, cx: &mut VisualTestContext) -> TextEditor {
        let editor = editor(host, cx);
        cx.update(|window, cx| editor.focus_handle(cx).focus(window, cx));
        cx.run_until_parked();
        editor
    }

    #[gpui::test]
    fn list_keys_edit_a_list_alike_in_both_views(cx: &mut TestAppContext) {
        for in_live in [true, false] {
            let (host, cx) = open(cx, "1. one\n2. two");
            let editor = if in_live {
                live(&host, cx)
            } else {
                source(&host, cx)
            };
            let text = |cx: &mut VisualTestContext| {
                cx.run_until_parked();
                cx.update(|_, cx| editor.text(cx)).to_string()
            };
            cx.simulate_keystrokes("ctrl-home end enter");
            assert_eq!(text(cx), "1. one\n2. \n3. two", "live: {in_live}");
            cx.simulate_input("x");
            cx.simulate_keystrokes("tab");
            assert_eq!(text(cx), "1. one\n\t1. x\n2. two", "live: {in_live}");
            cx.simulate_keystrokes("shift-tab");
            assert_eq!(text(cx), "1. one\n2. x\n3. two", "live: {in_live}");
            assert_eq!(cx.update(|_, cx| editor.caret(cx)), 11, "live: {in_live}");
            // Each is one undo step.
            cx.simulate_keystrokes("ctrl-z");
            assert_eq!(text(cx), "1. one\n\t1. x\n2. two", "live: {in_live}");
        }
    }

    #[gpui::test]
    fn outside_a_list_source_mode_keys_do_what_kit_s_do(cx: &mut TestAppContext) {
        let (host, cx) = open(cx, "plain");
        let editor = source(&host, cx);
        cx.simulate_keystrokes("ctrl-end enter");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "plain\n");
    }

    #[gpui::test]
    fn in_a_list_item_enter_still_takes_a_link_suggestion(cx: &mut TestAppContext) {
        for in_live in [true, false] {
            let (host, cx) = open_among(cx, "", NOTES);
            let editor = if in_live {
                live(&host, cx)
            } else {
                source(&host, cx)
            };
            cx.simulate_input("- [[orbit r");
            cx.run_until_parked();
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
            let text = cx.update(|_, cx| editor.text(cx));
            assert_eq!(text, "- [[Orbit roadmap]]", "live: {in_live}");
        }
    }

    #[gpui::test]
    fn typing_a_wikilink_in_live_preview_suggests_notes_and_enter_accepts(cx: &mut TestAppContext) {
        let (host, cx) = open_among(cx, "", NOTES);
        let editor = live(&host, cx);
        cx.simulate_input("See [[orbit r");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "See [[Orbit roadmap]]");
        assert_eq!(cx.update(|_, cx| editor.caret(cx)), 21);
    }

    #[gpui::test]
    fn the_create_row_writes_the_typed_name_as_the_link(cx: &mut TestAppContext) {
        let (host, cx) = open_among(cx, "", NOTES);
        let editor = live(&host, cx);
        cx.simulate_input("[[orbit r");
        cx.simulate_keystrokes("down tab");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "[[orbit r]]");
    }

    #[gpui::test]
    fn escape_closes_the_suggestions_so_enter_breaks_the_line(cx: &mut TestAppContext) {
        let (host, cx) = open_among(cx, "", NOTES);
        let editor = live(&host, cx);
        cx.simulate_input("[[orbit r");
        cx.simulate_keystrokes("escape enter");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "[[orbit r\n");
    }

    #[gpui::test]
    fn the_suggestions_refilter_as_you_type_and_backspace(cx: &mut TestAppContext) {
        let (host, cx) = open_among(cx, "", NOTES);
        let editor = live(&host, cx);
        cx.simulate_input("[[orbit");
        cx.simulate_keystrokes("backspace backspace backspace backspace backspace");
        cx.simulate_input("rec");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "[[Recipes]]");
    }

    #[gpui::test]
    fn pasting_part_of_a_name_suggests_too(cx: &mut TestAppContext) {
        let (host, cx) = open_among(cx, "", NOTES);
        let editor = live(&host, cx);
        cx.simulate_input("[[");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("recip".into()));
        cx.simulate_keystrokes("ctrl-v enter");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| editor.text(cx)), "[[Recipes]]");
    }
}
