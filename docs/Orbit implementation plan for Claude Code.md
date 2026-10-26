# Orbit: implementation plan for Claude Code

Oct 22, 2026 · @Ravi Bharti

## How to use this doc

Read this whole doc before planning; it records every decision made so far, so Claude Code can plan Phase 0 without re-asking. Orbit is a local-first, Obsidian-like markdown workspace written in Rust on GPUI Kit, Linux first.

**Status (2026-10-22):** architecture decided. Spike 1's headless model passes its tests. Spike 2's harness is written but has not been run against the real Mermaid crates. No application code exists yet.

**Update (2026-10-26):** Spikes 1 and 2 have been rebuilt and run in this repo, and their results supersede this doc's spike sections. See `spikes/SPIKES.md`. Spike 1: Live Preview is feasible for inline syntax, with IME still to verify (#27). Spike 2: merman without ELK is the provisional renderer.

**Rules for the implementer**

- Items marked **Decided** stay as written unless the user changes them; items marked **Open** need the user's input before work starts.
- Core crates (`md-engine`, `vault-core`, `vault-index`, `vault-crypto`, `vault-fs`, `diagrams`) must never import GPUI.
- Pin `gpui-kit` and its exact `gpui-pre` snapshot together; upgrade them as one step.
- Protected-note plaintext never reaches disk, logs, caches, the on-disk index, or crash dumps.
- Compile every API call before trusting it: several crate APIs below come from documentation only and are marked unverified.
- Work in small, testable steps; each roadmap task should end with passing tests or a runnable demo.

**Vocabulary and decisions (2026-10-26):** `CONTEXT.md` at the repo root is the glossary. Use its terms in code, UI and docs; it replaces "sealed folder/area" with **protected folder**. Architectural decisions are recorded in `docs/adr/`. A grilling session against the design canvas settled these conflicts between this doc and the design:

- Crates are named `orbit-*` instead of `vault-*`. A vault is the folder of notes; Orbit is the app.
- Mermaid's MVP guarantee is flowchart, sequence, class, ER, state, Gantt, mindmap and pie. The design's "unsupported type" copy is generated from the renderer's real list.
- The design adds features this roadmap lacked. MVP: outline, backlinks and tags panels, search filters, wikilink autocomplete, welcome screen, disk-conflict states, narrow layout, clipboard warning, Settings › Appearance and Security. Phase 2: slash menu, table handles, link popover, Copy SVG, properties, unlinked mentions. Phase 3: spell check, rebindable shortcuts.
- `.txt` files are notes (always shown as Source mode). `.canvas` files are deferred (ADR 0005).
- No OS-keyring quick unlock for now. Test targets: Wayland on Hyprland and KDE Plasma, with an X11 smoke test.
- The `vault-spikes` folder was not available, so the spikes are rebuilt from this doc.

**Existing artifacts:** a `vault-spikes` folder holds `spike1-conceal` (std-only, 10 passing tests), `spike2-mermaid` (runner and 15-diagram corpus), and `SPIKES.md` (run instructions). Copy it into the repo before starting.

## Goals and confirmed requirements

Orbit is an Obsidian-like local knowledge workspace: markdown files in a folder, edited in a WYSIWYG editor, with protected folders, Mermaid diagrams, and fast search. Every row below was confirmed by the user.

| Area | Decision |
| --- | --- |
| Editor style | Live Preview (Obsidian-style): the plain-text buffer is the source of truth; syntax is hidden except where the caret or selection touches it |
| UI toolkit | `gpui-component`, now part of GPUI Kit (`gpui-kit` facade) |
| Language | Rust; no JS runtime or webview |
| Markdown | Full GitHub flavour: tables, task lists, strikethrough, autolinks, footnotes, fenced code with highlighting, alerts, math (LaTeX), inline HTML subset, emoji shortcodes, images, wikilinks and embeds |
| Front matter | YAML, preserved as text, never re-serialized |
| Protection | Real encryption at rest, with encrypted filenames; locked areas are invisible and excluded from search, backlinks and indexes; unlock with a passphrase |
| Protected scope | A folder inside a vault, or the whole vault |
| Mermaid | 100% Rust, no JS fallback; flowchart, sequence, class, ER, state, Gantt, mindmap, pie and similar |
| MVP features | Backlinks and tags, full-text search, command palette and quick switcher |
| Later | Graph view, plugins, sync |
| Obsidian compatibility | Best effort: same wikilink, callout and front matter conventions; never read or write `.obsidian/` |
| Platforms | Linux first; macOS and Windows later |
| Vault size | Up to about 10,000 notes |
| Team | Solo developer, learning Rust and GPUI |
| Later plugin UI | For shells such as Omarchy and DankLinux; the form is **Open**, so keep the core headless and UI-separable |

## Tech stack

The stack is pure Rust apart from the system libraries GPUI links (GPU, windowing, fonts). "Pure Rust" here means no JS runtime, no webview, and no C libraries we choose ourselves, which is why the index uses `tantivy` rather than SQLite.

| Layer | Choice | Why | Alternative |
| --- | --- | --- | --- |
| UI | `gpui-kit` 0.7.x (facade over `gpui-base` and `gpui-component`) | Docks, command palette, tree, tabs and text primitives are included; tiled-window fixes for Linux landed in 0.6.4 | Plain `gpui` plus custom widgets |
| Editor | Custom `LiveEditor` element on `gpui-base` text primitives | Concealing syntax and embedding block widgets in the text flow is not in Kit | Extend Kit's `Editor` upstream |
| Markdown | `comrak` | GitHub-compatible parser with tables, task lists, footnotes, alerts, math, wikilinks, front matter and source positions | `pulldown-cmark` |
| Buffer | `ropey` plus own undo transactions | Byte offsets map directly to parser spans | Kit's text types, if exposed |
| Front matter | Kept as text; parsed read-only for the index | Lossless round trip of comments and key order | `saphyr` or `serde_yaml_ng` |
| Search and links | `tantivy` plus an in-memory link graph, cached with `redb` or postcard | Pure Rust; ample for 10k notes | SQLite FTS5 |
| Fuzzy match | `nucleo` | Quick switcher and command palette | `fuzzy-matcher` |
| File watching | `notify` with debounce | Picks up external edits and git pulls | None |
| Crypto | RustCrypto: `argon2`, `chacha20poly1305`, `hkdf`, `zeroize`, `secrecy` | Audited primitives, no custom crypto | `age` envelope |
| Mermaid | Behind a `DiagramRenderer` trait; renderer chosen by Spike 2 | Coverage differs widely between crates | See the Mermaid section |
| SVG raster | `resvg` and `usvg` | Pure Rust, needed if the renderer outputs SVG | Skip with a native canvas backend |
| Packaging | GitHub Actions, then AUR and AppImage | Linux first | Flatpak |

**Verified ecosystem facts (October 2026)**

- GPUI Kit's latest release is v0.7.3 (12 Oct 2026). It pins GPUI to the exact `gpui-pre` 0.3.9 snapshot, so upgrade both together. Expect breaking changes between minor releases.
- Kit v0.6.0 renamed the Longbridge `gpui-component` project to GPUI Kit and added `gpui-base` (unstyled editing and rich-text primitives). The unrelated crate `gpuikit` on crates.io is a different project.
- Kit's `Editor` offers multi-cursor, folding, search, tree-sitter highlighting, edit-tracked range decorations and atomic inline tokens. Its `TextView` renders markdown with optional YAML front matter, math, tables and images. Neither conceals syntax or hosts block widgets inside editable text.
- Kit v0.6.1 added a headless UI test API behind the `test-support` feature; use it for editor tests.
- Several parts are **unverified until compiled**: exact crate versions, `comrak` extension flags, and Kit's text-layout hooks.

## Architecture

Three layers with one-way dependencies: the UI calls the headless core, and the core reaches the disk only through the `VaultFs` trait. That keeps the core testable without a window and reusable by the later shell plugin.

&#91;embedded content: layered architecture · 3 layers, 12 components\]

**Dependency rules (Decided)**

- The core crates never import GPUI.
- All file access goes through `VaultFs`; `SealedFs` mounts unlocked protected areas into the same logical tree, so search and links treat them uniformly.
- The on-disk index and cache hold only unprotected notes; unlocked protected notes are indexed in RAM only.

**Crates**

| Crate | Purpose | Depends on |
| --- | --- | --- |
| `md-engine` | Parse, block cache, span and decoration model, wikilink and callout extensions | `comrak` |
| `vault-core` | Vault open, note model, link resolution, front matter, rename and refactor | `md-engine`, `vault-fs` |
| `vault-index` | Full-text index, backlinks, tags, headings | `tantivy`, `vault-core` |
| `vault-crypto` | KDF, key wrapping, blob format, zeroization | RustCrypto crates only |
| `vault-fs` | `VaultFs`, `PlainFs`, `SealedFs`, file watcher | `vault-crypto`, `notify` |
| `diagrams` | `DiagramRenderer` trait, chosen Mermaid backend, cache | Mermaid crate, `resvg` |
| `editor` | `LiveEditor` element, buffer, undo, IME handling | `gpui-kit`, `md-engine` |
| `app` | Window, docks, command palette, panels, settings | all of the above |
| `vault-api` (Phase 3) | Command and query facade for IPC or CLI | core crates |

**Key data flows**

- **Typing:** buffer edit, then incremental block reparse, then span and decoration update, then editor repaint. A debounced background job updates the index.
- **Mermaid block:** fenced source goes to the renderer on a background thread, keyed by a hash of source, theme and scale; the last good render stays if a new one fails.
- **Unlock:** passphrase, KDF, master key, enumerate blobs, rebuild tree and in-memory index, mount into `VaultFs`.
- **Lock:** zeroize keys, drop the mount, purge index entries, close tabs, clear undo history.

## Live Preview editor

**Decided:** the source buffer is the single source of truth, so markdown and front matter round-trip exactly. Rendering is a decoration layer over the buffer, never a separate document model.

**Decoration layer** with four kinds: style runs, conceal ranges, line styles (heading size, quote bar, bullets) and block widgets (table, image, Mermaid, math, front matter, callout).

**Reveal rule:** any span or block whose range touches the caret or selection shows its raw source; everything else is rendered. Boundary contact counts as touching.

**Design choices**

- Keep display offsets as close to source offsets as possible: hidden ranges collapse to zero width, so mapping is a simple subtract-the-hidden-length function.
- Render variable-height blocks through a virtualized list so a 50,000-line note stays smooth.
- Reparse only dirty top-level blocks and cache results by content hash.
- Fallback for the MVP: Kit's stock `Editor` beside `TextView` in a split source and preview view. It is already a usable notes app, and Live Preview replaces it in Phase 2.

**Spike 1 result (model half, tested):** `spike1-conceal` implements parsing of headings, bold, italic, strikethrough, code, links and wikilinks, plus the reveal rule and an offset map. Its 10 tests pass, including one proving that no caret position ever touches hidden text. That means the display-to-source ambiguity inside collapsed ranges never affects typing, deleting or IME at the caret. Known limits: no nested spans, escapes, lists, quotes or setext headings.

**Spike 1 still open (needs a GPU machine, one week time-box):**

1. Shape the source text with hidden ranges dropped, using GPUI's text system with per-range style runs, and translate caret, click and drag positions with the offset map. Check soft wrap and arrow-key movement.
2. See whether Kit's range decorations or inline tokens can hide or shrink ranges, which would avoid a custom element for inline syntax.
3. Test IME composition (fcitx5 or ibus, Japanese or Chinese) inside a revealed span and beside a concealed one.

**Pass:** approach 1 or 2 works for inline syntax with correct caret, selection and IME. **Fail:** ship the split editor for the MVP and revisit Live Preview later. Block widgets are a separate, later question.

## Protected folders and vaults

**Decided:** real encryption at rest with encrypted filenames and structure; locked areas are invisible in the UI, search, backlinks and graph; the unlock secret is a passphrase. A short PIN is not a primary secret because offline brute force defeats it.

**On-disk layout**

```text
vault/
  Notes/...md                # plain notes
  .vault/                    # our config and cache (unprotected content only)
  .s/<random128>/            # one sealed area per protected folder
      header                 # KDF parameters and wrapped master key
      blobs/<random128>      # one encrypted blob per note or attachment
```

A whole-vault lock uses the same container with the vault root as its scope.

**Key hierarchy**

- Passphrase goes through Argon2id (memory at least 64 MiB, tuned to about one second on target hardware) to give a key-encryption key.
- A random 256-bit master key is wrapped by that key. Changing the passphrase re-wraps the master key and re-encrypts no notes.
- Each blob gets its own key via HKDF from the master key and the blob ID, encrypted with XChaCha20-Poly1305 using a random 192-bit nonce. Associated data binds the format version and blob ID.
- Large attachments are encrypted in chunks (STREAM construction).
- A one-time recovery key is shown at creation. Without it, a forgotten passphrase means permanent data loss.

**Hidden filenames without a manifest:** each blob's first encrypted chunk holds its logical path and modification time. Unlock decrypts only those small chunks to rebuild the tree, so there is no central manifest to corrupt or conflict in sync.

**Unlock flow:** passphrase, then KDF, then master key, then enumerate blobs and rebuild the tree and an in-memory index, then mount into `VaultFs`. **Lock flow:** zeroize keys, drop the mount, purge index entries, close tabs, clear undo history.

**Still visible to someone with the disk:** that encrypted areas exist, blob counts and approximate sizes (padding can reduce this). State this limit plainly in the app.

**Leak checklist.** Protected notes must never reach: the on-disk index or cache, temp or swap files and crash dumps (disable core dumps, best-effort `mlock`), undo history after lock, recents lists, thumbnails, or the clipboard without a warning. Add auto-lock on idle, suspend and screen lock.

**Optional later:** a quick-unlock PIN that releases a key held in the OS keyring, valid only on that device. **Open:** whether to offer it at all.

## Data model and index

The index lives in memory and is snapshotted to a cache for unprotected notes only. Records below are the starting shape, not a final schema.

```rust
struct NoteRecord {
    id: NoteId,                 // stable hash of vault-relative path
    path: PathBuf,
    title: String,              // front matter title, else first H1, else filename
    aliases: Vec<String>,       // from front matter
    tags: Vec<String>,          // front matter plus inline #tags
    mtime: u64, size: u64, content_hash: [u8; 32],
    headings: Vec<Heading>,     // for [[note#heading]]
}
struct LinkRecord {
    src: NoteId, raw_target: String, resolved: Option<NoteId>,
    kind: LinkKind,             // Wiki | Markdown | Embed
    anchor: Option<Anchor>, span: Range<usize>,
}
```

**Tantivy schema:** `path` (stored string), `title` (text, boosted), `body` (text), `tags` (string, multi-valued), `mtime` (fast field). Unlocked protected notes go into a separate in-RAM index that is destroyed on lock.

**Link resolution** follows Obsidian conventions: shortest unique path, aliases from front matter, `[[note#heading]]` and `![[embed]]` forms. Rename-refactor (rewriting inbound links) arrives in Phase 2.

**Front matter policy (Decided):** never re-serialize it wholesale. A future properties panel edits the original text by span patch, so unknown keys, comments and ordering survive.

**Writes:** atomic (temp file, fsync, rename). A file watcher with debounce reconciles external edits; the editor shows a conflict prompt when the open note changed on disk.

## Mermaid rendering

**Decided:** Mermaid must render in 100% Rust. Put the renderer behind a `DiagramRenderer` trait so it can be swapped, render on a background thread, key the cache by a hash of source, theme and scale, and keep the last good render when a new edit fails to parse. On failure show the source plus the error, never a blank block.

**Candidates (from each project's own pages; none run by us yet)**

| Crate | Reported coverage | Risk |
| --- | --- | --- |
| `merman` | Parity-focused port targeting mermaid@11.16.1, 35 diagram families, no JS runtime; its wiki says Zed uses it as its Mermaid backend (not independently verified) | Published as an alpha |
| `rusty-mermaid` | 25 diagram types, layout engine, several backends including a GPUI canvas element | 0.x crate, small project |
| `mmdr` (`mermaid-rs-renderer`) | Pure Rust and very fast, but its README says output quality may not match mermaid-cli yet | Early development |

**Current lean:** `merman` to SVG, rasterized with `resvg`, for maximum compatibility; `rusty-mermaid` if crisp native vector drawing in GPUI matters more. Spike 2 decides.

**Spike 2 (harness written, not yet run against real crates):** `spike2-mermaid` renders a 15-diagram corpus (flowcharts with subgraphs and styling, Unicode labels, sequence with loops, class, ER, state, Gantt, mindmap, pie, journey, git graph, one invalid input, one large flowchart) through each renderer, records status, time and size, and writes `gallery.html` for side-by-side scoring.

**Decision rule:** pick the renderer scoring best on the diagram types the user actually uses; ties go to the one that outputs SVG. Also check dark theme, CJK text, large-diagram time, and that invalid input returns an error rather than panicking.

**Unverified:** the `merman::render_svg` and `rusty_mermaid::to_svg` call signatures come from documentation; the `mmdr` CLI flags in the harness are a guess. Compile and check them first. **Open:** the user should add 20 to 30 of their own real diagrams to the corpus before the final choice.

## Roadmap

Estimates assume part-time solo work while learning Rust and GPUI, so treat them as rough. Each phase ends at a gate; do not start the next phase until its gate passes.

**Phase 0: Foundation (2 to 3 weeks)**

- [ ] Cargo workspace with the crates listed in the architecture section, CI, and a pinned `gpui-kit` Hello World on Linux
- [ ] File tree, tabs and theming shell
- [ ] Spike 1 (GPUI half): conceal ranges with correct caret, click, selection and IME (one-week time-box)
- [ ] Spike 2: run the Mermaid harness on the real crates plus the user's own diagrams, and score them
- [ ] Spike 3: prototype the sealed blob format with test vectors
- [ ] **Gate:** editor approach chosen (Live Preview or split fallback), Mermaid renderer chosen, blob format specified

**Phase 1: MVP (8 to 10 weeks)**

- [ ] Split source and preview using Kit's `Editor` and `TextView`
- [ ] Vault open, file watcher, quick switcher, command palette
- [ ] Full-text search, tags, wikilinks, backlinks panel
- [ ] Sealed folders: create, unlock, lock, edit, auto-lock
- [ ] Mermaid blocks in the preview
- [ ] **Gate:** daily-usable notes app; sealed folders pass the leak checklist

**Phase 2: Core features (10 to 14 weeks)**

- [ ] Live Preview editor replaces the split view: concealed inline syntax first, then block widgets (images, callouts, math, tables, Mermaid)
- [ ] Vault-level protection and recovery-key flow
- [ ] Rename-refactor, attachments, properties panel
- [ ] Inline HTML subset, alerts, emoji shortcodes
- [ ] **Gate:** all confirmed markdown features render and edit correctly

**Phase 3: Hardening (6 to 8 weeks)**

- [ ] Performance at 10,000 notes; fuzz the parser and blob format
- [ ] Independent review of the crypto design
- [ ] AUR and AppImage packaging, then macOS and Windows ports
- [ ] `vault-api` plus IPC or CLI for shell integrations (Omarchy, DankLinux)
- [ ] **Gate:** release candidate

Graph view, plugins and sync stay out of scope until Phase 3 at the earliest.

## Risks, trade-offs and open questions

The largest risk is the custom Live Preview editor; the Phase 0 spikes and the split-view fallback exist to contain it.

| # | Risk | Likelihood | Impact | Mitigation |
| --- | --- | --- | --- | --- |
| 1 | Live Preview in GPUI is much harder than expected (IME, selection across concealed text, wrapping, variable-height virtualization, undo) | High | High | Spike 1 first; offset-preserving design; split-view fallback; Kit `test-support` headless tests |
| 2 | GPUI and Kit churn from exact-pinned `gpui-pre` snapshots and frequent breaking changes | High | Medium | Pin versions, upgrade on a schedule, keep UI behind a thin adapter, keep the core headless |
| 3 | Rust-native Mermaid gaps: alpha crates, layout and font differences, unsupported diagram types | Medium | Medium | `DiagramRenderer` trait, golden test corpus, bundled font, show source plus error, report bugs upstream |
| 4 | Crypto and data-loss mistakes: format bugs, forgotten passphrase, metadata or plaintext leaks | Medium | High | RustCrypto primitives only, versioned format with test vectors, fuzzing, atomic writes, recovery key, outside review before release |
| 5 | Scope creep while learning Rust and chasing Obsidian parity | High | High | Every milestone ships something usable; time-box spikes; graph, plugins and sync wait for Phase 3 |

**Trade-offs made**

- Live Preview over block-tree editing: lossless markdown, but custom editor work.
- `tantivy` plus an in-memory graph over SQLite: pure Rust and fine for 10k notes, but no SQL and the link graph is ours. Revisit near 100k notes.
- Per-blob encryption over one container file: syncs well with git and Dropbox, but blob count and sizes leak.
- Passphrase only: safer, less convenient than a PIN.

**Open questions for the user**

- Form of the later shell plugin (launcher widget, shell panel, reusable UI crate, or IPC front end). Omarchy and DankLinux plugin mechanisms were not verified; a GPUI widget may not embed in a QML-based shell.
- Whether to offer an optional OS-keyring quick-unlock.
- The user's own Mermaid diagrams for Spike 2.
- Minimum supported Linux desktops and compositors (Wayland versus X11) for testing.
