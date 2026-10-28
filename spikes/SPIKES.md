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
| IME (fcitx5 or ibus, Japanese or Chinese) inside a revealed span and next to a concealed one | **Pass**: checklist items 8–10 (#27), fcitx5 with Mozc on Wayland |
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
| 8 | **Pass** (2026-10-28, user, #27) | fcitx5 5.1.23 with Mozc on niri (Wayland). Inside 太字: the candidate window sits at the caret, the preedit is underlined in place, and the commit lands inside the bold |
| 9 | **Pass** (2026-10-28, user, #27) | One character after `[[リンク]]`: the preedit appears at the caret, not offset by the hidden brackets |
| 10 | **Pass** (2026-10-28, user, #27) | Composing just before `**太字**`, then committing: nothing duplicated or lost, and the layout settles. No candidate window lagging a keystroke behind was seen |

### Recommendation

**Live Preview is feasible for inline syntax on GPUI Kit 0.8.0, IME included.** Record **Live Preview for Phase 2** at the gate. Its one condition, checklist items 8–10, passed on 2026-10-28 (#27), so Live Preview work may start in Phase 2.

- **Evidence for:** the conceal model's caret property holds across all fixtures, and approach 1 passed every hands-on check (items 1–10). Those cover rendering, reveal on caret, click mapping, arrow keys through syntax, selection and drag, soft wrap, typing at boundaries, and IME composition inside, next to and into a span.
- **IME, checked:** with fcitx5 and Mozc (Japanese) on Wayland, the candidate window sits at the caret and the preedit renders in place, inside a revealed span and next to a concealed one, and committing never duplicates or loses text. Not yet tried: ibus, Chinese input, and X11.
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
- The corpus is 16 diagrams in `corpus/`. That's the doc's 13 kinds, with flowchart split into subgraphs, styling and shapes, plus a composite state diagram. The kinds are: three flowchart variants (subgraphs, styling, shapes), Unicode/CJK labels, sequence with loop and alt, class, ER, state, composite state, Gantt, mindmap, pie, journey, git graph, one invalid input, and a 120-node flowchart. `corpus/user/` holds the user's own 20 diagrams (added 2026-10-26), scored separately below.
- `results.csv` holds the final run's numbers.

### Real APIs (the design doc's guesses were unverified)

| Crate | Version | Call | Dark theme |
|---|---|---|---|
| `merman` | 0.9.2 (alpha) | `Renderer::new().render(RenderRequest::svg(src, OperationControl::new(), SvgRequest::default()))` → `RenderOutput::Svg(Some(out))`, `out.svg()` | Mermaid init directive `%%{init: {"theme": "dark"}}%%` |
| `rusty-mermaid` | 0.2.0 | `to_svg(src, &Theme::light())` (feature `svg`); also has a native `gpui` module | `Theme::dark()` |
| `mermaid-rs-renderer` (`mmdr`) | 0.3.1 | library call `render_strict(src, RenderOptions { theme, .. })`; no subprocess needed | `Theme::dark()` |

### Results

Automated (built-in corpus, light theme, `results.csv`):

| | merman (no ELK) | rusty-mermaid | mmdr |
|---|---|---|---|
| Valid diagrams rendered | 15/15 | 15/15 | 15/15 |
| **Invalid input** | **error**: "Diagram parse error (flowchart-v2): Unterminated…" | **renders a box labelled "A"** | **error**, but reports line 1, col 1 |
| Panics / timeouts | 0 | 0 | 0 |
| 120-node flowchart (final run, light) | 9 ms | 87 ms | 609 ms |
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

### The user's corpus (20 diagrams, 2026-10-26)

The user's diagrams go well beyond the 8 guaranteed MVP types. They add timeline, quadrant chart, swimlane, requirement, XY chart and block diagrams, so the renderer has to cover those too.

| Diagram | merman | rusty-mermaid | mmdr | Notes |
|---|---|---|---|---|
| 01 pie | 2 | 2 | 1 | mmdr clips the title, and the legend overlaps the labels |
| 02 sequence | 2 | 2 | 2 | |
| 03 flowchart | 2 | 2 | 1 | mmdr: double circle |
| 04 flowchart, styled | 2 | 0 | 1 | rusty: parse error on `;`. mmdr turns part of an edge label into a node |
| 05 sequence loop/alt/opt | 2 | 2 | 2 | |
| 06 sequence, activations + `par` | 2 | 2 | 0 | mmdr: parse error on `->>+` |
| 07 git graph (`gitGraph:`) | 2 | 1 | 1 | rusty drops commit ids. mmdr clips the branch label |
| 08 mindmap, 4 levels | 2* | 1 | 1 | *after the stroke-width clamp below. rusty prints `root((mindmap))`. mmdr renders `::icon(...)` as a node |
| 09 timeline | 2 | 1 | 0 | rusty gives "Google" its own column. **mmdr drops "Google"** |
| 10 quadrant chart | 2 | 1 | 2 | rusty: points collide with quadrant labels |
| 11 Gantt | 2 | 2 | 0 | **mmdr misreads `:24d`, and the axis runs to 2058** |
| 12 journey | 2 | 2 | 1 | mmdr drops the faces |
| 13 ER | 2 | 2 | 1 | mmdr's cardinality markers are wrong |
| 14 state | 2 | 2 | 2 | |
| 15 class with `<<interface>>` | 2 | 2 | 0 | mmdr: parse error |
| 16 swimlane | 2 | 0 | 0 | rusty: unknown type. **mmdr silently draws "swimlane" and "beta" as nodes** |
| 17 requirement | 2 | 1 | 1 | rusty: label overlaps the arrow. mmdr: text overflows the boxes |
| 18 git graph, commit types | 2 | 1 | 1 | as 07 |
| 19 XY chart | 2 | 2 | 2 | |
| 20 block | 2 | 0 | 0 | only merman lays out the columns. The others are silently wrong |
| **Total** | **40/40** | 28/40 | 19/40 | |

Refused outright: merman 0, rusty-mermaid 2, mmdr 2. Silently wrong (scored 0 with no error, the worst case for a notes app): merman 0, rusty-mermaid 1, mmdr 4.

**One more integration finding.** merman's mindmap CSS, copied from mermaid.js, sets edge `stroke-width: 17 - 3 * (depth + 1)`, which is negative from depth 5. Browsers ignore the invalid value and draw a 1px line, but resvg draws nothing, so deep branches float with no connecting lines. The adapter clamps negative stroke widths to 1, and with that the user's 4-level mindmap renders fully.

### Recommendation

**merman 0.9.2 without ELK, rendered to SVG and rasterized with resvg, pinned exactly.** The pick is no longer provisional: it scored 40/40 on the user's own diagrams, against 28 and 19, and is the only renderer that handled every diagram type they use.

Integration requirements for the `diagrams` crate, all exercised in this harness's merman adapter:
1. `default-features = false, features = ["all-diagrams", "complete-svg"]`. No ELK, so no EPL-2.0 code.
2. Prepend an init directive with `"htmlLabels": false` and the theme (`default` or `dark`).
3. Pass the SVG through `merman::svg::foreign_object_label_fallback_svg_text`, so mindmap and journey labels render.
4. Clamp negative `stroke-width` values to 1, so deep mindmap edges render.
5. Render on a background thread with a time limit, and show the source plus merman's diagnostic on error.

Watch items:
- merman is an alpha; pin it and upgrade deliberately.
- Class-diagram inheritance and aggregation markers looked wrong on the built-in corpus but fine in the user's class diagram. Recheck when class diagrams land.
- Mindmap `::icon(...)` isn't rendered, because that needs an icon font.
- The guaranteed set (#1) can widen to everything merman draws, since the user already relies on timeline, quadrant, swimlane, requirement, XY and block diagrams.

**Fallback:** mmdr, behind the same `DiagramRenderer` trait, only if merman stalls. It's much weaker on the user's diagrams.
