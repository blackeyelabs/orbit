//! Orbit's windows: one per open vault, and the Welcome window. Opening a
//! vault that's already open focuses its window; every vault opened goes to
//! the top of the recent vaults.

use std::{collections::HashMap, path::Path, path::PathBuf, sync::Arc};

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;
use orbit_core::{RecentVault, RecentVaults, Vault};

use crate::appearance;
use crate::welcome::Welcome;
use crate::window::OrbitWindow;

struct Windows {
    /// Vault root (real path) → its window, and the window's view.
    vaults: HashMap<PathBuf, (AnyWindowHandle, WeakEntity<OrbitWindow>)>,
    welcome: Option<(AnyWindowHandle, Entity<Welcome>)>,
    recent: RecentVaults,
    config_dir: Option<PathBuf>,
}

impl Global for Windows {}

pub fn init(cx: &mut App) {
    let config_dir = orbit_core::default_config_dir();
    let recent = config_dir
        .as_deref()
        .map(RecentVaults::load)
        .unwrap_or_default();
    cx.set_global(Windows {
        vaults: HashMap::new(),
        welcome: None,
        recent,
        config_dir,
    });
    cx.on_window_closed(|cx, id| {
        let windows = cx.global_mut::<Windows>();
        windows.vaults.retain(|_, (w, _)| w.window_id() != id);
        if windows
            .welcome
            .as_ref()
            .is_some_and(|(w, _)| w.window_id() == id)
        {
            windows.welcome = None;
        }
    })
    .detach();
}

/// The recent vaults, newest first.
pub fn recent_vaults(cx: &App) -> Vec<RecentVault> {
    cx.global::<Windows>().recent.vaults()
}

/// Opens the vault at `root` in a window of its own, or focuses the window
/// it's already open in.
pub fn open_vault(root: &Path, cx: &mut App) -> Result<(), orbit_core::Error> {
    let root = root.canonicalize().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => orbit_core::Error::NotFound,
        _ => orbit_core::Error::Io(e),
    })?;
    if let Some(window) = cx.global::<Windows>().vaults.get(&root).map(|(w, _)| *w)
        && window
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    let vault = Arc::new(Vault::open(&root)?);
    let opened_view = std::rc::Rc::new(std::cell::Cell::new(None));
    let set_view = opened_view.clone();
    let window = cx
        .open_window(
            window_options(size(px(1280.), px(800.)), cx),
            |window, cx| {
                appearance::apply(window, cx);
                window.set_window_title(vault.name());
                let view = cx.new(|cx| OrbitWindow::new(vault, window, cx));
                set_view.set(Some(view.downgrade()));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .map_err(|e| {
            orbit_core::Error::Io(std::io::Error::other(format!("no window could open: {e}")))
        })?;
    let windows = cx.global_mut::<Windows>();
    let view = opened_view.take().expect("the window made its view");
    windows.vaults.insert(root.clone(), (window.into(), view));
    windows.recent.add(&root);
    save_recent(windows);
    Ok(())
}

/// What `orbit [folder]` asks for: that vault, or the Welcome window; if
/// the vault won't open, the Welcome window says why.
pub fn open_or_welcome(path: Option<PathBuf>, cx: &mut App) {
    match path {
        Some(path) => {
            if let Err(e) = open_vault(&path, cx) {
                show_welcome(cx, move |welcome| welcome.show_open_error(&path, e))
            }
        }
        None => show_welcome(cx, |_| {}),
    }
}

/// A recent vault found again somewhere else: the list follows it there.
pub fn relocate_recent(old_root: &Path, new_root: &Path, cx: &mut App) {
    let windows = cx.global_mut::<Windows>();
    windows.recent.replace(old_root, new_root);
    save_recent(windows);
}

fn save_recent(windows: &Windows) {
    if let Some(dir) = &windows.config_dir
        && let Err(e) = windows.recent.save(dir)
    {
        tracing::warn!(error = %e, "couldn't save the recent vaults");
    }
}

/// Shows the Welcome window, opening it if it isn't open; `welcome` sets
/// what it shows.
pub fn show_welcome(cx: &mut App, welcome: impl FnOnce(&mut Welcome) + 'static) {
    if let Some((window, view)) = cx.global::<Windows>().welcome.clone()
        && window
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        view.update(cx, |view, cx| {
            welcome(view);
            cx.notify();
        });
        return;
    }
    let mut view = None;
    let opened = cx.open_window(
        window_options(size(px(720.), px(560.)), cx),
        |window, cx| {
            appearance::apply(window, cx);
            window.set_window_title("Welcome to Orbit");
            let welcome = cx.new(|cx| {
                let mut view = Welcome::new(window, cx);
                welcome(&mut view);
                view
            });
            view = Some(welcome.clone());
            cx.new(|cx| Root::new(welcome, window, cx))
        },
    );
    match (opened, view) {
        (Ok(window), Some(view)) => {
            tracing::info!("Welcome window shown");
            cx.global_mut::<Windows>().welcome = Some((window.into(), view));
        }
        (Err(e), _) => tracing::error!(error = %e, "couldn't open the Welcome window"),
        (Ok(_), None) => {}
    }
}

/// Closes the Welcome window, once a vault opened from it.
pub fn close_welcome(cx: &mut App) {
    if let Some((window, _)) = cx.global_mut::<Windows>().welcome.take() {
        window
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
}

fn window_options(window_size: Size<Pixels>, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            window_size,
            cx,
        ))),
        // Client-side decorations so the WM doesn't draw its own title bar on Linux.
        window_decorations: Some(WindowDecorations::Client),
        app_id: Some("orbit".into()),
        ..TitleBar::window_options()
    }
}

/// Locks every protected folder in every vault window, as auto-lock asks.
pub fn lock_everywhere(cx: &mut App) {
    let windows: Vec<(AnyWindowHandle, WeakEntity<OrbitWindow>)> =
        cx.global::<Windows>().vaults.values().cloned().collect();
    for (handle, view) in windows {
        handle
            .update(cx, |_, window, cx| {
                view.update(cx, |this, cx| this.lock_all_folders(window, cx))
                    .ok();
            })
            .ok();
    }
}
