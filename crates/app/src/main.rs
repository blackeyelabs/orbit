//! Orbit's desktop shell. All vault and settings logic lives in `orbit-core`;
//! this crate lays out the windows, applies the appearance (fonts, Orbit
//! Paper themes, grain) and forwards clicks to the Vault facade.

mod appearance;
mod auto_lock;
mod clipboard_guard;
mod commands;
mod diagram_block;
mod file_actions;
#[cfg(unix)]
mod instance;
mod link_completion;
mod note_editor;
mod preview;
mod protection;
mod protection_manage;
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
    harden_process();
    gpui_kit::application()
        .with_assets(appearance::OrbitAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            appearance::init(cx);
            note_editor::init(cx);
            auto_lock::init(cx);
            diagram_block::init(cx);
            file_actions::init(cx);
            commands::init(cx);
            switcher::init(cx);
            vaults::init(cx);
            #[cfg(unix)]
            instance::listen(cx);
            vaults::open_or_welcome(path, cx);
        });
}

/// Keeps protected notes out of crash dumps: none are written for Orbit,
/// and other processes of the user's can't read its memory. (Keys are also
/// kept out of swap where the system allows: orbit-crypto locks them.)
fn harden_process() {
    #[cfg(target_os = "linux")]
    {
        use rustix::process::{
            DumpableBehavior, Resource, Rlimit, set_dumpable_behavior, setrlimit,
        };
        let none = Rlimit {
            current: Some(0),
            maximum: Some(0),
        };
        if let Err(e) = setrlimit(Resource::Core, none) {
            eprintln!("orbit: couldn't turn off core dumps: {e}");
        }
        if let Err(e) = set_dumpable_behavior(DumpableBehavior::NotDumpable) {
            eprintln!("orbit: couldn't turn off core dumps: {e}");
        }
    }
}
