use gpui_kit::assets::IconName;
use gpui_kit::component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_kit::*;

#[derive(Debug)]
pub struct AppBarItem<'a> {
    title: &'a str,
    icon: IconName,
}

#[derive(Debug)]
pub struct Appbar {
    selected_index: usize,
    items: Vec<AppBarItem<'static>>,
}

impl Default for Appbar {
    fn default() -> Self {
        Self {
            selected_index: 0,
            items: vec![
                AppBarItem {
                    title: "Quick Switcher",
                    icon: IconName::Regex,
                },
                AppBarItem {
                    title: "Calendar",
                    icon: IconName::CalendarDays,
                },
            ],
        }
    }
}

impl Render for Appbar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        Sidebar::new("app_bar")
            .collapsible(true)
            .collapsed(true)
            .child(
                SidebarMenu::new().children(self.items.iter().enumerate().map(|(index, item)| {
                    SidebarMenuItem::new(item.title)
                        .icon(item.icon)
                        .active(index == self.selected_index)
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.selected_index = index;
                            cx.notify();
                        }))
                })),
            )
    }
}
