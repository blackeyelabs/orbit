use gpui_kit::assets::AllAssets;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;
pub mod sidebar;
pub mod titlebar;

use crate::sidebar::Appbar;
use crate::titlebar::OrbitTitleBar;

struct App {
    title_bar: Entity<OrbitTitleBar>,
    sidebar: Entity<Appbar>,
}

impl Render for App {
    fn render(&mut self, _: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.title_bar.clone())
            .child(h_flex().flex_1().child(self.sidebar.clone()))
    }
}

fn main() {
    gpui_kit::application().with_assets(AllAssets).run(|cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(480.), px(320.)),
                cx,
            ))),
            // Client-side decorations so the WM doesn't draw its own title bar on Linux.
            window_decorations: Some(WindowDecorations::Client),
            ..TitleBar::window_options()
        };
        cx.spawn(async move |cx| {
            cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| App {
                    title_bar: cx.new(|_| OrbitTitleBar::default()),
                    sidebar: cx.new(|_| Appbar::default()),
                });
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        })
        .detach();
    });
}
