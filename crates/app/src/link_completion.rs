//! Wikilink autocomplete: typing after `[[` suggests notes, and the last
//! row offers to create one by the typed name. The rows are worked out
//! once, here, for both editors: Kit's source editor (as a completion
//! provider) and Live Preview (as its [`Links`]), which also reads the
//! images it shows through it.

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use editor::{Links, Suggestion, Suggestions};
use gpui_kit::component::input::{
    CompletionHighlighter, CompletionProvider, Rope, RopeExt as _, highlight_characters,
};
use gpui_kit::{App, Image, ImageFormat, Result, Task, Window};
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
    /// Image link → the image it names, decoded, or `None` if the vault
    /// hasn't it, so a frame doesn't read the disk per image. Forgotten
    /// when the vault changes ([`Self::forget_images`]).
    images: RefCell<HashMap<String, Option<Arc<Image>>>>,
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
            images: RefCell::default(),
        }
    }

    /// Forgets the images read: the vault changed, so they may have too.
    pub fn forget_images(&self) {
        self.images.borrow_mut().clear();
    }

    pub fn set_note(&self, note: VaultPath) {
        *self.note.borrow_mut() = note;
        // Links resolve from the note, so its images may be others now.
        self.images.borrow_mut().clear();
    }

    /// The suggestions with the caret at `caret`, the line it's on starting
    /// at `line_start`: `before` is that line up to the caret, `after` the
    /// rest of it (only its first `MAX_LINK` characters are looked at).
    /// Nothing unless the caret's in a wikilink being typed.
    /// Offering the "Create …" row remembers it (see `accepted_create`).
    fn suggest_in_line(
        &self,
        line_start: usize,
        before: &str,
        after: &str,
        caret: usize,
    ) -> Option<Suggestions> {
        *self.offered.borrow_mut() = None;
        let start = line_start + md_engine::typed_link(before)?;
        let name = &before[start - line_start..];
        let row = |label: String, detail: Option<String>, link: &str| Suggestion {
            label,
            detail,
            insert: format!("{link}]]"),
        };
        let found = self
            .vault
            .link_suggestions(&self.note.borrow(), name, LIMIT);
        let mut rows: Vec<Suggestion> = found
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
                row(label, detail, &note.text())
            })
            .collect();
        if let Some(name) = found.new_note {
            rows.push(row(format!("Create “{name}” as a new note"), None, &name));
            *self.offered.borrow_mut() = Some(Offer { start, name, caret });
        }
        Some(Suggestions {
            replace: start..link_end(after, caret),

            rows,
        })
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

impl Links for LinkCompletions {
    /// Whether a link's `target` (`Note#Heading`) resolves from the note:
    /// a link to a heading in the note itself always does.
    fn resolves(&self, target: &str) -> bool {
        let (path, _) = md_engine::split_target(target);
        path.is_empty() || self.vault.resolve(&self.note.borrow(), path).is_some()
    }

    /// The image `target` names, read through the vault once.
    fn image(&self, target: &str) -> Option<Arc<Image>> {
        if let Some(image) = self.images.borrow().get(target) {
            return image.clone();
        }
        let image = image_format(target).and_then(|format| {
            let bytes = self
                .vault
                .read_linked_attachment(&self.note.borrow(), target)?;
            Some(Arc::new(Image::from_bytes(format, bytes)))
        });
        self.images
            .borrow_mut()
            .insert(target.to_string(), image.clone());
        image
    }

    /// What Source mode would suggest with the caret at `caret`.
    fn suggest(&self, text: &str, caret: usize) -> Option<Suggestions> {
        let line_start = text[..caret].rfind('\n').map_or(0, |n| n + 1);
        let line_end = text[caret..].find('\n').map_or(text.len(), |n| caret + n);
        self.suggest_in_line(
            line_start,
            &text[line_start..caret],
            &text[caret..line_end],
            caret,
        )
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
        let line_start = text.line_start_offset(text.offset_to_position(offset).line as usize);
        let before = text.slice(line_start..offset).to_string();
        // The rest of the line, as far as a link is looked for.
        let after: String = text
            .chars_at(offset)
            .take_while(|&c| c != '\n')
            .take(MAX_LINK)
            .collect();
        let Some(suggested) = self.suggest_in_line(line_start, &before, &after, offset) else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        let range = lsp_types::Range::new(
            text.offset_to_position(suggested.replace.start),
            text.offset_to_position(suggested.replace.end),
        );
        let items = suggested
            .rows
            .into_iter()
            .map(|row| CompletionItem {
                label: row.label,
                detail: row.detail,
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: row.insert,
                })),
                ..Default::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        // Whether the caret is in a link is up to `completions`, which sees
        // the text.
        !new_text.is_empty()
    }
}

/// Where the link the caret (at `caret`) is in ends, given `after`, the
/// rest of its line: past its `]]` if it's already closed (the editor may
/// close it), else at the caret. A suggestion replaces the rest of the
/// link.
fn link_end(after: &str, caret: usize) -> usize {
    let cap = after
        .char_indices()
        .nth(MAX_LINK)
        .map_or(after.len(), |(i, _)| i);
    let after = &after[..cap];
    match (after.find("]]"), after.find("[[")) {
        (Some(close), open) if open.is_none_or(|open| close < open) => caret + close + 2,
        _ => caret,
    }
}

/// The image format a link's extension names.
fn image_format(link: &str) -> Option<ImageFormat> {
    Some(
        match live_preview::conceal::image_extension(link)?.as_str() {
            "png" => ImageFormat::Png,
            "jpg" | "jpeg" => ImageFormat::Jpeg,
            "webp" => ImageFormat::Webp,
            "gif" => ImageFormat::Gif,
            "svg" => ImageFormat::Svg,
            "bmp" => ImageFormat::Bmp,
            _ => ImageFormat::Tiff,
        },
    )
}
