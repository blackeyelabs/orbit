# Phase 0 spikes

Each spike answers one question for the Phase 0 gate (ticket #6). Record results here; the gate document links to them.

## Spike 1: Live Preview conceal (#3)

**Question:** can Live Preview, where markdown syntax is hidden unless the caret or selection touches it, be built on GPUI Kit 0.8.0 with correct caret, selection and IME? If not, the MVP ships the Split view and Live Preview is revisited later.

### What's here

| Crate | What it is | Run |
|---|---|---|
| `spike1-conceal` | std-only conceal model: `layout(source, selection)` returns the display text, a source↔display offset map and styled runs | `cargo test -p spike1-conceal` |
| `spike1-gpui` | a small editable note (a heading line plus soft-wrapped paragraphs) drawn through the model with GPUI's text system (approach 1), adapted from gpui-pre's `examples/input.rs` | `cargo run -p spike1-gpui` |

### Status per acceptance criterion

| Criterion (#3) | Status |
|---|---|
| std-only conceal model: headings, bold, italic, strikethrough, code, links, wikilinks with aliases, reveal rule, offset map | **Pass**: 20 tests |
| ≥10 tests including "no caret position lands inside hidden text" | **Pass** |
| GPUI prototype: click, drag, arrow keys and soft wrap map through the offset map | **Built; hands-on checks pending** (checklist items 1–7) |
| Kit Editor decorations / inline tokens and `editor_host` tried as alternatives | **Partial**: both evaluated against the gpui-base 0.8.0 source, not prototyped. Approach 2 fails on API grounds. `editor_host` is recommended for a Phase 2 prototype |
| IME (fcitx5 or ibus, Japanese or Chinese) inside a revealed span and next to a concealed one | **Pending** (checklist items 8–10) |
| SPIKES.md records pass/fail and a recommendation | **This file**. The recommendation waits on the checklist |

### Results so far

**Model (automated, passing).** 20 tests through the `layout()` seam:
- Syntax handled: ATX headings, `**bold**`, `*italic*`/`_italic_`, `~~strike~~`, `` `code` `` (with no parsing inside it), `[text](url)` and `[[target]]`/`[[target|alias]]`.
- The reveal rule counts boundary contact as touching, and a selection reveals every span it overlaps.
- Unclosed syntax stays as plain text. `[[target|]]` shows the target. `_` inside words (`snake_case_name`) is not italic.
- Style runs mark revealed syntax separately, so the renderer can mute it.
- **The key property holds.** Across 13 fixtures, including CJK, adjacent spans and an empty alias, every caret position survives the source→display→source round trip. So the caret is never inside or at the start of hidden text, and typing, deleting and IME composition always act on visible text. A mutation check confirmed the test fails if boundary contact stops counting as touching.
- What the property does not cover: it holds partly by construction, because touching a span reveals it. It doesn't cover a click mapped through a frame whose layout differs from the one the click lands in. That's checklist item 3.
- Known limits, unchanged from the design doc: no nesting of the same construct, escapes, lists, quotes or setext headings.
- For Phase 2: source and display offsets are both plain `usize`, and only naming keeps them apart. The real editor should give them distinct types.

**Approach 1, the GPUI text system with offset mapping (built; needs hands-on checks).**
- The prototype compiles against gpui-kit 0.8.0 / gpui-pre 0.3.9, opens a window and runs without errors on Wayland. The rendering hasn't been looked at yet; that's step 1 of the checklist below.
- Every frame shapes `display_text()` with `shape_text` (soft wrap at the box width) and flattens the model's style runs into `TextRun`s.
- Caret and selection painting, mouse hit-testing, Up/Down and the IME callbacks (`bounds_for_range`, `character_index_for_point`) all map through `to_display`/`to_source`. The source text stays the model, and `text_for_range` and the edit callbacks work on source offsets.
- Click mapping rule: a display position maps to the start of the next visible source character. Clicking just before a concealed bold word therefore puts the caret *inside* the span, after `**`. To type just outside the opening marker, use the arrow keys (item 7). This is a deliberate model rule. Change it if the checklist shows it feels wrong.
- IME geometry comes from the last painted frame. Between an edit and the next paint, offsets can run past what was shaped, so positions clamp to the end of the nearest line instead of jumping to the corner. Watch items 8–10 for a candidate window that lags a keystroke behind.
- Font finding (checklist item 1): with GPUI's default `.SystemUIFont` on Linux, bold and italic weights and styles never take effect, and `"monospace"` doesn't resolve. Orbit must bundle and name its fonts, which Phase 0 already plans: Plus Jakarta Sans and JetBrains Mono, ticket #8.
- Limit found while building: one shaped line has one font size, so headings render bold rather than larger. Real Live Preview needs per-line layout for heading sizes. That's expected, since the editor is one element per block.

**Approach 2, Kit Editor range decorations or inline tokens (evaluated from gpui-base 0.8.0 source): fails for inline syntax.**
- `TextDecoration` and `RangeDecoration` apply a `HighlightStyle` or a fill/frame "without changing text layout". They can recolour syntax but can't hide or collapse it, so the text keeps its width.
- `InlineToken` replaces a source range with an atomic label (`text` vs `label`). It could conceal a link or wikilink, but the caret can't enter a token to edit it, and it can't hide bold, italic or heading markers whose content must stay editable.
- Verdict: no route to Live Preview through the stock Editor without upstream changes.

**Approach 3, `gpui_kit::base::text::editor_host` (new in 0.8.0; evaluated from source, not prototyped).**
- What it provides: the host keeps its own document, selection, undo and input handling, and hands Kit a `BlockNode` tree (`Paragraph` of `InlineNode`s with `TextMark` styles) with a host-chosen leaf key per text leaf. Rendering happens inside `collect_leaf_geometry`. It returns a `LeafGeometry` per key with `position_for_index`, `index_for_position`, `caret_height` and `bounds`, and paints host decorations (such as the selection) behind the text.
- Fit: our model maps one-to-one. `display_text()` becomes a paragraph's text, `runs()` become `TextMark`s, and `to_display`/`to_source` translate leaf indices. Block widgets (tables, images, code, and later Mermaid) come for free because `BlockNode` covers them. That's the part approach 1 doesn't solve.
- Gaps: the module says it "is not a complete visual-caret navigation or pagination API". The host still owns `EntityInputHandler`/IME (the same code as approach 1) and Up/Down across blocks. The API is a week old.
- Verdict: the most promising route for Phase 2's block widgets. Prototype it when Live Preview work starts, reusing this model and approach 1's input handler unchanged.

### Hands-on checklist (needs a person at the keyboard)

Run `cargo run -p spike1-gpui` on Wayland. The window shows a heading line and two paragraphs, with a debug line underneath: `caret source=… display=…`, the selection and the IME marked range, plus the raw source. Record **pass/fail and notes** in the table.

1. **Rendering.** With the caret at the end (the start state), all syntax is hidden: the heading shows without `##`, bold, italic, struck, code and link text are styled, and the wikilink shows "an alias".
2. **Reveal on caret.** Click inside "bold". Only that span's `**` appear, muted. Click elsewhere and they disappear again. Repeat for the link, the wikilink and the heading line.
3. **Click mapping.** Click just before and just after a concealed word. The caret lands where you clicked, and the debug `source=` offset matches the character you clicked next to.
4. **Arrow keys across syntax.** Use Left/Right through `**bold**` from outside. The markers appear as the caret touches the span, and the caret steps through them one character at a time without skipping or sticking. Home and End work.
5. **Selection and drag.** Drag from plain text into the middle of "bold". The selection highlight matches the text, and the revealed markers don't make it jump. Shift+Left/Right extends correctly, and Ctrl+C then Ctrl+V pastes the raw source text.
6. **Soft wrap.** Make the window narrower and type at the end. The paragraph wraps, Up/Down move between visual lines keeping roughly the same x, and clicks on wrapped lines map correctly.
7. **Typing at boundaries.** Type a character right after the closing `**` and right before the opening one. It lands outside the bold, and the source shown in the debug line confirms it.
8. **IME inside a revealed span** (fcitx5 or ibus, Japanese or Chinese). Put the caret inside "太字" and compose. The candidate window sits at the caret, the preedit is underlined in place, and committing inserts the text inside the bold.
9. **IME next to a concealed span.** Put the caret one character after `[[リンク]]`, outside the span, and compose. The preedit appears at the caret, not offset by the hidden brackets.
10. **IME while the caret moves into a span.** Start composing just before `**太字**`, then commit. Nothing is duplicated or lost, and the layout settles.

| # | Pass/fail | Notes |
|---|---|---|
| 1 | Partial (2026-10-26, from screenshot) | Concealment correct: `##`, link and wikilink syntax hidden, alias shown, strikethrough drawn, and a newly typed `[google](…)` concealed live. Bold, italic, heading and code were **not** styled, because GPUI's default `.SystemUIFont` on Linux doesn't switch faces and `monospace` isn't a resolvable family. Fixed by naming the families (Noto Sans, JetBrains Mono). Re-check |
| 2 | | |
| 3 | | |
| 4 | | |
| 5 | | |
| 6 | | |
| 7 | | |
| 8 | | |
| 9 | | |
| 10 | | |

### Recommendation

**Pending the checklist.** If items 1–7 pass and 8–10 pass on at least one of fcitx5 or ibus, inline Live Preview is feasible (approach 1, moving to `editor_host` for block widgets), and the gate records **Live Preview for Phase 2**. The MVP still ships the Split view, per issue #1. If caret mapping or IME fails in a way the model can't fix, the gate records **Split view for the MVP, Live Preview revisited**, with the failing items as evidence.
