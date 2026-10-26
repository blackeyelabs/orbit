use gpui_kit::assets::IconName;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::Button;
use gpui_kit::*;

#[derive(Debug, Default)]
enum PanelState {
    #[default]
    Folder,
    // Not selectable until the panels are wired up (#7).
    #[expect(dead_code)]
    Search,
    #[expect(dead_code)]
    Bookmark,
}

#[derive(Debug, Default)]
pub struct OrbitTitleBar {
    side_panel: bool,
    panel_state: PanelState,
}

impl Render for OrbitTitleBar {
    fn render(&mut self, _: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(Button::new("Left panel").icon(if self.side_panel {
                        IconName::PanelLeftClose
                    } else {
                        IconName::PanelLeft
                    }))
                    .child(div().w_2())
                    .child(Button::new("Files").icon(match self.panel_state {
                        PanelState::Folder => IconName::FolderOpen,
                        _ => IconName::FolderClosed,
                    }))
                    .child(Button::new("Search").icon(match self.panel_state {
                        PanelState::Search => IconName::Search,
                        _ => IconName::Search,
                    }))
                    .child(Button::new("Bookmark").icon(match self.panel_state {
                        PanelState::Bookmark => IconName::Bookmark,
                        _ => IconName::Bookmark,
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(Button::new("settings").icon(IconName::ChevronDown))
                    .child(Button::new("right panel").icon(IconName::PanelRight)),
            )
    }
}
