//! Code blocks in Live Preview: the block drawn by Kit (highlighted as the
//! preview was), the closing fence shown under it while the caret is in it,
//! and the language picker, a button above the block's top-left corner
//! that opens a searchable list of languages.

use gpui_kit::base::v_flex;
use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use live_preview::blocks::Code;

use crate::LivePreview;

actions!(
    live_preview,
    [
        /// Up in the language picker: the row above.
        PickerUp,
        /// Down in the language picker: the row below.
        PickerDown
    ]
);

/// The language picker's key context.
const KEY_CONTEXT: &str = "LanguagePicker";

/// Binds Up and Down in the picker's search field, over the field's own.
pub(crate) fn init(cx: &mut App) {
    let context = Some("LanguagePicker > Input");
    cx.bind_keys([
        KeyBinding::new("up", PickerUp, context),
        KeyBinding::new("down", PickerDown, context),
    ]);
}

/// The languages the picker offers, most used first (by GitHub's yearly
/// counts, give or take), as fences name them.
pub(crate) const LANGUAGES: [&str; 100] = [
    "txt",
    "javascript",
    "python",
    "typescript",
    "java",
    "csharp",
    "cpp",
    "c",
    "php",
    "go",
    "rust",
    "kotlin",
    "swift",
    "ruby",
    "bash",
    "shell",
    "powershell",
    "sql",
    "html",
    "css",
    "scss",
    "json",
    "yaml",
    "toml",
    "xml",
    "markdown",
    "dart",
    "scala",
    "r",
    "lua",
    "perl",
    "haskell",
    "elixir",
    "erlang",
    "clojure",
    "fsharp",
    "ocaml",
    "julia",
    "matlab",
    "groovy",
    "objectivec",
    "zig",
    "nim",
    "crystal",
    "d",
    "vb",
    "fortran",
    "cobol",
    "pascal",
    "ada",
    "assembly",
    "nasm",
    "solidity",
    "vue",
    "svelte",
    "jsx",
    "tsx",
    "graphql",
    "dockerfile",
    "makefile",
    "cmake",
    "nix",
    "terraform",
    "hcl",
    "ini",
    "erb",
    "diff",
    "ejs",
    "protobuf",
    "latex",
    "tex",
    "mermaid",
    "plantuml",
    "elm",
    "lisp",
    "scheme",
    "racket",
    "prolog",
    "tcl",
    "awk",
    "astro",
    "zsh",
    "fish",
    "batch",
    "applescript",
    "coffeescript",
    "less",
    "sass",
    "handlebars",
    "jinja",
    "liquid",
    "twig",
    "nginx",
    "http",
    "glsl",
    "hlsl",
    "wgsl",
    "cuda",
    "verilog",
    "vhdl",
];

/// The longest custom language name the picker takes.
pub(crate) const MAX_CUSTOM: usize = 20;

/// A code block as drawn: the picker's button over it and, while the caret
/// is in it, its closing fence under it.
pub(crate) struct CodeFrame {
    /// Its block, whose language the picker sets.
    pub block: usize,
    /// Its language and closing fence.
    pub code: Code,
    /// The caret is in it, so its fences show.
    pub revealed: bool,
    /// It's the note's last block: no gap under the shown fence.
    pub is_last: bool,
}

impl CodeFrame {
    /// The closing fence, as written, while it shows.
    pub(crate) fn closing_fence(&self) -> Option<&str> {
        self.code.close.as_deref().filter(|_| self.revealed)
    }
}

/// The gap under a block, in rems, as Kit's default style has it
/// (`TextViewStyle::paragraph_gap`, not readable from a render context):
/// under a shown closing fence, which Kit doesn't draw. It goes out of step
/// if that default changes.
const BLOCK_GAP: f32 = 0.75;

/// A code block around Kit's rendered `node`: the picker's button above
/// it, the closing fence under it while the caret is in it.
pub(crate) fn code_frame(
    frame: &CodeFrame,
    node: AnyElement,
    editor: &Entity<LivePreview>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let editor = editor.clone();
    let block = frame.block;
    let label = frame.code.lang.clone().unwrap_or_else(|| "Language".into());
    let button = div()
        .id(("code-language", block))
        .flex_none()
        .px_1()
        .rounded(theme.radius)
        .text_xs()
        .text_color(theme.muted_foreground)
        .cursor_pointer()
        .hover(|this| this.bg(theme.muted))
        .child(label)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            editor.update(cx, |editor, cx| editor.open_picker(block, window, cx));
        });
    let fence = frame.closing_fence().map(|fence| {
        div()
            .px_4()
            .font_family(theme.mono_font_family.clone())
            .text_color(theme.muted_foreground)
            .when(!frame.is_last, |this| this.pb(rems(BLOCK_GAP)))
            .child(fence.to_string())
    });
    v_flex()
        .w_full()
        .child(div().flex().child(button))
        .child(node)
        .children(fence)
        .into_any_element()
}

/// One row of the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// One of [`LANGUAGES`].
    Language(&'static str),
    /// The name typed, as a language of its own.
    Custom(String),
}

impl Row {
    /// The language it names.
    fn name(&self) -> &str {
        match self {
            Row::Language(name) => name,
            Row::Custom(name) => name,
        }
    }
}

/// The rows for `query`: the languages that start with it, then those
/// that hold it, case ignored; then, unless one is just what was typed, the
/// name typed as a custom language, if it's one word of up to
/// [`MAX_CUSTOM`] characters.
pub(crate) fn rows(query: &str) -> Vec<Row> {
    let query = query.trim();
    let lower = query.to_lowercase();
    let mut rows: Vec<Row> = LANGUAGES
        .iter()
        .filter(|l| l.starts_with(&lower))
        .chain(
            LANGUAGES
                .iter()
                .filter(|l| !l.starts_with(&lower) && l.contains(&lower)),
        )
        .map(|l| Row::Language(l))
        .collect();
    let custom = !query.is_empty()
        && query.chars().count() <= MAX_CUSTOM
        && !query.contains(|c: char| c.is_whitespace() || c == '`')
        && !LANGUAGES.contains(&lower.as_str());
    if custom {
        rows.push(Row::Custom(query.to_string()));
    }
    rows
}

/// The open picker.
pub(crate) struct Picker {
    /// The code block it names the language of.
    block: usize,
    /// The note was reloaded or changed in Source mode since it opened, so
    /// the block may be another: it closes, writing nothing.
    stale: bool,
    /// The search field.
    query: Entity<InputState>,
    /// The languages that match the search, and maybe a custom name.
    rows: Vec<Row>,
    /// The row Enter takes.
    chosen: usize,
    /// The list's scroll, which keeps the chosen row in view.
    scroll: ScrollHandle,
    /// Refilters the rows as the search changes.
    _query_edits: Subscription,
}

/// The picker's width.
const PICKER_WIDTH: Pixels = px(220.);
/// The tallest its list grows: past it, rows scroll.
const PICKER_MAX_HEIGHT: Pixels = px(240.);

impl LivePreview {
    /// A click on a code block's language button: opens the picker for
    /// block `block`, its search field focused.
    pub(crate) fn open_picker(
        &mut self,
        block: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_suggestions(cx);
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search languages"));
        let query_edits = cx.subscribe(&query, |this, query, event: &InputEvent, cx| {
            if let InputEvent::Change = event
                && let Some(picker) = this.picker.as_mut()
            {
                picker.rows = rows(&query.read(cx).value());
                picker.chosen = 0;
                picker.scroll.scroll_to_item(0);
                cx.notify();
            }
        });
        query.update(cx, |query, cx| query.focus(window, cx));
        self.picker = Some(Picker {
            block,
            stale: false,
            query,
            rows: rows(""),
            chosen: 0,
            scroll: ScrollHandle::new(),
            _query_edits: query_edits,
        });
        cx.notify();
    }

    /// The text was replaced under the picker (a reload, Source mode): it
    /// closes on the next frame, writing nothing.
    pub(crate) fn picker_text_replaced(&mut self) {
        if let Some(picker) = self.picker.as_mut() {
            picker.stale = true;
        }
    }

    /// Closes the picker, back to the note.
    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Up or Down in the picker: the row above or below, wrapping.
    fn step_picker(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        let len = picker.rows.len();
        if len == 0 {
            return;
        }
        picker.chosen = if down {
            (picker.chosen + 1) % len
        } else {
            (picker.chosen + len - 1) % len
        };
        picker.scroll.scroll_to_item(picker.chosen);
        cx.notify();
    }

    /// Enter or a click on row `row`: writes its language to the block's
    /// fence, and closes the picker.
    fn pick(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        if let Some(row) = picker.rows.get(row).filter(|_| !picker.stale) {
            let name = row.name().to_string();
            self.edited(cx, |doc| {
                doc.set_code_language(picker.block, &name);
            });
        }
        self.close_picker(window, cx);
    }

    /// The picker, over the top-left of its block's code, while it's open.
    /// Its field losing focus (a click elsewhere) closes it.
    pub(crate) fn render_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self.picker.as_ref()?;
        if picker.stale {
            self.close_picker(window, cx);
            return None;
        }
        if !picker.query.read(cx).focus_handle(cx).is_focused(window) {
            self.picker = None;
            return None;
        }
        // Its block gone or out of view: closed, back to the note.
        let leaf = self
            .doc
            .blocks()
            .blocks
            .get(picker.block)
            .map(|b| b.leaves.start);
        let at = leaf
            .and_then(|leaf| self.leaf_geometry(leaf))
            .map(|g| g.bounds().origin);
        let Some(at) = at else {
            self.close_picker(window, cx);
            return None;
        };
        let theme = cx.theme();
        let rows = picker.rows.iter().enumerate().map(|(ix, row)| {
            let label = match row {
                Row::Language(name) => name.to_string(),
                Row::Custom(name) => format!("Use “{name}”"),
            };
            div()
                .id(("language", ix))
                .px_2()
                .py_1()
                .rounded(theme.radius)
                .text_sm()
                .when(ix == picker.chosen, |this| this.bg(theme.accent))
                .hover(|this| this.bg(theme.accent.opacity(0.6)))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.pick(ix, window, cx);
                    }),
                )
        });
        let list = v_flex()
            .id("languages")
            .track_scroll(&picker.scroll)
            .max_h(PICKER_MAX_HEIGHT)
            .overflow_y_scroll()
            .children(rows);
        let popup = v_flex()
            .key_context(KEY_CONTEXT)
            .occlude()
            .w(PICKER_WIDTH)
            .p_1()
            .gap_1()
            .bg(theme.tokens.popover)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .shadow_md()
            .on_action(cx.listener(|this, _: &PickerUp, _, cx| this.step_picker(false, cx)))
            .on_action(cx.listener(|this, _: &PickerDown, _, cx| this.step_picker(true, cx)))
            .on_action(cx.listener(|this, _: &Enter, window, cx| {
                let chosen = this.picker.as_ref().map_or(0, |p| p.chosen);
                this.pick(chosen, window, cx);
            }))
            .on_action(cx.listener(|this, _: &Escape, window, cx| this.close_picker(window, cx)))
            .child(Input::new(&picker.query).small())
            .child(list);
        Some(deferred(anchored().position(at).snap_to_window().child(popup)).into_any_element())
    }
}
