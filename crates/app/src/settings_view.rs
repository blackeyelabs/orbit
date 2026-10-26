//! Settings › Appearance: theme, paper grain and interface size. Changes
//! apply immediately and are saved (orbit-core's Settings).

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::*;
use orbit_core::ThemeChoice;

use crate::appearance::AppSettings;

pub struct AppearancePane;

impl AppearancePane {
    /// Applies a settings change and redraws this pane.
    fn change(
        window: &mut Window,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut orbit_core::Settings),
    ) {
        AppSettings::update(window, cx, edit);
        cx.notify();
    }

    fn row(
        label: &'static str,
        hint: &'static str,
        control: impl IntoElement,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .gap_4()
            .py_3()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(hint),
                    ),
            )
            .child(control)
    }
}

impl Render for AppearancePane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = AppSettings::get(cx).clone();
        let theme_button = |id: &'static str, label: &'static str, choice: ThemeChoice| {
            let selected = settings.theme == choice;
            let button = Button::new(id).label(label).small();
            let button = if selected {
                button.primary()
            } else {
                button.outline()
            };
            button.on_click(cx.listener(move |_, _, window, cx| {
                Self::change(window, cx, |s| s.theme = choice);
            }))
        };
        let size = settings.interface_size_percent();
        let (can_shrink, can_grow) = (
            settings.can_shrink_interface(),
            settings.can_grow_interface(),
        );
        v_flex()
            .child(Self::row(
                "Theme",
                "Orbit Paper, Orbit Paper Dark, or follow the desktop.",
                h_flex()
                    .gap_2()
                    .child(theme_button(
                        "theme-paper",
                        "Orbit Paper",
                        ThemeChoice::OrbitPaper,
                    ))
                    .child(theme_button(
                        "theme-paper-dark",
                        "Orbit Paper Dark",
                        ThemeChoice::OrbitPaperDark,
                    ))
                    .child(theme_button(
                        "theme-system",
                        "Match system",
                        ThemeChoice::MatchSystem,
                    )),
                cx,
            ))
            .child(Self::row(
                "Paper grain",
                "A soft halftone texture on every surface.",
                Switch::new("paper-grain")
                    .checked(settings.paper_grain)
                    .on_click(cx.listener(|_, checked: &bool, window, cx| {
                        let checked = *checked;
                        Self::change(window, cx, |s| s.paper_grain = checked);
                    })),
                cx,
            ))
            .child(Self::row(
                "Interface size",
                "Scales the whole interface.",
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("size-down")
                            .label("−")
                            .small()
                            .outline()
                            .disabled(!can_shrink)
                            .on_click(cx.listener(|_, _, window, cx| {
                                Self::change(window, cx, |s| s.shrink_interface());
                            })),
                    )
                    .child(
                        div()
                            .w(rems(3.))
                            .text_center()
                            .text_sm()
                            .child(format!("{size}%")),
                    )
                    .child(
                        Button::new("size-up")
                            .label("+")
                            .small()
                            .outline()
                            .disabled(!can_grow)
                            .on_click(cx.listener(|_, _, window, cx| {
                                Self::change(window, cx, |s| s.grow_interface());
                            })),
                    ),
                cx,
            ))
    }
}
