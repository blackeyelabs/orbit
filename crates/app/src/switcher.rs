//! The Switcher, laid out as on the design canvas: one box for the Quick
//! switcher (notes) and, after a `>`, the Command palette. Matching is the
//! core's; this lists the matches and says which was chosen.

use std::{ops::Range, sync::Arc};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::{ActiveTheme as _, IndexPath};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use orbit_core::{NoteMatch, Vault, VaultPath, command_query, match_commands};

use crate::commands::{self, Command};

/// Notes listed at once.
const LIMIT: usize = 50;
// Canvas sizes at 100% (640, 96 and 440 px), in rems.
const WIDTH: f32 = 40.;
const TOP: f32 = 6.;
const LIST_HEIGHT: f32 = 27.5;

const KEY_CONTEXT: &str = "Switcher";

pub fn init(cx: &mut App) {
    // Shift+Enter opens in a new tab, as Ctrl+Enter does.
    cx.bind_keys([KeyBinding::new(
        "shift-enter",
        gpui_kit::base::actions::Confirm { secondary: true },
        Some("Switcher > List"),
    )]);
}

/// What the Switcher lists.
enum Rows {
    Notes(Vec<NoteMatch>),
    /// Each command with the matched parts of its name.
    Commands(Vec<(&'static Command, Vec<Range<usize>>)>),
}

impl Rows {
    fn len(&self) -> usize {
        match self {
            Rows::Notes(notes) => notes.len(),
            Rows::Commands(commands) => commands.len(),
        }
    }

    fn heading(&self) -> &'static str {
        match self {
            Rows::Notes(_) => "NOTES",
            Rows::Commands(_) => "COMMANDS",
        }
    }

    fn none_match(&self) -> &'static str {
        match self {
            Rows::Notes(_) => "No notes match",
            Rows::Commands(_) => "No commands match",
        }
    }
}

/// What was chosen.
pub enum Chosen {
    /// A note to open, in a new tab or not.
    Note {
        path: VaultPath,
        new_tab: bool,
    },
    Command(&'static Command),
}

pub struct Switcher {
    vault: Arc<Vault>,
    /// A note is open, for the commands that need one.
    note_open: bool,
    rows: Rows,
    selected: Option<IndexPath>,
    /// Set by Enter or a click, for the window to act on.
    chosen: Option<Chosen>,
}

impl Switcher {
    /// A Switcher showing what `query` matches; `>` opens on commands.
    pub fn open(
        vault: Arc<Vault>,
        note_open: bool,
        query: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ListState<Switcher>> {
        let switcher = Switcher {
            vault,
            note_open,
            rows: Rows::Notes(Vec::new()),
            selected: None,
            chosen: None,
        };
        let state = cx.new(|cx| ListState::new(switcher, window, cx).searchable(true));
        state.update(cx, |state, cx| {
            state.set_query(query, window, cx);
            state.focus(window, cx);
        });
        state
    }

    pub fn take_chosen(&mut self) -> Option<Chosen> {
        self.chosen.take()
    }
}

impl ListDelegate for Switcher {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.rows = match command_query(query) {
            Some(query) => {
                let available: Vec<&'static Command> = commands::all()
                    .iter()
                    .filter(|command| self.note_open || !command.needs_note)
                    .collect();
                let names: Vec<&str> = available.iter().map(|command| command.name).collect();
                Rows::Commands(
                    match_commands(query, &names)
                        .into_iter()
                        .map(|found| (available[found.index], found.highlight))
                        .collect(),
                )
            }
            None => Rows::Notes(self.vault.find_notes(query, LIMIT)),
        };
        cx.notify();
        Task::ready(())
    }

    fn items_count(&self, _: usize, _: &App) -> usize {
        self.rows.len()
    }

    fn render_section_header(
        &mut self,
        _: usize,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let label = self.rows.heading();
        Some(
            div()
                .px_3()
                .pt_2()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let selected = self.selected == Some(ix);
        let (icon, label, highlight, detail): (
            Option<IconName>,
            String,
            &[Range<usize>],
            AnyElement,
        ) = match &self.rows {
            Rows::Notes(notes) => {
                let note = notes.get(ix.row)?;
                let folder = note.path.parent();
                let detail = div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .when(!folder.is_root(), |this| this.child(folder.to_string()))
                    .into_any_element();
                (
                    Some(IconName::FileText),
                    note.label.clone(),
                    &note.highlight,
                    detail,
                )
            }
            Rows::Commands(found) => {
                let (command, highlight) = found.get(ix.row)?;
                let detail =
                    Kbd::binding_for_action(&*command.action(), Some(command.context), window)
                        .map(IntoElement::into_any_element)
                        .unwrap_or_else(|| div().into_any_element());
                (None, command.name.to_string(), highlight, detail)
            }
        };
        let marked = HighlightStyle {
            color: Some(cx.theme().primary),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let label = StyledText::new(label)
            .with_highlights(highlight.iter().map(|range| (range.clone(), marked)));
        Some(
            ListItem::new(ix).selected(selected).child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .children(icon)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(selected, |this| this.font_weight(FontWeight::SEMIBOLD))
                            .child(label),
                    )
                    .child(detail),
            ),
        )
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let text = self.rows.none_match();
        div()
            .p_4()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(text)
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
        cx.notify();
    }

    fn confirm(&mut self, secondary: bool, _: &mut Window, _: &mut Context<ListState<Self>>) {
        let Some(ix) = self.selected else {
            return;
        };
        self.chosen = match &self.rows {
            Rows::Notes(notes) => notes.get(ix.row).map(|note| Chosen::Note {
                path: note.path.clone(),
                new_tab: secondary,
            }),
            Rows::Commands(found) => found
                .get(ix.row)
                .map(|(command, _)| Chosen::Command(command)),
        };
    }
}

/// The Switcher over the window: a scrim, the box, and the key hints.
pub fn render(
    state: &Entity<ListState<Switcher>>,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let hint = |keys: &'static str, what: &'static str| {
        h_flex()
            .gap_1()
            .child(div().font_family("JetBrains Mono").child(keys))
            .child(what)
    };
    div()
        .absolute()
        .size_full()
        .top_0()
        .left_0()
        .flex()
        .justify_center()
        .child(
            div()
                .id("switcher-scrim")
                .absolute()
                .size_full()
                .bg(cx.theme().overlay)
                .on_click(move |_, window, cx| on_dismiss(window, cx)),
        )
        .child(
            v_flex()
                .key_context(KEY_CONTEXT)
                .mt(rems(TOP))
                .w(rems(WIDTH))
                .max_w_full()
                .h(rems(LIST_HEIGHT))
                .rounded_xl()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .shadow_lg()
                .overflow_hidden()
                .child(
                    List::new(state)
                        .flex_1()
                        .p_2()
                        .search_placeholder("Open a note, or type > for commands"),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_x_5()
                        .gap_y_2()
                        .px_4()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(hint("↑↓", "move"))
                        .child(hint("Enter", "open"))
                        .child(hint("Shift+Enter", "new tab"))
                        .child(hint("Esc", "close"))
                        .child(hint(">", "commands")),
                ),
        )
}
