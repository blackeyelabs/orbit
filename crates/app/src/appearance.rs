//! Orbit Paper: bundled fonts, the two themes from the design canvas, the
//! paper grain, and applying the person's appearance settings.

use std::{borrow::Cow, path::PathBuf, rc::Rc};

use gpui_kit::assets::AllAssets;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::theme::{Theme, ThemeMode, ThemeSet};
use gpui_kit::*;
use orbit_core::{Settings, ThemeChoice};

/// The interface's base sizes before scaling (Kit's defaults).
const BASE_REM: f32 = 16.;
const BASE_MONO: f32 = 13.;
const GRAIN_TILE: f32 = 300.;
const GRAIN_SVG: &str = "orbit/grain.svg";

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/PlusJakartaSans-Regular.ttf"),
    include_bytes!("../assets/fonts/PlusJakartaSans-Italic.ttf"),
    include_bytes!("../assets/fonts/PlusJakartaSans-Medium.ttf"),
    include_bytes!("../assets/fonts/PlusJakartaSans-SemiBold.ttf"),
    include_bytes!("../assets/fonts/PlusJakartaSans-Bold.ttf"),
    include_bytes!("../assets/fonts/PlusJakartaSans-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
];

/// Kit's assets plus Orbit's own.
pub struct OrbitAssets;

impl AssetSource for OrbitAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == GRAIN_SVG {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/grain.svg"))));
        }
        AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        AllAssets.list(path)
    }
}

/// The settings the whole app shares, and where they are saved.
pub struct AppSettings {
    pub settings: Settings,
    config_dir: Option<PathBuf>,
}

impl Global for AppSettings {}

impl AppSettings {
    pub fn get(cx: &App) -> &Settings {
        &cx.global::<AppSettings>().settings
    }

    pub fn load() -> Self {
        let config_dir = orbit_core::default_config_dir();
        let settings = config_dir
            .as_deref()
            .map(Settings::load)
            .unwrap_or_default();
        AppSettings {
            settings,
            config_dir,
        }
    }

    /// Changes the settings, saves them and re-applies the appearance (and
    /// the log level).
    pub fn update(window: &mut Window, cx: &mut App, change: impl FnOnce(&mut Settings)) {
        let this = cx.global_mut::<AppSettings>();
        let was_detailed = this.settings.detailed_logging;
        change(&mut this.settings);
        if this.settings.detailed_logging != was_detailed {
            crate::logging::set_detailed(this.settings.detailed_logging);
        }
        if let Some(dir) = &this.config_dir
            && let Err(e) = this.settings.save(dir)
        {
            tracing::warn!(error = %e, "couldn't save settings");
        }
        apply(window, cx);
    }
}

/// Registers the bundled fonts and installs Orbit Paper and Orbit Paper Dark.
pub fn init(cx: &mut App) {
    if let Err(e) = cx
        .text_system()
        .add_fonts(FONTS.iter().map(|f| Cow::Borrowed(*f)).collect())
    {
        tracing::error!(error = %e, "couldn't load the bundled fonts");
    }
    match serde_json::from_str::<ThemeSet>(include_str!("../assets/themes/orbit-paper.json")) {
        Ok(set) => Theme::update(cx, |theme| {
            for config in set.themes {
                theme.apply_config(&Rc::new(config));
            }
        }),
        // Only reachable if the bundled JSON is broken; Kit's themes still work.
        Err(e) => tracing::error!(error = %e, "the bundled Orbit Paper theme is invalid"),
    }
    cx.set_global(AppSettings::load());
}

/// Applies the current settings: theme (following the desktop for Match
/// system) and interface size.
pub fn apply(window: &mut Window, cx: &mut App) {
    let settings = AppSettings::get(cx).clone();
    let dark = match settings.theme {
        ThemeChoice::OrbitPaper => false,
        ThemeChoice::OrbitPaperDark => true,
        ThemeChoice::MatchSystem => matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    };
    let mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, Some(window), cx);
    let scale = f32::from(settings.interface_size_percent()) / 100.;
    window.set_rem_size(px(BASE_REM * scale));
    Theme::update(cx, |theme| {
        theme.font_size = px(BASE_REM * scale);
        theme.mono_font_size = px(BASE_MONO * scale);
    });
    window.refresh();
}

/// The halftone paper grain over every surface (design: 3 px pitch, 7.5% ink
/// on Orbit Paper, 5% chalk on Orbit Paper Dark). Drawn as SVG tiles; it has
/// no mouse handlers, so clicks pass through.
pub fn grain(window: &Window, cx: &App) -> AnyElement {
    // The theme's ink (graphite) or chalk, at the canvas's grain strength.
    let theme = cx.theme();
    let colour = theme
        .foreground
        .opacity(if theme.mode.is_dark() { 0.05 } else { 0.075 });
    let size = window.viewport_size();
    let cols = (size.width / px(GRAIN_TILE)).ceil() as usize;
    let rows = (size.height / px(GRAIN_TILE)).ceil() as usize;
    div()
        .absolute()
        .inset_0()
        .overflow_hidden()
        .children((0..rows).flat_map(|r| {
            (0..cols).map(move |c| {
                svg()
                    .path(GRAIN_SVG)
                    .absolute()
                    .left(px(c as f32 * GRAIN_TILE))
                    .top(px(r as f32 * GRAIN_TILE))
                    .size(px(GRAIN_TILE))
                    .text_color(colour)
            })
        }))
        .into_any_element()
}
