# Performance: measuring and optimizing Orbit's CPU and memory

- **Researched:** 2026-10-28
- **Question:** Orbit uses a lot of CPU and memory while it runs. How should we measure that, where does it come from, and what should be done about it?
- **Method:** each claim is marked as one of:
  - **[measured]**, on the development machine: 16 CPUs, 31.8 GB of RAM, Intel UHD and NVIDIA RTX A2000 Mobile GPUs, CachyOS, niri on Wayland;
  - **[code]**, read from this repo or the pinned library sources in `~/.cargo/registry`, cited as `file:line`;
  - **[docs]**, from primary documentation, linked.

## TL;DR

- **The main cause was a feedback loop in the file watcher** [measured, code].
  - Reading a note (indexing it, opening it) was reported as changing it, so Orbit read it again, without end.
  - That cost **about 10–11 cores at idle** on the fixture vault, and 400–480 MB of RSS.
  - Fixed: idle CPU is now about 0 ms/s, and a warm start settles at about 180 MB.
- **Typing into a long note re-laid out its whole preview on every key** [measured].
  - The preview is now a cached view that catches up after a pause.
  - Typing into a 200 KB note went from about 1,000 to about 300 CPU ms/s.
- **Orbit's own autosave came back through the watcher as a change by another app** [code, test]. Fixed in the watcher, and proven by a test that fails on the old code on a real disk.
- **Measure an optimized build** [measured]:
  - Before this note only a debug build existed. The dev profile has no optimization except argon2 and blake2 (`Cargo.toml`, `[profile.dev.package.*]`).
  - Use `cargo build --profile profiling -p app --features dev-profiler`.
- **Not causes** [measured]:
  - **Tantivy's 50 MB writer budget:** 15 MB changed nothing measurable.
  - **The GPU:** GPUI already draws with the GPU the compositor uses, the Intel UHD here (gpui-pre-wgpu `wgpu_context.rs:582-593`).
- **What remains:**
  - A 3 MB note takes RSS to about 1 GB, in the editor's layout.
  - Typing into a small note costs about 120 ms/s.

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
    - `type:<note>`, which types bursts of 30 keys at ten a second, each followed by a 2 s pause, past the 1.5 s autosave delay, so saves happen while it's measured.

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
- `perf` is installed [measured].
  - It can't attach to a running Orbit, because Orbit makes itself non-dumpable. Launch Orbit under perf instead.
  - At `kernel.perf_event_paranoid = 2`, call graphs need user-only call chains. The command that worked: `perf record -e task-clock --call-graph dwarf,65528 --user-callchains -F 49 -- target/profiling/orbit <vault>`, then `perf report --children --sort symbol`.
- These are in the repos [measured], and are worth installing (`sudo pacman -S samply heaptrack gdb`):
  - `samply record <binary>` opens the profile in the Firefox Profiler. It needs paranoid at 1 or below ([samply](https://github.com/mstange/samply)).
  - `heaptrack <binary>` finds peak memory and allocation churn ([heaptrack](https://github.com/KDE/heaptrack)).

## Results

Three builds of the same tree, measured the same way:
- **A, before:** the old watcher and the old preview, which is `d8d9ccb` with orbit-fs from `a864161`, and note_editor.rs and preview.rs from `4a41dee`;
- **B:** the watcher fix only;
- **C, now:** both fixes (`d8d9ccb`).

Each scenario ran on a freshly generated fixture vault, so no tabs were carried over from earlier runs. The warm start is a second start of that same vault. Each run is 20 s to settle, then 30 s measured (`scripts/measure.sh`), at the window's default size, with paper grain on [measured].

| Scenario | Build | CPU ms/s | RSS MB | anonymous MB | file-backed MB | peak RSS MB | threads | indexing ms | changed_on_disk | search updates |
|---|---|---|---|---|---|---|---|---|---|---|
| idle, cold start | A | 10128 | 460 | 332 | 88 | 483 | 51 | 339 | 302 | 69 |
| | B | 0.3 | 248 | 120 | 88 | 248 | 49 | 281 | 0 | 0 |
| | C | 0.0 | 259 | 131 | 88 | 259 | 48 | 271 | 0 | 0 |
| idle, warm start | A | 11355 | 459 | 334 | 88 | 480 | 49 | 46 | 291 | 75 |
| | B | 0.0 | 179 | 54 | 88 | 179 | 49 | 39 | 0 | 0 |
| | C | 0.0 | 178 | 54 | 88 | 178 | 50 | 37 | 0 | 0 |
| 2 KB note open, idle | A | 1650 | 478 | 348 | 90 | 498 | 50 | 296 | 3 | 3 |
| | B | 14 | 279 | 147 | 90 | 279 | 49 | 285 | 0 | 0 |
| | C | 14 | 287 | 154 | 90 | 287 | 49 | 283 | 0 | 0 |
| typing into a 2 KB note | A | 1542 | 498 | 369 | 90 | 507 | 50 | 294 | 4 | 4 |
| | B | 149 | 296 | 163 | 90 | 296 | 49 | 291 | 0 | 0 |
| | C | 124 | 288 | 156 | 90 | 288 | 50 | 287 | 0 | 0 |
| typing into `Long.md` (200 KB) | A | 10572 | 641 | 507 | 90 | 659 | 48 | 312 | 292 | 59 |
| | B | 1014 | 549 | 416 | 90 | 549 | 49 | 285 | 0 | 0 |
| | C | 294 | 491 | 359 | 90 | 491 | 48 | 280 | 0 | 0 |
| `Big.md` (3 MB) open | A | 10116 | 1382 | 1251 | 90 | 1389 | 50 | 310 | 237 | 69 |
| | B | 15 | 1044 | 912 | 90 | 1051 | 49 | 279 | 0 | 0 |
| | C | 15 | 1053 | 922 | 90 | 1060 | 49 | 282 | 0 | 0 |
| `Diagrams.md` (10 Mermaid) open | A | 9888 | 445 | 310 | 90 | 476 | 50 | 374 | 289 | 52 |
| | B | 3.3 | 271 | 129 | 90 | 281 | 49 | 274 | 0 | 0 |
| | C | 14 | 264 | 123 | 90 | 271 | 49 | 270 | 0 | 0 |

How to read it:
- **The loop isn't always running in A.** In the "2 KB note open" and "typing into a 2 KB note" runs it had settled to about 1.6 cores. In the others it ran at 10–11 cores.
- **Orbit's own saves (fix 1) aren't shown by these counts.** In A the read loop's changes swamp them. The proof is the test `the_watcher_doesnt_report_orbits_own_saves_back`, which fails on the old watcher on a real disk and passes now.
- **The preview fix is measured by B against C:** typing into `Long.md` drops from 1,014 to 294 ms/s. The note-open scenarios differ by about 10 ms/s, which is noise from the caret blink.
- **Not measured:**
  - unlocking a protected folder, since the scenarios can't type a passphrase;
  - span durations (`measure.sh` only counts spans; the durations are in the log);
  - a debug build.

Earlier exploratory runs, from the session that found these causes, gave the same picture. They aren't in this table, because their runs shared a vault and so carried tabs over between scenarios.

### Found by measuring: reading a file counted as changing it [measured, code]

This is the main cause of the CPU use you saw: about 11 cores at idle on the fixture vault.
- notify watches with `WatchMask::OPEN` (notify-8.2.1 `src/inotify.rs:429`), so opening a file to read it raises an event.
- `notify-debouncer-mini` passes on every event's path without its kind.
- So indexing and opening notes were reported as changes. Orbit then re-read those notes, the reads raised more events, and the loop never stopped:
  - `search.update` ran with thousands of changed notes, for 7–9 s each (its span's `time.busy`, in the first measured run's log) [measured];
  - `window.changed_on_disk` ran 312 times in 50 s.
- **The fix** (`crates/orbit-fs/src/lib.rs`, `report_changes`):
  - uses `notify` directly, with Orbit's own debounce;
  - drops access events, except a close after writing;
  - drops Orbit's own writes, so an autosave no longer comes back as a change by another app.
    - A write is marked before it starts, and recorded with the file's size, change time and inode once done.
    - The watcher decides which paths are Orbit's when it sends a batch, since on a real disk its events can arrive before the save returns (the folder's fsync).
- Tests:
  - `crates/orbit-core/tests/outside_changes.rs`: `reading_a_note_isnt_reported_as_changing_it` and `the_watcher_doesnt_report_orbits_own_saves_back`;
  - both run on the build's disk (`CARGO_TARGET_TMPDIR`), not a RAM-backed `/tmp`;
  - both fail on the old watcher.

### Found by profiling: typing into a long note re-laid out its whole preview [measured]

Profiling typing into `Long.md` (`perf record -e task-clock --call-graph dwarf,65528 --user-callchains`, with Orbit launched under perf) showed where the time went:
- **Layout:** about 26% in taffy flexbox and block layout.
- **Text shaping:** about 25% in harfrust and cosmic-text.

The cause:
- Every keystroke, and every caret blink, notifies the editor's view.
- The preview was part of that view, so its thousands of markdown block nodes were laid out again on each one.
- Delaying the preview's text until typing pauses (150 ms, or 1 s at most) cut only 11% on its own, since the preview was still laid out every frame.

The fix moves the preview into a view of its own, `PreviewPane` (`crates/app/src/preview.rs`), which the editor embeds with `Entity::cached` (gpui-pre `view.rs:241-252`). It's laid out again only when its text changes, and the delay means that's at most a few times a second while typing. Together the two changes cut typing into `Long.md` from 1,014 to 294 ms/s (B against C above).

Notes on profiling:
- `perf` can't attach to a running Orbit: it makes itself non-dumpable. Launch Orbit under perf instead.
- At `perf_event_paranoid = 2`, call graphs need `--user-callchains`.

## Optimizations, ranked by impact against effort

| # | Change | Where | Status |
|---|---|---|---|
| 0 | Reading a file isn't a change (the watcher loop above) | `orbit-fs/src/lib.rs` | **done**: warm idle CPU 11,355 → 0 ms/s, RSS 459 → 179 MB (A against B) |
| 1 | Ignore Orbit's own saves coming back through the watcher | `orbit-fs/src/lib.rs` (in the watcher, not the window) | **done**, proven by its test. The load numbers can't separate it from fix 0 |
| 2 | Refresh the tabs' links only when notes are added, removed or renamed | `window.rs` `changed_on_disk` | set aside: `changed_on_disk` no longer runs while typing or idle; it matters only for outside changes (sync, git) |
| 3 | Walk the tree once per refresh (`has_notes` from the same nodes) | `window.rs` `refresh_tree`, `orbit-core/src/lib.rs` | set aside, for the same reason |
| 4 | Delay the preview, Outline and word count until typing pauses | `note_editor.rs` `catch_up_later` | **done**, with 6 |
| 5 | Tantivy writer budget at the minimum (15 MB) | `orbit-index/src/lib.rs` | dropped: on three cold starts each, peak RSS was 256/256/266 MB at 15 MB against 251/253/253 MB at 50 MB, and indexing 273–279 against 271–274 ms. The fixture's notes never fill even 15 MB before a commit |
| 6 | The preview as a cached view of its own | `preview.rs` `PreviewPane`, `note_editor.rs` | **done**, with 4: typing into a 200 KB note, 1,014 → 294 ms/s (B against C). Fix 4 alone cut only about 11% in an earlier run. Fix 6 was narrowed to the preview; the sidebar and right panel aren't cached |
| 7 | The GPU on hybrid laptops | README | **not needed**: GPUI prefers the GPU the compositor renders on, ahead of discrete over integrated (gpui-pre-wgpu `wgpu_context.rs:582-593`). Here it picks the Intel UHD over the RTX A2000 [measured, `ORBIT_LOG=info` shows "Selected GPU adapter"]. The README documents `ZED_DEVICE_ID` for overriding it. Still open: wgpu enumerates every adapter, so NVIDIA's Vulkan driver is loaded into the process, which is likely part of the roughly 125 MB of file-backed RSS |

## What's left

- **Very large notes:** `Big.md` (3 MB) holds about 1 GB of RSS, 920 MB of it anonymous, at 15 ms/s.
  - The likely place is the Kit editor's display and wrap maps (not profiled yet).
  - Notes over 2 MB already open as source only.
  - Measure the editor with heaptrack before changing anything.
- **Typing into a small note** costs about 120 ms/s, mostly the editor's own redraw on each key.
- **Caret blinks:** a note open and idle costs about 14 ms/s. Caching the sidebar and right panel too could cut that further.
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
