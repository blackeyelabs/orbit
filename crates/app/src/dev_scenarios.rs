//! For measuring (built with the `dev-profiler` feature only): scripted
//! scenarios a vault window runs by itself, so CPU and memory can be
//! compared run for run (docs/research/performance.md). `ORBIT_SCENARIO`
//! picks one:
//! - `idle`: nothing, with no note open;
//! - `open:<note>`: opens the note, then nothing;
//! - `type:<note>`: opens the note and types into it, ten keys a second
//!   for 3 s, then pauses 2 s (past the autosave delay), over and over:
//!   saves happen as they would.
//!
//! Each logs "scenario started" at info. Ctrl+Alt+Shift+F cycles GPUI's
//! frame-time overlay.

use std::time::Duration;

use gpui_kit::*;
use orbit_core::VaultPath;

use crate::window::OrbitWindow;

/// How long it waits for the window to settle before starting.
const SETTLE: Duration = Duration::from_millis(1500);
/// The pause between typed keys.
const KEY_EVERY: Duration = Duration::from_millis(100);
/// What's typed, over and over.
const TYPED: &str = "the quick brown fox jumps over the lazy dog ";
/// Keys typed before each pause.
const BURST: usize = 30;
/// The pause after each burst: longer than the autosave delay.
const PAUSE: Duration = Duration::from_secs(2);

actions!(
    orbit_dev,
    [
        /// Cycles GPUI's frame-time overlay: hidden, frame time, detailed.
        CycleFrameOverlay
    ]
);

/// Binds the overlay key.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "ctrl-alt-shift-f",
        CycleFrameOverlay,
        Some(crate::window::KEY_CONTEXT),
    )]);
}

/// The overlay's action, for the window's root.
pub fn cycle_overlay(_: &CycleFrameOverlay, window: &mut Window, _: &mut App) {
    window.cycle_debug_frame_overlay_mode();
}

/// Starts the scenario `ORBIT_SCENARIO` names, if any.
pub fn start(window: &mut Window, cx: &mut Context<OrbitWindow>) {
    let Some(scenario) = std::env::var("ORBIT_SCENARIO")
        .ok()
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let (kind, note) = match scenario.split_once(':') {
        Some((kind, note)) => (kind.to_string(), VaultPath::new(note).ok()),
        None => (scenario.clone(), None),
    };
    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor().timer(SETTLE).await;
        tracing::info!(scenario = %kind, "scenario started");
        if let Some(note) = note {
            this.update_in(cx, |this, window, cx| this.open_note(note, window, cx))
                .ok();
        }
        match kind.as_str() {
            "type" => {}
            "idle" | "open" => return,
            other => return tracing::warn!(scenario = other, "no such scenario"),
        }
        for (typed, key) in TYPED.chars().cycle().enumerate() {
            let pause = if typed > 0 && typed % BURST == 0 {
                PAUSE
            } else {
                KEY_EVERY
            };
            cx.background_executor().timer(pause).await;
            let key = if key == ' ' {
                "space".to_string()
            } else {
                key.to_string()
            };
            let typed = cx.update(|window, cx| {
                window.dispatch_keystroke(Keystroke::parse(&key).expect("a key"), cx);
            });
            if typed.is_err() {
                break;
            }
        }
    })
    .detach();
}
