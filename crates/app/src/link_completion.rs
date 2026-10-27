//! Wikilink autocomplete in the source editor: typing after `[[` suggests
//! notes, and the last row offers to create one by the typed name.

use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui_kit::component::input::{
    CompletionHighlighter, CompletionProvider, Rope, RopeExt as _, highlight_characters,
};
use gpui_kit::{App, Result, Task, Window};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionResponse, CompletionTextEdit, TextEdit,
};
use orbit_core::{Vault, VaultPath, note_title};

/// Suggestions shown at once.
const LIMIT: usize = 20;
/// How far past the caret a link's closing `]]` is looked for.
const MAX_LINK: usize = 300;

pub struct LinkCompletions {
    vault: Arc<Vault>,
    /// The note being edited, which links resolve from.
    note: RefCell<VaultPath>,
    /// The "Create …" row the menu shows now, if it does.
    offered: RefCell<Option<Offer>>,
}

/// A "Create …" row on offer.
struct Offer {
    /// Where the typed name starts, after `[[`.
    start: usize,
    name: String,
    /// Where the caret was when it was offered.
    caret: usize,
}

impl LinkCompletions {
    pub fn new(vault: Arc<Vault>, note: VaultPath) -> Self {
        LinkCompletions {
            vault,
            note: RefCell::new(note),
            offered: RefCell::new(None),
        }
    }

    pub fn set_note(&self, note: VaultPath) {
        *self.note.borrow_mut() = note;
    }

    /// After an edit: the name whose "Create …" row was just accepted, if it
    /// was. Accepting writes `name]]` in one edit that leaves the caret
    /// after it, with no request for suggestions between (typing `]]`
    /// takes two keystrokes, and each asks again). Any other edit, from
    /// somewhere else than where the row was offered, takes it off offer.
    pub fn accepted_create(&self, text: &str, caret: usize) -> Option<String> {
        let mut offered = self.offered.borrow_mut();
        let offer = offered.as_ref()?;
        let written = format!("{}]]", offer.name);
        let end = offer.start + written.len();
        if caret == end && text.get(offer.start..end) == Some(written.as_str()) {
            return offered.take().map(|offer| offer.name);
        }
        if caret != offer.caret {
            *offered = None;
        }
        None
    }

    /// What the menu marks in each row: the typed name's letters. (Kit's
    /// query runs from where the menu opened, `[[` included.)
    pub fn highlighter() -> Rc<CompletionHighlighter> {
        Rc::new(|query: &str, item: &CompletionItem| {
            highlight_characters(query.rsplit("[[").next().unwrap_or(query), item)
        })
    }
}

impl CompletionProvider for LinkCompletions {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        _: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        *self.offered.borrow_mut() = None;
        let line_start = text.line_start_offset(text.offset_to_position(offset).line as usize);
        let before = text.slice(line_start..offset).to_string();
        let Some(start) = md_engine::typed_link(&before).map(|start| line_start + start) else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        let name = text.slice(start..offset).to_string();
        let range = lsp_types::Range::new(
            text.offset_to_position(start),
            text.offset_to_position(link_end(text, offset)),
        );
        let item = |label: String, detail: Option<String>, link: String| CompletionItem {
            label,
            detail,
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: format!("{link}]]"),
            })),
            ..Default::default()
        };
        let found = self
            .vault
            .link_suggestions(&self.note.borrow(), &name, LIMIT);
        let mut items: Vec<CompletionItem> = found
            .notes
            .into_iter()
            .map(|note| {
                let title = note_title(&note.path).to_string();
                let folder = note.path.parent();
                let (label, detail) = match &note.alias {
                    // An alias, then the note it's for, where it is.
                    Some(alias) if folder.is_root() => (alias.clone(), Some(title)),
                    Some(alias) => (alias.clone(), Some(format!("{folder}/{title}"))),
                    None => (title, (!folder.is_root()).then(|| folder.to_string())),
                };
                item(label, detail, note.text())
            })
            .collect();
        if let Some(name) = found.new_note {
            items.push(item(
                format!("Create “{name}” as a new note"),
                None,
                name.clone(),
            ));
            *self.offered.borrow_mut() = Some(Offer {
                start,
                name,
                caret: offset,
            });
        }
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        // Whether the caret is in a link is up to `completions`, which sees
        // the text.
        !new_text.is_empty()
    }
}

/// Where the link the caret is in ends: past its `]]` if it's already
/// closed on this line (the editor may close it), else at the caret. A
/// suggestion replaces the rest of the link.
fn link_end(text: &Rope, offset: usize) -> usize {
    let after: String = text
        .chars_at(offset)
        .take_while(|&c| c != '\n')
        .take(MAX_LINK)
        .collect();
    match (after.find("]]"), after.find("[[")) {
        (Some(close), open) if open.is_none_or(|open| close < open) => offset + close + 2,
        _ => offset,
    }
}
