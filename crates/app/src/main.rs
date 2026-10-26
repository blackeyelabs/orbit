//! Orbit's desktop shell. All vault and settings logic lives in `orbit-core`;
//! this crate lays out the window, applies the appearance (fonts, Orbit Paper
//! themes, grain) and forwards clicks to the Vault facade.

mod appearance;
mod settings_view;
mod window;

use std::path::PathBuf;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;
use orbit_core::Vault;

use crate::window::OrbitWindow;

fn main() {
    // Phase 0/1 stopgap: the vault comes from the command line until the
    // Welcome screen (#12) exists.
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: orbit <vault folder>");
        std::process::exit(2);
    };
    let vault = match Vault::open(&path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("orbit: {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    gpui_kit::application()
        .with_assets(appearance::OrbitAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            appearance::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1280.), px(800.)),
                    cx,
                ))),
                // Client-side decorations so the WM doesn't draw its own title bar on Linux.
                window_decorations: Some(WindowDecorations::Client),
                ..TitleBar::window_options()
            };
            cx.spawn(async move |cx| {
                cx.open_window(options, |window, cx| {
                    appearance::apply(window, cx);
                    let view = cx.new(|cx| OrbitWindow::new(vault, window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("failed to open window");
            })
            .detach();
        });
}
