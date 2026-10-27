//! Orbit's desktop shell. All vault and settings logic lives in `orbit-core`;
//! this crate lays out the windows, applies the appearance (fonts, Orbit
//! Paper themes, grain) and forwards clicks to the Vault facade.

mod appearance;
mod commands;
mod file_actions;
#[cfg(unix)]
mod instance;
mod link_completion;
mod note_editor;
mod preview;
mod right_panel;
mod search_panel;
mod settings_view;
mod switcher;
mod vaults;
mod welcome;
mod window;

use std::path::PathBuf;

fn main() {
    // `orbit <folder>` opens that vault; plain `orbit` shows the Welcome window.
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    #[cfg(unix)]
    if instance::hand_off(path.as_deref()) {
        return;
    }
    gpui_kit::application()
        .with_assets(appearance::OrbitAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            appearance::init(cx);
            note_editor::init(cx);
            file_actions::init(cx);
            commands::init(cx);
            switcher::init(cx);
            vaults::init(cx);
            #[cfg(unix)]
            instance::listen(cx);
            vaults::open_or_welcome(path, cx);
        });
}
