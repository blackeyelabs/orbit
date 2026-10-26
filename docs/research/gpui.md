# GPUI: research notes for building a Rust desktop app

- **Researched:** 2026-09-25
- **Question:** How do we build a desktop app with GPUI (Zed's UI framework), and what are the pros and cons for `orbit` (Rust 2024, Linux/CachyOS dev machine)?
- **Snapshot used:** `zed-industries/zed` `main` at commit
  [`7ea077c78b865d17854cee812bb775dbe64a925a`](https://github.com/zed-industries/zed/tree/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui)
  (2026-09-25). Paths like `crates/gpui/README.md` below point at that commit unless a tag is named.
- **Method:** Primary sources only: the Zed repo (source, README, examples, Cargo manifests, PRs), crates.io API, docs.rs, gpui.rs, zed.dev blog, and the official repos of the other libraries. Anything I couldn't confirm is marked **(unverified)**.

---

## TL;DR

- GPUI is Zed's own GPU-accelerated UI framework, licensed Apache-2.0. Its README calls it "a hybrid immediate and retained mode, GPU accelerated, UI framework for Rust". It's **pre-1.0**, and the README warns that "there will often be breaking changes between versions" ([crates/gpui/README.md](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/README.md)).
- **The crates.io release is almost a year behind `main`.** The newest official `gpui` on crates.io is **0.2.2, published 2025-10-22** ([crates.io API](https://crates.io/api/v1/crates/gpui)). Since then, `main` has replaced the Linux renderer (Blade → wgpu, [PR #46758](https://github.com/zed-industries/zed/pull/46758)), moved platform code into a separate `gpui_platform` crate ([PR #49277](https://github.com/zed-industries/zed/pull/49277)), and added AccessKit accessibility ([PR #56065](https://github.com/zed-industries/zed/pull/56065)). `gpui_platform` is **not on crates.io**: the workspace sets `publish = false` ([Cargo.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/Cargo.toml)). In practice, **a new project should use a git dependency pinned to a specific `rev`**, or the third-party `gpui-pre` snapshots (see §2.1).
- There are **no built-in widgets** such as a text input or button. You get primitives: `div`, text, `img`, `svg`, `canvas`, `list`, `uniform_list`, and so on. The official `input` example builds a text field from scratch in ~780 lines. Zed's own `ui` component crate is **GPL-3.0** and unpublished. The practical widget layer is Longbridge's **GPUI Kit / gpui-component** (Apache-2.0, 75+ components).
- **Best fit:** keyboard-driven, high-performance, "developer-tool-like" apps where you're willing to track an unstable API. **Poor fit:** apps that need a stable API, a rich built-in widget set, mature accessibility, or mobile support.

---

## 1. What GPUI is

### 1.1 Architecture: three "registers"

The README describes three layers ([crates/gpui/README.md, "The Big Picture"](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/README.md)):

1. **State management with `Entity`s.** Entities are "owned by GPUI and are only accessible through an owned smart pointer similar to an `Rc`."
2. **High-level declarative UI with views.** "A view is simply an `Entity` that can be rendered, by implementing the `Render` trait. At the start of each frame, GPUI will call this render method on the root view." Views build a tree of elements and style them "with a tailwind-style API".
3. **Low-level imperative UI with `Element`s.** Elements "have total control over how they and their child elements are rendered". Use them for things like large lists or custom layout for a code editor.

Every service is reached through a **context** (`cx`) ([crates/gpui/docs/contexts.md](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/docs/contexts.md)):

- `App`: the root context. It owns all entity data.
- `Context<T>`: scoped to one entity. It adds `notify` and `emit` and dereferences to `App`.
- `AsyncApp` / `AsyncWindowContext`: `'static` contexts you can hold across `.await`. Calls through them are fallible.
- `TestAppContext`: used in tests.
- `Window` is **not** a context. It holds a root view and window state.

**Ownership model** ([crates/gpui/src/_ownership_and_data_flow.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src/_ownership_and_data_flow.rs); background in the Zed blog post [Ownership and data flow in GPUI](https://zed.dev/blog/gpui-ownership)):

- "Every model or view in the application is actually owned by a single top-level object called the `App`."
- `cx.new(|cx| T)` returns an `Entity<T>` handle. You mutate through `entity.update(cx, |state, cx| …)` and call `cx.notify()` to trigger observers and re-render.
- Reactivity comes in two forms:
  - `cx.observe(&entity, …)`, which fires on `notify`.
  - `cx.subscribe(&entity, …)`, which receives typed events from `cx.emit(...)` on types that implement `EventEmitter<E>`.
  - Both return a `Subscription`, which you can `.detach()` or keep and drop later.

**Hybrid immediate/retained, in practice.** `render()` rebuilds a lightweight element tree each frame the view is dirty, which is the immediate-mode part. State lives in retained `Entity`s owned by `App`, which is the retained part. Before GPUI 2, the Zed team's 2023 rendering deep-dive ([Leveraging Rust and the GPU to render user interfaces at 120 FPS](https://zed.dev/blog/videogame), Antonio Scandurra, 2023-03-07) described GPUI 1's element tree and GPU "Scene". GPUI 2 has been in production in Zed since January 2024 ([GPUI 2 is now in production](https://zed.dev/blog/gpui-2-on-preview)). The 2023 post's layout description ("constraints flow down and sizes flow up") is pre-Taffy and outdated.

### 1.2 Elements available in core

`crates/gpui/src/elements/` contains `div`, `text`, `img`, `svg`, `canvas`, `list`, `uniform_list`, `anchored`, `deferred`, `animation`, `image_cache`, `surface`, and `container_query` ([source dir](https://github.com/zed-industries/zed/tree/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src/elements)). `div` is the "all purpose swiss-army knife" (README).

### 1.3 Rendering backends per platform (on `main` today)

| Platform | GPU backend | Windowing | Text | Source |
|---|---|---|---|---|
| macOS | **Metal** (`metal_renderer.rs`, `shaders.metal` in the `gpui_apple` crate) | AppKit (`gpui_macos`) | CoreText via Zed's `font-kit` fork. README: without the `font-kit` feature, glyphs aren't rendered | [crates/gpui_apple/src](https://github.com/zed-industries/zed/tree/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui_apple/src), [README](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/README.md) |
| Linux / FreeBSD | **wgpu** restricted to `Backends::VULKAN \| Backends::GL` | **Wayland and/or X11**, chosen by Cargo features `wayland` / `x11`. Zed "pick[s] whichever we can find at runtime" | cosmic-text + swash (`cosmic_text_system.rs`), with font-kit/fontconfig for font discovery | [gpui_wgpu/src/wgpu_context.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui_wgpu/src/wgpu_context.rs), [gpui_linux/Cargo.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui_linux/Cargo.toml), [docs/src/development/linux.md](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/docs/src/development/linux.md) |
| Windows | **DirectX 11** (`directx_renderer.rs`, HLSL shaders) | Win32 | DirectWrite | [crates/gpui_windows/src](https://github.com/zed-industries/zed/tree/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui_windows/src), [Windows When? Windows Now](https://zed.dev/blog/zed-for-windows-is-here) (2025-10-15: "uses DirectX 11 for rendering, and DirectWrite for text rendering") |
| Web (wasm) | wgpu with WebGPU or WebGL2 | browser canvas (`gpui_web`) | cosmic-text plus Canvas fallback for CJK/emoji | [crates/gpui/examples/README.md](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/examples/README.md). Needs nightly Rust and Trunk per that README |

History:

- Until early 2026, Linux used the **Blade** graphics library. The `gpui-v0.2.2` tag still has `blade-graphics` deps and `taffy = "=0.9.0"` ([gpui-v0.2.2 Cargo.toml](https://github.com/zed-industries/zed/blob/gpui-v0.2.2/crates/gpui/Cargo.toml)).
- [PR #46758](https://github.com/zed-industries/zed/pull/46758) (merged 2026-02-13) removed Blade and reimplemented the Linux renderer on wgpu. The PR description says "The blade graphics library is a mess and causes several issues for both Zed users as well as other 3rd party apps using GPUI", and cites NVIDIA and Wayland-compositor freezes.
- **So the crates.io 0.2.2 build on Linux uses the old Blade renderer, while `main` uses wgpu.**

### 1.4 Styling and layout

- **Styling** is a Tailwind-like fluent API on `Styled` elements: `.flex().flex_col().gap_3().bg(rgb(..)).size(px(500.)).rounded_md().shadow_lg().text_xl()`. See [examples/hello_world.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/examples/hello_world.rs) and `src/styled.rs`.
- **Layout** uses **Taffy** (flexbox and CSS grid), pinned as `taffy = "=0.13.0"` ([crates/gpui/Cargo.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/Cargo.toml)). The `grid_layout` example shows "CSS-grid-style layout" ([examples/README.md](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/examples/README.md)).

### 1.5 Async executors

- The README describes "an async executor that is integrated with the platform's event loop."
- `src/executor.rs` defines `BackgroundExecutor`, whose `spawn` takes `Send` futures and runs them on a thread pool, and `ForegroundExecutor`, whose `spawn` takes non-`Send` futures and runs them on the main thread.
- `Context::spawn` / `spawn_in(window, …)` give the future an async context so it can update entities after awaiting ([src/executor.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src/executor.rs), [src/app/context.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src/app/context.rs)).
- Note that this isn't Tokio. Tokio-based libraries (e.g. reqwest) need their own runtime or an adapter. **(unverified: I found no official guidance on this.)**

### 1.6 Actions and keybindings

From [crates/gpui/docs/key_dispatch.md](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/docs/key_dispatch.md): "GPUI is designed for keyboard-first interactivity." Actions are user-defined structs, declared with `#[gpui::action]` or `actions!(namespace, [A, B])`. They're bound to keystrokes inside a *key context* and handled with `.on_action(...)`. The `input` example uses exactly this pattern:

```rust
// crates/gpui/examples/input.rs (at 7ea077c)
cx.bind_keys([
    KeyBinding::new("backspace", Backspace, None),
    KeyBinding::new("left", Left, None),
    KeyBinding::new("shift-left", SelectLeft, None),
    // ...
]);
// in render():
div().on_action(cx.listener(Self::backspace))
```

### 1.7 Accessibility (new in 2026)

- GPUI now depends on **AccessKit** (`accesskit` 0.24, with `accesskit_macos`, `accesskit_windows`, and `accesskit_unix`) ([Cargo.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/Cargo.toml), [gpui_linux/Cargo.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui_linux/Cargo.toml)). It landed in [PR #56065](https://github.com/zed-industries/zed/pull/56065) on 2026-05-27, which "*ONLY* adds AccessKit support to GPUI, and doesn't touch Zed".
- The guide [src/_accessibility.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src/_accessibility.rs) explains the rules. An element must have both an `.id(...)` and a `.role(Role::…)` to be exposed. Stable global IDs across frames matter. A `text!` macro derives IDs from the source location.
- **This is opt-in and manual:** a plain `div` isn't exposed to screen readers.
- Zed's own Windows screen-reader issue ([#41138](https://github.com/zed-industries/zed/issues/41138), "Windows: Screen reader accessibility missing completely") is still **open**.

---

## 2. Getting started

### 2.1 Adding the dependency: three options

**Option A: official crates.io release (old).**

```toml
gpui = "0.2.2"
```

- Latest official version is 0.2.2 (2025-10-22). Owners are Zed staff and the `zed-industries` team ([crates.io](https://crates.io/crates/gpui), [owners API](https://crates.io/api/v1/crates/gpui/owners)).
- Its API matches the `gpui-v0.2.2` tag: `Application::new().run(...)`, with no `gpui_platform` ([hello_world.rs @ gpui-v0.2.2](https://github.com/zed-industries/zed/blob/gpui-v0.2.2/crates/gpui/examples/hello_world.rs)).
- It still uses Blade on Linux and predates AccessKit.
- docs.rs documents this version ([docs.rs/gpui](https://docs.rs/gpui/latest/gpui/)).
- The `main` README's crates.io instructions (`gpui_platform = { version = "*" … }`) **don't work today**, because `gpui_platform` isn't published: my crates.io lookup of `gpui_platform` returned no crate, and the workspace has `publish = false`.

**Option B: git dependency pinned to a commit (recommended if you want official code).**

```toml
[dependencies]
gpui = { git = "https://github.com/zed-industries/zed", rev = "7ea077c78b865d17854cee812bb775dbe64a925a" }
gpui_platform = { git = "https://github.com/zed-industries/zed", rev = "7ea077c78b865d17854cee812bb775dbe64a925a", features = ["wayland", "x11"] }
```

- Always pin a `rev`, because `main` changes daily. The commit log shows several `gpui:` commits per week, e.g. 2026-09-16…24 ([commits on crates/gpui](https://github.com/zed-industries/zed/commits/main/crates/gpui)).
- Feature guidance from the README: macOS needs `font-kit`. Linux/FreeBSD needs `wayland` and/or `x11`, which "also compile the renderer and text system". Windows needs no features.
- Zed pins its toolchain to Rust **1.98.1** ([rust-toolchain.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/rust-toolchain.toml)). The README says "You'll also need to use the latest version of stable Rust."
- Downsides:
  - The first fetch clones the whole Zed monorepo into `~/.cargo/git`.
  - Several transitive deps are Zed forks pulled by git: `zed-font-kit` and `zed-xim` ([gpui/Cargo.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/Cargo.toml), [gpui_linux/Cargo.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui_linux/Cargo.toml)).
- **Local build check (this machine):** see §2.3.

**Option C: `gpui-pre` snapshot crates (third-party).**

- `gpui-pre` 0.3.6 (2026-09-21), "gpui-pre snapshot of zed@bcf6582", is published by **Jason Lee (huacnlee, Longbridge)**, not by Zed ([crates.io API](https://crates.io/api/v1/crates/gpui-pre), [owners](https://crates.io/api/v1/crates/gpui-pre/owners)).
- GPUI Kit depends on it via package renaming, e.g. `gpui = { package = "gpui-pre", version = "=0.3.6" }` and `gpui_platform = { package = "gpui-pre-platform", version = "=0.3.6", … }`. Its Cargo.toml warns: "any snapshot may change GPUI's API" ([gpui-kit Cargo.toml](https://github.com/longbridge/gpui-kit/blob/main/Cargo.toml)).
- It's convenient: no git clone, and it's versioned on crates.io. But it's an unofficial channel.

### 2.2 Linux system dependencies (CachyOS / Arch)

- The GPUI README on `main` only lists macOS (Xcode) requirements.
- Zed's `script/linux` installs these on Arch: `gcc clang musl cmake alsa-lib fontconfig glib2 libva wayland libgit2 libxcb libxkbcommon-x11 openssl zstd pkgconf sqlite pipewire xdg-desktop-portal jq git`. That list covers the **whole Zed editor**, not just GPUI ([script/linux](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/script/linux)).
- **GPUI-relevant subset (inferred from `gpui_linux`/`gpui_wgpu` deps, not an official list):**
  - Required: `wayland`, `libxcb`, `libxkbcommon` + `libxkbcommon-x11`, `fontconfig`/`freetype2`, a Vulkan loader + driver (`vulkan-icd-loader` plus e.g. `vulkan-radeon`/`nvidia-utils`), `pkgconf`, and `clang`/`cmake` for build scripts.
  - Also needed: `xdg-desktop-portal` for file dialogs via `ashpd`, and a Secret Service provider for `oo7`, which `gpui_linux` says is "Always used".
  - `pipewire` is only needed for the `screen-capture` feature.
- On this machine, `wayland`, `libxkbcommon(-x11)`, `libxcb`, `vulkan-icd-loader`, `fontconfig`, `freetype2`, `pkgconf`, and `clang` are already installed (checked with `pacman -Q`).

### 2.3 Minimal "hello world" (official example)

From [crates/gpui/examples/hello_world.rs @ 7ea077c](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/examples/hello_world.rs). It's abridged: the example also draws six colored squares, loads bundled fonts through `example_support`, and has a wasm entry point.

```rust
use gpui::{
    App, Bounds, Context, SharedString, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    rgb, size,
};
use gpui_platform::application;

struct HelloWorld {
    text: SharedString,
}

impl Render for HelloWorld {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size(px(500.0))
            .justify_center()
            .items_center()
            .shadow_lg()
            .border_1()
            .border_color(rgb(0x0000ff))
            .text_xl()
            .text_color(rgb(0xffffff))
            .child(format!("Hello, {}!", self.text))
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.), px(500.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| HelloWorld {
                    text: "World".into(),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
```

With crates.io **0.2.2**, replace `gpui_platform::application()` with `gpui::Application::new()` and drop the `gpui_platform` import ([hello_world.rs @ gpui-v0.2.2](https://github.com/zed-industries/zed/blob/gpui-v0.2.2/crates/gpui/examples/hello_world.rs)).

To run the official examples: `cargo run -p gpui --example hello_world` from a Zed checkout ([examples/README.md](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/examples/README.md)).

**Local build check (2026-09-25):** I built a scratch crate (outside this repo) with the Option B dependencies and the abridged code above. Results on this CachyOS machine with **rustc 1.97.0**, which is older than Zed's pinned 1.98.1:

- **Build:** it compiled without errors and resolved **698 packages** in `Cargo.lock`.
  - Cold `cargo build` (debug) took 2m33s wall-clock, including the git fetch, using ~500% CPU.
  - Cold `cargo build --release` took 2m14s at ~1340% CPU.
- **Binary size:**
  - Debug: 565 MB (debuginfo).
  - Release: **37 MB**, or **27 MB** after `strip`.
- **Run:** the binary ran for 5 s under Wayland (`WAYLAND_DISPLAY=wayland-0`) with no panics or errors logged. I didn't visually confirm the window.

Timings are specific to this many-core machine, so treat them as indicative only.

### 2.4 Where to look for each topic (official examples)

The examples README ([examples/README.md](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/examples/README.md)) recommends starting with these:

| Topic | Example / API |
|---|---|
| App shape | `hello_world` |
| **Text input** | `input`: "text input, focus, selections, clipboard actions, and keyboard bindings". It's 784 lines and implements `EntityInputHandler` by hand |
| **Virtualized list (fixed height)** | `uniform_list`: `uniform_list("entries", 50, cx.processor(\|this, range, window, cx\| …))` renders only the visible `range` |
| **Virtualized list (variable height)** | `list` / `ListState` ([src/elements/list.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src/elements/list.rs)): "render a large number of differently sized elements efficiently". Also see the `data_table` and `list_example` examples |
| Testing | `testing`: `#[gpui::test]` with `TestAppContext` |
| Focus / tab order | `focus_visible`, `tab_stop` |
| Popovers / overlays | `popover` (`deferred` + `anchored`) |
| Drag & drop, scrolling | `drag_drop`, `scrollable` |
| Menus, windows, notifications | `set_menus`, `window`, `window_positioning`, `system_notifications` |
| Accessibility | `a11y` |
| Linux layer-shell (panels/overlays on wlroots) | `layer_shell` |

### 2.5 State, events, input, theming: patterns

- **State:**
  - Put app/domain state in `Entity<T>` models. Make views separate entities that hold `Entity` handles to models and `observe` or `subscribe` to them (see §1.1).
  - For app-wide singletons, use globals: `impl Global for T {}` plus `cx.set_global(...)` ([src/global.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src/global.rs), [src/app.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src/app.rs)).
- **Events:**
  - Mouse: `.on_click`, `.on_mouse_down`, etc. Elements need an `.id(...)` to be stateful/clickable (see `uniform_list` example).
  - Keyboard: actions plus key contexts (§1.6).
  - Focus: `FocusHandle` / `Focusable` (`input` example).
- **Theming:**
  - GPUI core has **no theme system**. It exposes the OS light/dark appearance (`WindowAppearance`, `App::window_appearance()`, `Window::observe_window_appearance`) ([src/platform.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src/platform.rs), [src/window.rs](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src/window.rs)).
  - Zed's `theme` crate is GPL-3.0 and unpublished ([crates/theme/Cargo.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/theme/Cargo.toml)).
  - Common pattern: a `Theme` struct stored as a `Global`, and components read colors from it. GPUI Kit ships "semantic themes" ([gpui-kit README](https://github.com/longbridge/gpui-kit)).
- **Suggested project structure:** this is my recommendation, not an official one.
  - `main.rs`: app bootstrap, keymap, and window creation.
  - `actions.rs`: `actions!` declarations.
  - `state/`: `Entity` models, with no rendering.
  - `ui/`: views that implement `Render`, plus small `RenderOnce` components.
  - `theme.rs`: a `Global`.
  - Keep blocking or IO work in `cx.background_spawn` / `BackgroundExecutor::spawn` and push results back via `entity.update`.

---

## 3. Ecosystem

### 3.1 Widgets and component libraries

- **GPUI core:** primitives only (§1.2). There's **no built-in button or text input**. The official route to a text field is the ~780-line `input` example.
- **Zed's `ui` crate** has button, context_menu, data_table, list, modal, popover, scrollbar, tab_bar, toggle, tooltip, and more ([crates/ui/src/components](https://github.com/zed-industries/zed/tree/7ea077c78b865d17854cee812bb775dbe64a925a/crates/ui/src/components)). It's licensed **GPL-3.0-or-later** and `publish = false` ([crates/ui/Cargo.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/ui/Cargo.toml)), so it's unsuitable for non-GPL apps and awkward to depend on. Zed's editor (the real text editor) is also GPL.
- **GPUI Kit / gpui-component** (Longbridge). The repo `longbridge/gpui-component` now redirects to `longbridge/gpui-kit`, which has ~14.8k stars and was pushed 2026-09-25 ([GitHub API](https://api.github.com/repos/longbridge/gpui-kit)).
  - Crates `gpui-kit` / `gpui-component` are at 0.6.6 (2026-09-21), Apache-2.0 ([crates.io](https://crates.io/crates/gpui-component)).
  - It claims "75+ documented components and primitives, WebAssembly support, AccessKit accessibility, UI integration testing", virtualized tables and lists, a code editor ("Stable performance at 200K lines with Tree-sitter"), dock layout, Markdown/HTML rendering, charts, and themes. It's used in production for Longbridge Pro ([README](https://github.com/longbridge/gpui-kit)).
  - Docs are at <https://gpui-kit.com>.
  - It's built on `gpui-pre` snapshots (§2.1 Option C).
  - The Zed blog profiled its author ([Community Champion Spotlight: Jason Lee](https://zed.dev/blog/community-champion-jason-lee)).
- **Other libraries** are listed in the official [zed-industries/awesome-gpui](https://github.com/zed-industries/awesome-gpui) list. Examples include adabraka-ui, Base GPUI (headless), Bezel, gpui-router, gpui-nav, gpui-hooks, gpui-tea (Elm-style), gpui-form, gpui-storybook, gpui-video-player, gpui-mobile (iOS/Android, community), and gpui-pdf. Most are small, and several are marked dormant there.

### 3.2 Documentation quality

- The README admits: "Currently, the best way to learn about these APIs is to read the Zed source code or drop a question in the Zed Discord. We're working on improving the documentation" ([README](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/README.md)).
- Official prose docs consist of the README, `docs/contexts.md`, `docs/key_dispatch.md`, the ownership guide, and the accessibility guide.
- About 45 examples are categorized in `examples/README.md`.
- The [gpui.rs](https://www.gpui.rs/) site is a landing page that links to these. It says contributions must go through Zed's repo.
- docs.rs only covers 0.2.2.
- The Zed blog "gpui" tag's newest GPUI-specific post is from January 2024 ([zed.dev/blog/tagged/gpui](https://zed.dev/blog/tagged/gpui)).

### 3.3 Apps built on GPUI (besides Zed)

From [awesome-gpui](https://github.com/zed-industries/awesome-gpui) (stars as listed there, 2026-09-25):

- Longbridge Pro (commercial trading app)
- OpenLogi (22.2k)
- Zeron (2.2k)
- zedis Redis GUI (2.1k)
- Loungy launcher (1.7k, archived)
- Navop (1.6k)
- Waku (1.5k)
- Sonora music client (1.5k)
- tty7 terminal (1.1k)
- GitComet, Arbor, hummingbird music player, omarchist
- Delta, "built by the creators of Zed"

The category is heavily developer tools, database clients, and AI-agent frontends.

---

## 4. Pros and cons

### Pros

- **Performance / GPU rendering.**
  - Zed designed it "like a video game, organizing the entire application around feeding data to shaders running on the GPU" ([Zed is 1.0](https://zed.dev/blog/zed-1-0), 2026-04-29).
  - It targets 120 FPS with custom shaders per primitive ("rectangles, shadows, text, icons, and images") ([videogame post](https://zed.dev/blog/videogame), [120 FPS Metal post](https://zed.dev/blog/120fps)).
  - It has virtualized `uniform_list`/`list` in core.
- **Battle-tested in a large shipping app.** Zed has ~90.9k stars, reached 1.0 in April 2026, and ships on macOS, Linux, and Windows ([Zed is 1.0](https://zed.dev/blog/zed-1-0), [Windows post](https://zed.dev/blog/zed-for-windows-is-here)).
- **Ergonomic model for complex apps.** It has:
  - A single-owner `App` with entity handles, so you don't need `Rc<RefCell>` soup.
  - Typed events and observers.
  - Keyboard-first actions and keymaps.
  - An integrated async executor.
  - First-class tests (`#[gpui::test]`, `TestAppContext`).
- **Native platform backends everywhere:** Metal, DirectX 11, and Vulkan/GL via wgpu. It also supports native Wayland and X11, including **Wayland layer-shell** (`layer_shell` example), which suits a Linux-first dev.
- **Tailwind-style styling and Taffy flex/grid layout** are familiar to web developers.
- **Licensing:** `gpui` and its platform crates are **Apache-2.0** ([crates/gpui/Cargo.toml](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/Cargo.toml), `LICENSE-APACHE` in each crate). Note the Zed repo as a whole is mixed-license, and GitHub reports "NOASSERTION".
- **Momentum:**
  - Multiple upstream commits per week on `crates/gpui`.
  - The Linux renderer moved to the ecosystem-standard wgpu.
  - AccessKit was added.
  - An experimental web target exists.
  - A healthy third-party app and component ecosystem has grown up.

### Cons

- **Publishing and API stability.**
  - It's pre-1.0, and the README warns "There will often be breaking changes between versions."
  - The official crates.io release (0.2.2) is ~11 months stale versus `main`. Since then, the entry point changed (`Application::new()` → `gpui_platform::application()`), the Linux renderer was swapped, and accessibility was added.
  - There's no published `gpui_platform`, so staying current means using a git rev or third-party snapshots.
  - Issues filed against the crates.io version are closed by Zed's stale-bot if they don't reproduce "on the latest stable Zed". One example is [#46486](https://github.com/zed-industries/zed/issues/46486), about a `gpui 0.2.2` macOS crash, closed "not planned".
- **Driven by Zed's priorities.**
  - The README says: "GPUI is still in active development as we work on the Zed code editor".
  - gpui.rs says contributions must go through Zed's repo and be "kept in sync with it".
  - Features that Zed doesn't need (rich widget set, mobile, theming) come from the community, not Zed.
- **Missing widgets.** There's no stock text input, button, checkbox, dropdown, or file tree in core. You either build them or adopt GPUI Kit, which then couples you to its `gpui-pre` pin.
- **Documentation gaps.** The official stance is "read the Zed source code". Only a handful of guides exist, and docs.rs is stale for `main`.
- **Accessibility is new and opt-in.**
  - AccessKit landed in May 2026. Elements need explicit IDs and roles.
  - Zed itself still has an open "screen reader accessibility missing completely" issue on Windows ([#41138](https://github.com/zed-industries/zed/issues/41138)).
  - Treat screen-reader support as early.
- **Platform caveats.**
  - Linux needs a Vulkan or GL driver. The wgpu instance is created with `VULKAN | GL` only.
  - Windows needs DX11.
  - The web target needs nightly Rust.
  - There's no official iOS/Android support; `gpui-mobile` is community-made.
- **Build weight / compile times.** GPUI pulls a large dependency tree: wgpu, cosmic-text, resvg/usvg, image, taffy, lyon, accesskit, x11rb, wayland-*, ashpd, oo7, and more ([Cargo manifests above](https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui_linux/Cargo.toml)). A git dependency also clones the full Zed monorepo. A minimal hello-world resolved 698 crates. On this machine it built in about 2–2.5 minutes cold, and the release binary was 37 MB (27 MB stripped) (§2.3). **I found no official binary-size figures.**
- **Not Tokio-native.** It uses its own executor tied to the platform loop (§1.5).

---

## 5. Brief comparison with alternatives

Each project's own README/crates.io, checked 2026-09-25.

| Framework | Model / renderer | Latest | License | Notes (from its own docs) |
|---|---|---|---|---|
| **GPUI** | Hybrid immediate/retained; Metal / DX11 / wgpu | 0.2.2 (crates.io, Oct 2025); `main` moves daily | Apache-2.0 | Pre-1.0, few built-in widgets, AccessKit since 2026-05 |
| **egui** ([README](https://github.com/emilk/egui)) | Immediate mode; `egui-wgpu` / glow via `eframe` | 0.36.2 (2026-09-08) | MIT/Apache-2.0 | Many built-in widgets, native plus web. "Optional support for AccessKit… enabled by default in eframe" |
| **Iced** ([README](https://github.com/iced-rs/iced)) | Elm architecture; wgpu (Vulkan/Metal/DX12) + tiny-skia fallback | 0.14.0 (2025-12-07) | MIT | "Iced is currently experimental software." Built-in widget set |
| **Slint** ([README](https://github.com/slint-ui/slint)) | Declarative `.slint` DSL; femtovg / Skia / software renderers | 1.18.1 (2026-09-21) | GPLv3 **or** royalty-free **or** commercial | Post-1.0 stability, embedded-to-desktop, bindings for C++/JS/Python |
| **Tauri** ([README](https://github.com/tauri-apps/tauri)) | Web frontend in the system webview (WRY; WebKitGTK on Linux) | 2.11.6 (2026-09-21) | Apache-2.0/MIT | Mature, mobile support; UI is HTML/CSS/JS rather than Rust |
| **Dioxus** ([README](https://github.com/DioxusLabs/dioxus)) | React-like; webview by default, "experimental WGPU-based renderer" (Blitz) | 0.7.10 (2026-07-30) | MIT/Apache-2.0 | Web, desktop, and mobile from one codebase; HTML/CSS/Tailwind styling |
| **Makepad** ([README](https://github.com/makepad/makepad)) | Own GPU runtime plus live-editable DSL | `makepad-widgets` 1.0.0 (2025-05) | MIT | Native and web, built-in widget set |
| **Xilem / Masonry** ([README](https://github.com/linebender/xilem)) | Reactive; Vello + wgpu, AccessKit | 0.4.0 (2025-10-29) | Apache-2.0 | "An experimental Rust architecture for reactive UI" |
| **Floem** ([README](https://github.com/lapce/floem)) | Fine-grained reactivity; wgpu (vger/vello) or Skia, tiny-skia fallback | 0.2.0 (2024-11) | MIT | Used by Lapce; crates.io release is old |

---

## 6. Recommendation

**GPUI is a good fit when:**

- The app is desktop-only, performance-sensitive, and keyboard-heavy: an editor, terminal, database/API client, dev tool, or dashboard with big virtualized lists. That's exactly the niche where most GPUI apps live (§3.3).
- You're happy to **pin a Zed `rev` and bump deliberately**, reading the diff of `crates/gpui` each time. Alternatively, adopt GPUI Kit, which picks a coherent snapshot of GPUI plus 75+ components for you.
- You want native Wayland (including layer-shell) and X11 on Linux, plus macOS and Windows, from one codebase.
- An Apache-2.0 framework is required. Avoid copying from Zed's GPL `ui`/`editor` crates.

**GPUI is a poor fit when:**

- You need a **stable, semver-respecting API** or plan to depend only on crates.io releases. In that case prefer Slint (1.x) or egui/Iced (frequent releases on crates.io).
- You need rich **built-in widgets** without a third-party layer. egui, Iced, Slint, and Makepad ship more out of the box.
- **Accessibility compliance** is a hard requirement today. GPUI's AccessKit support is only months old and opt-in. egui, Xilem, and Slint have more mileage **(relative maturity is my judgement, unverified)**.
- You need **mobile** or a first-class **web** target. Tauri or Dioxus fit better, and GPUI's web target needs nightly.
- The team prefers web technologies for UI. That points to Tauri or Dioxus.

**For `orbit` specifically:** if the goal is a fast native Linux-first desktop app and API churn is acceptable, start with **Option B** (git dependency pinned to `7ea077c…`, features `wayland,x11`). Alternatively, start with **`gpui-kit = "0.6"`** if you want ready-made inputs, buttons, lists, and theming on day one. Decide early, because GPUI Kit pins its own GPUI snapshot, and mixing it with a separate Zed git dependency will give you two incompatible `gpui` crates **(inferred from the renaming scheme in the gpui-kit Cargo.toml)**.

---

## Sources

**Zed repo (commit `7ea077c78b865d17854cee812bb775dbe64a925a`, 2026-09-25, unless noted)**

- crates/gpui/README.md: <https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/README.md>
- crates/gpui/Cargo.toml: <https://github.com/zed-industries/zed/blob/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/Cargo.toml>
- crates/gpui/examples/README.md and hello_world.rs, input.rs, uniform_list.rs, testing.rs: <https://github.com/zed-industries/zed/tree/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/examples>
- crates/gpui/docs/contexts.md, key_dispatch.md: <https://github.com/zed-industries/zed/tree/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/docs>
- crates/gpui/src/_ownership_and_data_flow.rs, _accessibility.rs, executor.rs, elements/: <https://github.com/zed-industries/zed/tree/7ea077c78b865d17854cee812bb775dbe64a925a/crates/gpui/src>
- crates/gpui_platform, gpui_linux, gpui_wgpu, gpui_windows, gpui_macos, gpui_apple, gpui_web: <https://github.com/zed-industries/zed/tree/7ea077c78b865d17854cee812bb775dbe64a925a/crates>
- crates/ui/Cargo.toml, crates/theme/Cargo.toml (GPL-3.0-or-later)
- Workspace Cargo.toml (`publish = false`), rust-toolchain.toml (1.98.1), script/linux, docs/src/development/linux.md
- Tag gpui-v0.2.2 (commit 69e2130295c2649963eb639fc70b4f2ee8ea1624): <https://github.com/zed-industries/zed/tree/gpui-v0.2.2/crates/gpui>
- PR #46758 (wgpu on Linux): <https://github.com/zed-industries/zed/pull/46758>
- PR #49277 (gpui_platform extraction): <https://github.com/zed-industries/zed/pull/49277>
- PR #56065 (AccessKit): <https://github.com/zed-industries/zed/pull/56065>
- Issue #41138 (Windows screen reader): <https://github.com/zed-industries/zed/issues/41138>
- Issue #46486 (crates.io 0.2.2 macOS crash): <https://github.com/zed-industries/zed/issues/46486>
- Commit history for crates/gpui: <https://github.com/zed-industries/zed/commits/main/crates/gpui>

**Registries / sites**

- crates.io gpui: <https://crates.io/crates/gpui> (API: <https://crates.io/api/v1/crates/gpui>)
- crates.io gpui-pre: <https://crates.io/crates/gpui-pre>
- crates.io gpui-component / gpui-kit: <https://crates.io/crates/gpui-component>, <https://crates.io/crates/gpui-kit>
- docs.rs gpui: <https://docs.rs/gpui/latest/gpui/>
- gpui.rs: <https://www.gpui.rs/>

**Zed blog**

- Leveraging Rust and the GPU to render user interfaces at 120 FPS (2023-03-07): <https://zed.dev/blog/videogame>
- Optimizing the Metal pipeline to maintain 120 FPS in GPUI: <https://zed.dev/blog/120fps>
- GPUI 2 is now in production (2024-01-03): <https://zed.dev/blog/gpui-2-on-preview>
- Ownership and data flow in GPUI (2024-01-25): <https://zed.dev/blog/gpui-ownership>
- Windows When? Windows Now (2025-10-15): <https://zed.dev/blog/zed-for-windows-is-here>
- Zed is 1.0 (2026-04-29): <https://zed.dev/blog/zed-1-0>
- Community Champion Spotlight: Jason Lee: <https://zed.dev/blog/community-champion-jason-lee>
- GPUI tag: <https://zed.dev/blog/tagged/gpui>

**Ecosystem**

- GPUI Kit (formerly gpui-component): <https://github.com/longbridge/gpui-kit>, Cargo.toml: <https://github.com/longbridge/gpui-kit/blob/main/Cargo.toml>, docs <https://gpui-kit.com>
- awesome-gpui (official list): <https://github.com/zed-industries/awesome-gpui>

**Alternatives**

- egui <https://github.com/emilk/egui> · Iced <https://github.com/iced-rs/iced> · Slint <https://github.com/slint-ui/slint> · Tauri <https://github.com/tauri-apps/tauri> · Dioxus <https://github.com/DioxusLabs/dioxus> · Makepad <https://github.com/makepad/makepad> · Xilem <https://github.com/linebender/xilem> · Floem <https://github.com/lapce/floem>
