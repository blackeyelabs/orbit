# Live Preview as the default editor: what it takes

- **Researched:** 2026-10-29
- **Question:** Spike 1 runs a Live Preview: markdown syntax is hidden unless the caret touches it, and the text is editable in place. What's required to make that the default editor, instead of today's Split view (Kit's source editor beside a rendered preview)?
- **Method:**
  - read the spikes, today's editor, the plan doc and the pinned Kit sources (`~/.cargo/registry/src/index.crates.io-*/gpui-base-0.8.0`), cited as `file:line`;
  - **[decided]** marks what the Phase 0 gate and the ADRs already settled.

## The short answer

Live Preview isn't a mode to switch on. It's an editor of Orbit's own, still to be written, in `crates/editor`.
- The spike proved the hardest inline part: hiding syntax with a correct caret, selection, soft wrap and IME.
- It didn't build an editor around that: no scrolling, no undo, no blocks, and the whole note reshaped every frame.
- The replacement must also keep everything Kit's editor gives Orbit today: undo, find, multi-cursor, autocomplete, and the Copy/Cut that guards protected notes.
- That's the plan doc's Phase 2: "Live Preview editor replaces the split view: concealed inline syntax first, then block widgets (images, callouts, math, tables, Mermaid)" (`docs/Orbit implementation plan for Claude Code.md:248`).
- It becomes the default only at its gate: "all confirmed markdown features render and edit correctly".

## 1. What exists

**The conceal model** (`spikes/spike1-conceal/src/lib.rs`, 307 lines, std only).
- **API:** `layout(source, Selection { anchor, head })` returns the display text, a source↔display offset map (`to_display`, `to_source`) and styled runs.
- **Reveal rule:** a span's markers show while the caret or selection touches it, its boundaries included.
- **Syntax handled:** ATX headings, `**bold**`, `*italic*`/`_italic_`, `~~strike~~`, `` `code` ``, `[text](url)`, and `[[target]]`/`[[target|alias]]`.
- **Tests:** 20, in `spikes/spike1-conceal/tests/layout.rs`. They include a property test that no caret position across 13 fixtures ever lands in hidden text.

**The GPUI prototype** (`spikes/spike1-gpui/src/main.rs`, 780 lines).
- **What it does:**
  - shapes the display text;
  - maps every caret, click, selection and IME position through the offset map (`EntityInputHandler`, lines 323–424);
  - paints the caret and selection.
- **Checks:** it passed the hands-on checks 1–10 in `spikes/SPIKES.md`. Checks 8–10, IME with fcitx5 and Mozc on Wayland, passed in #27.

**Decisions [decided]:**
- `docs/phase0-gate.md` §1: Live Preview for Phase 2. Keep the conceal model and approach 1's input handler, and render through Kit's `editor_host`. Kit's Editor decorations and tokens are ruled out, because decorations can't hide text and tokens are atomic.
- ADR 0001: the buffer is the source of truth, and Source and Live Preview are two views of the same text.
- `docs/Orbit implementation plan for Claude Code.md:115-126`:
  - rendering is a decoration layer over the buffer: style runs, conceal ranges, line styles and block widgets;
  - variable-height blocks go in a virtualized list;
  - only changed top-level blocks are reparsed, cached by content hash.

**The editor crate:** `crates/editor/src/lib.rs` is a single doc comment, "The Live Preview editor element, buffer and undo". Nothing is in it yet.

## 2. What the spike is not

- **No scrolling:** a fixed nine-line box (`spikes/spike1-gpui/src/main.rs:549-553`).
- **Slow on long notes:** the whole display text is reshaped every frame (`main.rs:572-576`). The performance work found that layout and shaping already dominate typing into a long note (`docs/research/performance.md`).
- **Missing editing:**
  - no undo or redo, and no Cut;
  - no word, line or paragraph movement, no Shift+Up/Down, no double-click;
  - Home and End jump to the ends of the whole document (`main.rs:197-203`).
- **Headings are bold only:** no heading sizes.
- **Simplified painting:**
  - colours and fonts are hard-coded;
  - selection painting assumes every line is the same height.
- **Inline gaps in the conceal model:**
  - no lists, quotes, escapes, or nesting of the same construct;
  - no embeds and images, math, tags, highlights or front matter;
  - inside a code fence, `**` would still be concealed.
- **No blocks:** no lists, tables, code blocks, images, callouts, Mermaid or math.

## 3. What the replacement must keep

These are everything `crates/app/src/note_editor.rs` gets from Kit's `EditorState` today:
- **The text:**
  - `value()` and the `InputEvent::Change` events that drive saving, the Outline and the word count;
  - `set_value` to load text from disk without it counting as an edit.
- **Formatting:** Bold, Italic and Link, through `md_engine::format_edit`, using the selection and `replace` (`apply_format`).
- **Wikilink autocomplete:** `crates/app/src/link_completion.rs` is a Kit `CompletionProvider`, tied to Kit's editor.
- **Navigation:** go to heading or line, Ln/Col in the status bar, scroll restore across sessions, focus.
- **The protected-note guard** (`crates/app/src/clipboard_guard.rs`, `NoteEditor::copy_protected`):
  - it intercepts Kit's `Copy` and `Cut` actions;
  - once you agree, it lets the editor copy every cursor's selection.
- **Free with Kit today:** undo, find, multi-cursor, code highlighting, and a scrolled view of large text.
- **Source mode:** `.txt` notes, and notes of 2 MB or more (`orbit-core/src/lib.rs`, `is_large_note`), stay on Kit's editor.
- **The preview's rendering** (`crates/app/src/preview.rs`):
  - link rewriting and following, and images from the vault;
  - front matter, callouts, inline and block math, unresolved links;
  - Mermaid (`diagram_block.rs`);
  - Kit's tables and code blocks.

Autosave, the save conflicts (Load theirs, Keep mine, Save a copy, Save elsewhere) and `OpenNote` depend only on the text and its change events, so they carry over unchanged.

## 4. What has to be built, in `crates/editor`

1. **A buffer:**
   - a rope, with undo and redo (Kit's `History<T>`, `gpui-base/src/history.rs`, could serve);
   - the full set of editing and movement commands.
2. **Blocks:** the note split into top-level blocks, each reparsed only when it changes and cached by content hash, in a virtualized view of blocks of different heights.
3. **Rendering through `editor_host`** (`gpui-base-0.8.0/src/text/editor_host.rs`, 1,322 lines). It parses markdown into `BlockNode`s, renders them, and gives per-leaf geometry (`LeafGeometry`: caret positions, index-for-point). The pieces:
   - **Prose:** the conceal model builds keyed `Paragraph` and `Heading` nodes.
   - **Widgets:** tables, images, Mermaid, math and callouts reuse the preview's plugins.
   - **The caret's block:** a widget block the caret is in drops to its raw source.
4. **Caret, selection and hit-testing across blocks.** `editor_host` says it "is not a complete visual-caret navigation or pagination API" (`editor_host.rs:18-22`). Moving between blocks, selecting across them and lines of different heights are Orbit's to build.
5. **A wider conceal model:**
   - lists, quotes, fences, escapes, nesting;
   - embeds, images, math, tags, highlights, front matter;
   - distinct types for source and display offsets, as `docs/phase0-gate.md` notes.
6. **Kit's features, rebuilt:**
   - the autocomplete popup;
   - find;
   - theme colours and fonts;
   - Copy and Cut actions the clipboard guard can intercept;
   - Ln/Col.
7. **An editor facade**, so `NoteEditor` can drive either editor (text, change events, selection, replace, caret, scroll) and Source mode keeps Kit's.

## 5. Risks

- **The editor's hard core:** variable-height virtualization with caret movement and selection across blocks. The plan doc rates Live Preview High in likelihood and High in impact (`docs/Orbit implementation plan for Claude Code.md:270`).
- **Speed on long notes:** layout dominated typing into a 200 KB note before the preview was cached (`docs/research/performance.md`), and the spike reshapes everything every frame.
- **`editor_host` is new in Kit 0.8.0 and untried in Orbit:**
  - Its `TextMark` (`gpui-base-0.8.0/src/text/node.rs:700-711`) has bold, italic, strike, underline, code, a highlight background and a link, but **no foreground colour or font size**. Muting revealed markers needs a workaround, or a change upstream.
  - Kit's own source map is crate-private, so Orbit keeps its own offset map.
- **Regressions in what Kit gives for free:** undo, find, multi-cursor and completion, and the exact Copy/Cut behaviour the protected-note guard relies on.

## 6. A route

This follows the plan doc's order, with Split view the default until the gate:

1. **Tracer bullet:** paragraphs and headings in Live Preview behind a setting, with scrolling, undo and the editor facade.
2. **All inline syntax** concealed and editable, with autocomplete, formatting, the clipboard guard and find.
3. **Block widgets, one at a time:** lists, quotes, code, tables, images, callouts, math, Mermaid.
4. **Long notes:** virtualization and incremental reparsing, measured with `scripts/measure.sh` against the fixture vault.
5. **The gate:** "all confirmed markdown features render and edit correctly". Live Preview becomes the default, with Split and Source still available.

## Sources

- `spikes/spike1-conceal/src/lib.rs`
- `spikes/spike1-conceal/tests/layout.rs`
- `spikes/spike1-gpui/src/main.rs`
- `spikes/SPIKES.md`
- `docs/phase0-gate.md`
- `docs/adr/0001-buffer-is-source-of-truth.md`
- `docs/Orbit implementation plan for Claude Code.md`, lines 115–128, 248 and 270
- `docs/research/performance.md`
- `crates/editor/src/lib.rs`
- `crates/app/src/{note_editor.rs, link_completion.rs, clipboard_guard.rs, preview.rs, diagram_block.rs}`
- `crates/orbit-core/src/lib.rs`
- Kit's pinned sources, in gpui-base 0.8.0:
  - `src/text/editor_host.rs`;
  - `src/text/node.rs`;
  - `src/history.rs`;
  - `src/input/editor/` (folding, `InlineToken`, decorations).
