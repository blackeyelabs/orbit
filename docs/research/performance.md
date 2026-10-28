# Performance: measuring and optimizing Orbit's CPU and memory

- **Researched:** 2026-10-28
- **Question:** Orbit uses a lot of CPU and memory while it runs. How should we measure that, where does it come from, and what should be done about it?
- **Method:** each claim is marked as one of:
  - **[measured]**, on the development machine: 16 CPUs, 31.8 GB of RAM, Intel UHD and NVIDIA RTX A2000 Mobile GPUs, CachyOS, niri on Wayland;
  - **[code]**, read from this repo or the pinned library sources in `~/.cargo/registry`, cited as `file:line`;
  - **[docs]**, from primary documentation, linked.

## TL;DR

- **No number before this note came from an optimized build** [measured]. The dev profile has no optimization, except argon2 and blake2 (`Cargo.toml`, `[profile.dev.package.*]`).
  - Measure with the `profiling` profile (release, with line tables): `cargo build --profile profiling -p app --features dev-profiler`.
- **Saving reacts to itself, while typing** [code]:
  - An autosave runs 1.5 s after an edit (`crates/app/src/note_editor.rs`, `AUTOSAVE_DELAY`).
  - The file watcher reports Orbit's own save back as a change (`crates/orbit-fs/src/lib.rs`, the watcher filters out only the temporary file).
  - The window then:
    - walks the whole vault tree, twice;
    - refreshes the right panel;
    - commits and reloads the search index;
    - re-renders the preview of **every open tab** (`OrbitWindow::changed_on_disk`, `crates/app/src/window.rs`).
- **Each keystroke** works through the whole note [code]:
  - a full copy of the text (`value()` makes a new `SharedString` from the rope, gpui-base `input/base/state.rs:1466`);
  - then word count, headings and link rewriting;
  - then a markdown re-parse of the preview (gpui-base `text/state.rs:44, 531`).
- **The caret blinks every 500 ms and repaints its view** (gpui-base `input/base/blink_cursor.rs:4, 73`) [code]. Orbit embeds no view with `.cached(…)`, so the whole window is rebuilt on each blink (gpui-pre `view.rs:40-52, 241-252`).
- **Memory** [code]:
  - Tantivy's writer has a 50 MB budget (`crates/orbit-index/src/lib.rs`, `WRITER_MEMORY`). That's a ceiling, and the arena grows in 1 MB pages (tantivy `indexer/index_writer.rs:27-32`).
  - Each unlocked protected folder adds an in-memory index with its own writer.
  - The plain index is memory-mapped, so it shows as file-backed pages, not heap.
- **GPU** [code]: gpui-pre 0.3.9 renders with wgpu, and prefers a discrete GPU over an integrated one (gpui-pre-wgpu `wgpu_context.rs:588-593`). On a hybrid laptop that can wake the NVIDIA GPU. `ZED_DEVICE_ID=<pci id>` picks the adapter.

## How to measure

**Build**
- Use `cargo build --profile profiling -p app --features dev-profiler`.
  - The `profiling` profile inherits release and keeps line tables, which is what samply recommends ([samply](https://github.com/mstange/samply)).
  - Cargo's release defaults are `opt-level = 3` and `codegen-units = 16`, without LTO ([Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)).
- The `dev-profiler` feature (`crates/app/Cargo.toml`) adds two things:
  - **GPUI's frame-time overlay**, through `gpui-kit/profiler`. Ctrl+Alt+Shift+F cycles it (`Window::cycle_debug_frame_overlay_mode`, gpui-pre `window.rs:3649-3658`, painted by `debug_overlay.rs` from the last 1000 frames).
  - **Scripted scenarios** (`crates/app/src/dev_scenarios.rs`), picked with `ORBIT_SCENARIO`:
    - `idle`;
    - `open:<note>`;
    - `type:<note>`, which types ten keys a second.

**Numbers**
- `scripts/measure.sh <binary> <vault> <scenario> [warm-up s] [measure s] [work dir]` runs Orbit on its own config, state, cache and runtime folders, so it never meets a running Orbit. It reports:
  - CPU ms per second, from `/proc/<pid>/stat` utime + stime;
  - RSS, with its anonymous and file-backed parts, peak RSS (`VmHWM`) and threads, all from `/proc/<pid>/status`. Orbit makes itself non-dumpable (`harden_process`, `crates/app/src/main.rs`), which puts `smaps` and `smaps_rollup` out of reach even of the same user [measured];
  - the indexing time, and how often `window.changed_on_disk` and `search.update` ran, from the log.
- The search index and the session live in the vault's own `.vault/` folder. So a freshly generated vault gives a cold start, and one Orbit has opened before gives a warm start, with the tabs it left open.
- **Spans:** spans log their busy and idle time when they close (`with_span_events(FmtSpan::CLOSE)`, `crates/app/src/logging.rs`; [tracing-subscriber](https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/fmt/struct.Layer.html#method.with_span_events)). The spans are:
  - `vault.open`;
  - `search.index`;
  - `search.update`;
  - `protect.unlock`;
  - `window.changed_on_disk`;
  - `editor.edited`, at trace.

**Fixture vault**
- Generate it with `scripts/make-fixture-vault.py <folder>`. It's deterministic:
  - 5,000 notes of about 2 KB in 50 folders, with wikilinks and tags;
  - `Long.md`, about 200 KB;
  - `Big.md`, about 3 MB;
  - `Diagrams.md`, with 10 Mermaid charts;
  - `Private/`, a folder to protect.

**Profilers**
- `perf` is installed [measured]. Use `perf record -g --call-graph dwarf -p <pid>`. At `kernel.perf_event_paranoid = 2`, only user space can be profiled, for your own processes.
- These are in the repos [measured], and are worth installing (`sudo pacman -S samply heaptrack gdb`):
  - `samply record <binary>` opens the profile in the Firefox Profiler. It needs paranoid at 1 or below ([samply](https://github.com/mstange/samply)).
  - `heaptrack <binary>` finds peak memory and allocation churn ([heaptrack](https://github.com/KDE/heaptrack)).

## Results

The fixture vault, at the window's default size, with paper grain on. Each row is one `scripts/measure.sh` run.

| Build / change | Scenario | CPU ms/s | RSS MB | anonymous MB | file-backed MB | peak RSS MB | threads | indexing ms | changed_on_disk | search updates |
|---|---|---|---|---|---|---|---|---|---|---|
| Baseline (`a864161`) | idle, cold start | 10822 | 428 | 310 | 125 | 465 | 48 | 43 | 312 | 85 |
| Baseline | idle, warm start | 10807 | 402 | 280 | 120 | 449 | 47 | – | 312 | 85 |
| Baseline | open a 2 KB note | 1571 | 425 | 302 | 123 | 458 | 47 | – | 3 | 4 |
| Baseline | type into a 2 KB note | 1618 | 448 | 324 | 123 | 466 | 47 | – | 4 | 5 |
| Baseline | type into `Long.md` (200 KB) | 1631 | 457 | 332 | 123 | 465 | 48 | – | 4 | 4 |
| Baseline | open `Big.md` (3 MB) | 1633 | 481 | 357 | 124 | 483 | 47 | – | 4 | 4 |
| Watcher fix | idle, cold start | 0.7 | 257 | 130 | 124 | 257 | 48 | 269 | 0 | 0 |
| Watcher fix | idle, warm start | 1.3 | 172 | 53 | 118 | 172 | 46 | 39 | 0 | 0 |
| Watcher fix | open a 2 KB note | 22 | 198 | 75 | 123 | 198 | 46 | 39 | 0 | 0 |
| Watcher fix | type into a 2 KB note | 134 | 220 | 97 | 123 | 220 | 46 | 40 | 0 | 0 |
| Watcher fix | type into `Long.md` (200 KB) | 1150 | 518 | 394 | 123 | 518 | 46 | 43 | 0 | 0 |
| Watcher fix | open `Big.md` (3 MB) | 175 | 1035 | 912 | 123 | 1090 | 47 | 46 | 0 | 0 |
| Catch-up delay alone | idle, cold start | 0.0 | 256 | 128 | 125 | 256 | 50 | 275 | 0 | 0 |
| Catch-up delay alone | type into a 2 KB note | 219 | 202 | 78 | 124 | 202 | 46 | 38 | 0 | 0 |
| Catch-up delay alone | type into `Long.md` (200 KB) | 1022 | 472 | 347 | 124 | 472 | 46 | 40 | 0 | 0 |
| Catch-up delay + cached preview | idle, cold start | 0.0 | 242 | 114 | 125 | 242 | 49 | 270 | 0 | 0 |
| Catch-up delay + cached preview | `Long.md` open, idle | 15 | 287 | 163 | 124 | 287 | 47 | 40 | 0 | 0 |
| Catch-up delay + cached preview | type into a 2 KB note | 222 | 300 | 177 | 123 | 300 | 47 | 42 | 0 | 0 |
| Catch-up delay + cached preview | type into `Long.md` (200 KB) | 380 | 491 | 367 | 123 | 491 | 47 | 40 | 0 | 0 |

Notes:
- The scenarios ran in one work dir, in this order: two idle runs, then the others. Each run restores the tabs the last one left open, so `open Big.md` also has the 2 KB note and `Long.md` open.
- The baseline's `changed_on_disk` and `search updates` columns are per run, taken as differences between successive runs: the log was shared, and the counts were read cumulatively before `measure.sh` was fixed.
- Indexing shows "–" where only the first run's line was read.

### Found by measuring: reading a file counted as changing it [measured, code]

This is the main cause of the CPU use you saw: about 11 cores at idle on the fixture vault.
- notify watches with `WatchMask::OPEN` (notify-8.2.1 `src/inotify.rs:429`), so opening a file to read it raises an event.
- `notify-debouncer-mini` passes on every event's path without its kind.
- So indexing and opening notes were reported as changes. Orbit then re-read those notes, the reads raised more events, and the loop never stopped:
  - `search.update` ran with thousands of changed notes, for 7–9 s each;
  - `window.changed_on_disk` ran 312 times in 50 s.
- **The fix** (`crates/orbit-fs/src/lib.rs`, `report_changes`):
  - uses `notify` directly, with Orbit's own debounce;
  - drops access events, except a close after writing;
  - drops Orbit's own writes while the file is still as Orbit left it (its size and change time just after the write), so an autosave no longer comes back as a change by another app.
- Tests:
  - `crates/orbit-core/tests/outside_changes.rs`: `reading_a_note_isnt_reported_as_changing_it` and `the_watcher_doesnt_report_orbits_own_saves_back`;
  - both fail on the old watcher.

### Found by profiling: typing into a long note re-laid out its whole preview [measured]

Profiling typing into `Long.md` (`perf record -e task-clock --call-graph dwarf,65528 --user-callchains`, with Orbit launched under perf) showed where the time went:
- **Layout:** about 26% in taffy flexbox and block layout.
- **Text shaping:** about 25% in harfrust and cosmic-text.

The cause:
- Every keystroke, and every caret blink, notifies the editor's view.
- The preview was part of that view, so its thousands of markdown block nodes were laid out again on each one.
- Delaying the preview's text until typing pauses (150 ms, or 1 s at most) cut only 11% on its own, since the preview was still laid out every frame.

The fix moves the preview into a view of its own, `PreviewPane` (`crates/app/src/preview.rs`), which the editor embeds with `Entity::cached` (gpui-pre `view.rs:241-252`). It's laid out again only when its text changes, and the delay means that's at most a few times a second while typing. Together the two changes cut typing into `Long.md` from 1,022 to 380 ms/s.

Notes on profiling:
- `perf` can't attach to a running Orbit: it makes itself non-dumpable. Launch Orbit under perf instead.
- At `perf_event_paranoid = 2`, call graphs need `--user-callchains`.

## Optimizations, ranked by impact against effort

| # | Change | Where | Status |
|---|---|---|---|
| 0 | Reading a file isn't a change (the watcher loop above) | `orbit-fs/src/lib.rs` | **done**: idle CPU 10,800 → 1 ms/s, RSS 400 → 170 MB |
| 1 | Ignore Orbit's own saves coming back through the watcher | `orbit-fs/src/lib.rs` (in the watcher, not the window) | **done**: `changed_on_disk` while typing, 4–5 per run → 0 |
| 2 | Refresh the tabs' links only when notes are added, removed or renamed | `window.rs` `changed_on_disk` | set aside: `changed_on_disk` no longer runs while typing or idle; it matters only for outside changes (sync, git) |
| 3 | Walk the tree once per refresh (`has_notes` from the same nodes) | `window.rs` `refresh_tree`, `orbit-core/src/lib.rs` | set aside, for the same reason |
| 4 | Delay the preview, Outline and word count until typing pauses | `note_editor.rs` `catch_up_later` | **done**, with 6 |
| 5 | Tantivy writer budget at the minimum (15 MB) | `orbit-index/src/lib.rs` | dropped: on three cold starts each, peak RSS was 256/256/266 MB at 15 MB against 251/253/253 MB at 50 MB, and indexing 273–279 against 271–274 ms. The fixture's notes never fill even 15 MB before a commit |
| 6 | The preview as a cached view of its own | `preview.rs` `PreviewPane`, `note_editor.rs` | **done**: typing into a 200 KB note, 1,150 → 380 ms/s |
| 7 | The GPU on hybrid laptops | README | **not needed**: GPUI prefers the GPU the compositor renders on, ahead of discrete over integrated (gpui-pre-wgpu `wgpu_context.rs:582-593`). Here it picks the Intel UHD over the RTX A2000 [measured, `ORBIT_LOG=info` shows "Selected GPU adapter"]. The README documents `ZED_DEVICE_ID` for overriding it. Still open: wgpu enumerates every adapter, so NVIDIA's Vulkan driver is loaded into the process, which is likely part of the roughly 125 MB of file-backed RSS |

## What's left

- **Very large notes:** `Big.md` (3 MB) takes RSS to about 1 GB in the Kit editor's soft-wrap layout. Notes over 2 MB already open as source only. Measure the editor with heaptrack before changing anything.
- **Typing into a small note** costs about 220 ms/s, mostly the editor's own redraw on each key.
- **Caret blinks:** with a note open and idle, the caret's blink costs about 15–22 ms/s. Caching the sidebar and right panel too could cut that further.
- **Outside changes:** fixes 2 and 3 would help when many notes change outside Orbit at once (a sync, a git pull).

## Sources

- [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)
- [samply](https://github.com/mstange/samply)
- [heaptrack](https://github.com/KDE/heaptrack)
- [tracing-subscriber `with_span_events`](https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/fmt/struct.Layer.html#method.with_span_events)
- Pinned library sources, under `~/.cargo/registry/src/index.crates.io-*/`:
  - `gpui-pre-0.3.9/src/{view.rs, window.rs, debug_overlay.rs}`;
  - `gpui-pre-wgpu-0.3.9/src/wgpu_context.rs`;
  - `gpui-base-0.8.0/src/{input/base/state.rs, input/base/blink_cursor.rs, text/state.rs}`;
  - `tantivy-0.27.0/src/indexer/index_writer.rs`.
