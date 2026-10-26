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
| GPUI prototype: click, drag, arrow keys and soft wrap map through the offset map | **Pass**: checklist items 1–7 |
| Kit Editor decorations / inline tokens and `editor_host` tried as alternatives | **Partial**: both evaluated against the gpui-base 0.8.0 source, not prototyped. Approach 2 fails on API grounds. `editor_host` is recommended for a Phase 2 prototype |
| IME (fcitx5 or ibus, Japanese or Chinese) inside a revealed span and next to a concealed one | **Not tested**: no IME on the test machine. The callbacks are implemented, but nobody has used them |
| SPIKES.md records pass/fail and a recommendation | **Pass**: see Recommendation |

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
| 1 | **Pass** (2026-10-26, from screenshots) | Concealment correct: `##`, link and wikilink syntax hidden, alias shown. After naming the font families (GPUI's default `.SystemUIFont` on Linux doesn't switch faces), bold, italic, heading (bold, not larger), code (JetBrains Mono), strikethrough and links all render. Newly typed `**bold text**`, `_this italic_`, `*italic*` and `[google](…)` concealed and styled live. A caret at source 329 (just after a `\n`) is drawn at the start of the next line, which is correct |
| 2 | **Pass** (2026-10-26, user) | Reveal on caret works: only the touched span's markers appear, and they hide again when the caret leaves |
| 3 | **Pass** (2026-10-26, user) | Clicks before and after concealed words land where clicked, and the debug source offset matches. Clicking just before a bold word puts the caret inside it, by design |
| 4 | **Pass** (2026-10-26, user) | Left/Right through `**bold**`: markers appear as the caret touches the span and it steps through them one character at a time |
| 5 | **Pass** (2026-10-26, user) | Dragging into a span: the highlight follows the text without jumping. Shift+arrows extend. Copy/paste carries the raw markdown |
| 6 | **Pass** (2026-10-26, user) | Soft wrap: the paragraph wraps at the window width, and Up/Down move between visual lines |
| 7 | **Pass** (2026-10-26, user) | Typing right after a closing `**` lands outside the bold, confirmed in the debug source |
| 8 | **Not tested** | No IME set up on the test machine |
| 9 | **Not tested** | No IME set up on the test machine |
| 10 | **Not tested** | No IME set up on the test machine |

### Recommendation

**Live Preview is feasible for inline syntax on GPUI Kit 0.8.0, with IME still unverified.** Record **Live Preview for Phase 2** at the gate, with one condition: checklist items 8–10 must pass before Live Preview work starts.

- **Evidence for:** the conceal model's caret property holds across all fixtures, and approach 1 passed every hands-on caret check (items 1–7). Those cover rendering, reveal on caret, click mapping, arrow keys through syntax, selection and drag, soft wrap, and typing at boundaries.
- **Open risk:** IME composition is untested. The input-handler callbacks map through the same offset map that passed items 3 and 7, and the model guarantees the caret is never in hidden text. Still, the candidate-window position and preedit behaviour haven't been seen. If they fail in a way the model can't fix, fall back to Split view for Phase 2 as well.
- **Route for Phase 2:** keep this model and approach 1's input handler, and prototype rendering through `editor_host` so block widgets (tables, images, code, Mermaid) and per-block heading sizes come from Kit. The Editor decoration/token route is ruled out.
- **Unchanged:** the MVP ships the Split view (issue #1), whatever this spike found.
- **Fonts:** Orbit must bundle and name its fonts. The default `.SystemUIFont` on Linux won't switch to bold or italic faces (#8).

## Spike 2: Mermaid renderer bake-off (#4)

**Question:** which pure-Rust Mermaid renderer should Orbit use? Decision rule from the design doc: best score on the diagram types the user actually uses; ties go to the one that outputs SVG. It must handle dark theme and CJK, render large diagrams in reasonable time, and return an error for invalid input rather than panicking.

### What's here

`spikes/spike2-mermaid` is a standalone cargo project, excluded from the workspace so these alpha crates never enter Orbit's `Cargo.lock`. Each renderer sits behind its own feature, and each builds on its own.

```sh
cd spikes/spike2-mermaid
cargo run --release --features merman,rusty-mermaid,mmdr   # writes output/
cargo test                                                 # harness logic, 6 tests
```

- The harness runs every renderer on every diagram, light and dark, one thread per render. A panic is caught, and a render still running after 20 s is recorded as a timeout and abandoned: its thread keeps running until the process exits, and a stack overflow would still abort the run. It writes `output/out/<renderer>/<diagram>-<theme>.svg`, a resvg PNG of each (how Orbit would draw them), `output/results.csv`, `output/raster-failures.txt` (empty in the final run) and `output/gallery.html`. The gallery shows the PNGs, each linking to its SVG.
- The corpus is 16 diagrams in `corpus/`. That's the doc's 13 kinds, with flowchart split into subgraphs, styling and shapes, plus a composite state diagram. The kinds are: three flowchart variants (subgraphs, styling, shapes), Unicode/CJK labels, sequence with loop and alt, class, ER, state, composite state, Gantt, mindmap, pie, journey, git graph, one invalid input, and a 120-node flowchart. `corpus/user/` is for the user's own diagrams. **It's empty, so the pick below is provisional.**
- `results.csv` holds the final run's numbers.

### Real APIs (the design doc's guesses were unverified)

| Crate | Version | Call | Dark theme |
|---|---|---|---|
| `merman` | 0.9.2 (alpha) | `Renderer::new().render(RenderRequest::svg(src, OperationControl::new(), SvgRequest::default()))` → `RenderOutput::Svg(Some(out))`, `out.svg()` | Mermaid init directive `%%{init: {"theme": "dark"}}%%` |
| `rusty-mermaid` | 0.2.0 | `to_svg(src, &Theme::light())` (feature `svg`); also has a native `gpui` module | `Theme::dark()` |
| `mermaid-rs-renderer` (`mmdr`) | 0.3.1 | library call `render_strict(src, RenderOptions { theme, .. })`; no subprocess needed | `Theme::dark()` |

### Results

Automated (final run, light theme, `results.csv`):

| | merman (no ELK) | rusty-mermaid | mmdr |
|---|---|---|---|
| Valid diagrams rendered | 15/15 | 15/15 | 15/15 |
| **Invalid input** | **error**: "Diagram parse error (flowchart-v2): Unterminated…" | **renders a box labelled "A"** | **error**, but reports line 1, col 1 |
| Panics / timeouts | 0 | 0 | 0 |
| 120-node flowchart | 16 ms | 90 ms | 623 ms |
| Diagram families (claimed) | 35 (`all-diagrams` feature list) | 25 (README) | 23 (crate description) |

Visual score per diagram (0 wrong, 1 flawed, 2 correct), judged from resvg PNGs:

| Diagram | merman | rusty-mermaid | mmdr | Notes |
|---|---|---|---|---|
| Flowchart: subgraphs | 2 | 2 | 2 | |
| Flowchart: styling | 2 | 2 | 2 | All apply `classDef`, `style` and `linkStyle` |
| Flowchart: shapes | 2 | 2 | 1 | mmdr draws `((circle))` as a double circle and flips the flag |
| Flowchart: Unicode/CJK | 2 | 2 | 2 | |
| Flowchart: 120 nodes | 2 | 1 | 2 | rusty is dense, with long crossing edges. merman *with* ELK spread layers very wide |
| Sequence | 2 | 2 | 1 | mmdr's `alt` frame overruns and overlaps the note |
| Class | 1 | 1 | 0 | merman: inheritance shows a plain arrowhead, aggregation a filled diamond. rusty: `Vec~T~` generics not converted. mmdr: tiny, scattered, generics not converted |
| ER | 2 | 2 | 2 | |
| State | 2 | 2 | 1 | mmdr layout sprawls |
| State: composite | 2 | 2 | 1 | mmdr's note and labels overlap the frame |
| Gantt | 2 | 2 | 1 | mmdr's section names collide with task names |
| Mindmap | 2 | 1 | 2 | rusty prints the root literally as `root((Orbit))` |
| Pie | 2 | 2 | 2 | |
| Journey | 2 | 2 | 1 | mmdr drops the satisfaction faces and staggers sections |
| Git graph | 2 | 1 | 1 | rusty overlaps commit labels. mmdr clips the branch label |
| **MVP types** (flowchart, sequence, class, ER, state, Gantt, mindmap, pie) | **15/16** | 13/16 | 10/16 | Flowchart counted once (min of variants), state once |

The tie-break (prefer SVG output) decides nothing: all three output SVG.

Dark theme works in all three. merman's dark Gantt has low-contrast labels on some bars.

### Findings that shape the integration

- **merman's labels are HTML by default.** It mirrors mermaid.js's `htmlLabels: true`, so labels are `<foreignObject>` HTML that resvg can't draw (blank boxes). Orbit must always set `htmlLabels: false` (through the init directive). Mindmap and journey still emit HTML labels anyway, as mermaid.js does, so Orbit must also pass merman's SVG through `merman::svg::foreign_object_label_fallback_svg_text`, which adds a `<text>` overlay. With both, every label renders.
- **License: drop ELK.** merman's default features include an ELK layout engine under **EPL-2.0** (redistribution needs EPL notices). With `default-features = false, features = ["all-diagrams", "complete-svg"]`, ELK is gone from the dependency tree, all diagrams still render, and the layout is as good or better: the 120-node flowchart became readable and the composite-state note stopped truncating. The harness uses this configuration.
- **Class-diagram markers** (merman): inheritance and aggregation markers look wrong in both configurations. Check whether that's merman or resvg's marker support before Phase 1 relies on class diagrams.
- **rusty-mermaid's native GPUI canvas** is attractive (crisp vector drawing with no SVG step), but it's disqualified by drawing invalid input instead of erroring, and by the weaker mindmap and git graph results.

### Recommendation (provisional)

**merman 0.9.2 without ELK, rendered to SVG and rasterized with resvg**, pinned exactly. That matches the design doc's lean.
- It renders all 8 guaranteed MVP types (15/16), is the fastest by far, errors with a clear message on bad input, and has the widest family coverage.
- Integration requirements for `diagrams`: set `htmlLabels: false`, apply the foreign-object fallback, pin the version (it's an alpha), and render on a background thread as planned.
- **Runner-up:** mmdr. It's a good fallback behind the same `DiagramRenderer` trait, but it's weaker on class, sequence and Gantt, and about 40× slower on large graphs.
- **Provisional until** `corpus/user/` holds 20–30 of the user's real diagrams and they're scored the same way.
