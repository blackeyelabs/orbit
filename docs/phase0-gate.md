# Phase 0 gate

**Date:** 2026-10-26. **Ticket:** #6. **Evidence:** [`spikes/SPIKES.md`](../spikes/SPIKES.md) (Spikes 1 and 2) and [`docs/protected-folder-format.md`](protected-folder-format.md) (Spike 3).

The design doc requires three decisions before Phase 1 starts: the editor approach, the Mermaid renderer and the protected-folder format. All three are recorded below. **The gate is passed. Phase 1 (#1, MVP tickets #7–#26) may start.**

## 1. Editor approach: Live Preview for Phase 2

**Decision.** Live Preview, with markdown syntax hidden unless the caret or selection touches it, is feasible on GPUI Kit 0.8.0 for inline syntax. It's planned for Phase 2. The MVP ships the Split view regardless, as issue #1 already specifies.

**Condition, met.** Live Preview work was not to start until IME composition was verified in [#27](https://github.com/blackeyelabs/orbit/issues/27). It passed on 2026-10-28 with fcitx5 and Mozc (Japanese) on Wayland: checklist items 8–10 all pass, so Phase 2 may start Live Preview.

**Evidence:** Spike 1, #3; [details](../spikes/SPIKES.md#spike-1-live-preview-conceal-3).
- The **conceal model** is a std-only `layout(source, selection)` that returns the display text, a source↔display offset map and style runs. It passes 20 tests, including a property test across 13 fixtures (CJK, adjacent spans, an empty alias) showing that no caret position ever lands inside hidden text. A mutation check showed that test can fail.
- **Approach 1** is a GPUI prototype that shapes the display text and maps every caret, click, selection and IME position through the offset map. It passed hands-on checks 1–10 on Wayland: rendering, reveal on caret, click mapping, arrow keys through syntax, selection and drag, soft wrap, typing at span boundaries, and IME composition (checks 8–10, run later in #27).
- One Spike 1 criterion is only **partial**: the alternatives were evaluated from the GPUI Kit source, not prototyped.
  - Kit's Editor decorations and inline tokens are ruled out on API grounds. Decorations can't hide text, and tokens are atomic, so text inside a bold span couldn't be edited.
  - Kit 0.8.0's `editor_host` looks like the best route to block widgets, but it's **untested**.

**What it means for later work:**
- **Phase 2:** keep the conceal model and approach 1's input handler, and start by prototyping rendering through `editor_host`.
- **Fonts:** Orbit must bundle and name its fonts. On Linux, GPUI's default `.SystemUIFont` never switches to bold or italic faces. Ticket #8 already plans to bundle Plus Jakarta Sans and JetBrains Mono.
- **Offset types:** source and display offsets should get distinct types in the real editor.

## 2. Mermaid renderer: merman 0.9.2 without ELK

**Decision.** `merman` 0.9.2, pinned exactly, built with `default-features = false, features = ["all-diagrams", "complete-svg"]`. Its SVG is rasterized with `resvg`, behind the `DiagramRenderer` trait. This is **final**, not provisional as the ticket expected: the user's own 20 diagrams were added and scored.

**Evidence:** Spike 2, #4; [details](../spikes/SPIKES.md#spike-2-mermaid-renderer-bake-off-4); raw numbers in [`results.csv`](../spikes/spike2-mermaid/results.csv).

| | merman | rusty-mermaid | mmdr |
|---|---|---|---|
| User's 20 diagrams (0–2 each) | **40/40** | 28/40 | 19/40 |
| Silently wrong output, user corpus | 0 | 1 | 4 |
| Invalid input | clear error | renders it anyway | error, wrong position |
| 120-node flowchart, final run | **~9 ms** | ~87 ms | ~609 ms |

**What it means for later work.** The `diagrams` crate (#20) must do the five things the Spike 2 harness's merman adapter already does, and SPIKES.md explains why:
1. Use no ELK. It's EPL-2.0, and the layout was as good or better without it on every corpus diagram that differed.
2. Set `htmlLabels: false`.
3. Apply merman's foreign-object label fallback.
4. Clamp negative stroke widths.
5. Render on a background thread with a time limit.

**Done in #20 (2026-10-27).**
- **Integration:** Kit's `TextView` hosts the renderer as a `MarkdownPlugin` on fenced code with the `mermaid` language, the same hook as block math. The plugin only asks a `DiagramCache`, which draws on its own thread, and a drawing that lands makes the preview parse again.
- **Cache key:** source and theme, not scale. The cache keeps SVG, and GPUI rasterizes it at each display's scale itself.
- **Class markers:** merman's aggregation and inheritance markers came out *filled* under resvg. resvg drops a whole CSS rule over its attribute selectors (`[id$="-extensionStart"]`), and those rules carried the `fill: transparent` for `.aggregation` and `.extension`. Orbit now removes just those selectors, and a raster test (`crates/diagrams/tests/drawn.rs`) checks that the markers are hollow.

**Change to issue #1.** The guaranteed Mermaid set should widen from the 8 MVP types to at least the 14 families the user's diagrams use, which adds timeline, quadrant chart, swimlane, requirement, XY chart and block. merman rendered all 14 correctly. Before relying on class diagrams, re-check merman's inheritance and aggregation markers.

## 3. Protected-folder format: v1 frozen

**Decision.** Format v1, as specified in [`docs/protected-folder-format.md`](protected-folder-format.md) and implemented in `crates/orbit-crypto`, is **frozen**. Later builds must keep opening v1. Any layout change needs a new version byte, and the v1 vectors must keep passing.

**Evidence:** Spike 3, #5.
- **Design:** a 229-byte header wrapping a random master key under the passphrase and the recovery key, plus one STREAM-encrypted blob per note whose first chunk holds only the path and mtime. The spec describes every byte.
- **Tests:** `cargo test -p orbit-crypto` runs 31, covering:
  - every single-bit flip of a header or blob;
  - every truncation of a blob;
  - swapped, dropped and appended duplicate chunks;
  - Argon2 limits;
  - passphrase changes and recovery-key rotation.
- **Vectors:** frozen v1 vectors, single- and multi-chunk, in [`crates/orbit-crypto/tests/vectors/`](../crates/orbit-crypto/tests/vectors/). One test decrypts them by following the written spec with raw primitives, independent of the crate's own decryption code.
- **Known limits** are listed in the spec's "Not covered by v1" section:
  - blob counts and sizes are visible on disk;
  - old headers and blobs can be rolled back or replayed;
  - an independent design review is still planned before release.

## Phase 1 starts here

- **Start at #7** (open a vault): every other MVP ticket builds on its headless Vault facade.
- **Done since:** #27, the IME check, passed on 2026-10-28. It gated Phase 2 Live Preview, not Phase 1.
- **Carry into Phase 1:** widen the guaranteed Mermaid set in #1 and #20, and re-check class-diagram markers when #20 lands.
