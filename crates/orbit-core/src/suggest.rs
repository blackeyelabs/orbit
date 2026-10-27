//! Wikilink autocomplete: notes fuzzy-matched by path and aliases, each
//! with the link Orbit writes for it.

use crate::VaultPath;
use crate::fuzzy;
use crate::index::Files;

/// What a typed `[[query` suggests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkSuggestions {
    /// The notes it matches, best first.
    pub notes: Vec<LinkSuggestion>,
    /// The name to offer as a new note: the query, if no link resolves to
    /// it.
    pub new_note: Option<String>,
}

/// A note a typed `[[query` suggests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSuggestion {
    pub path: VaultPath,
    /// The alias that matched, if the note's path didn't match better.
    pub alias: Option<String>,
    /// The shortest unique link to the note.
    pub link: String,
}

impl LinkSuggestion {
    /// What goes between `[[` and `]]`: the link, showing the alias that
    /// matched (`Plan|Roadmap`).
    pub fn text(&self) -> String {
        match &self.alias {
            Some(alias) => format!("{}|{alias}", self.link),
            None => self.link.clone(),
        }
    }
}

/// Up to `limit` notes for a typed `[[query`, each with its link.
pub(crate) fn suggest(files: &Files, query: &str, limit: usize) -> Vec<LinkSuggestion> {
    fuzzy::find_notes(files, query, limit)
        .into_iter()
        .map(|found| LinkSuggestion {
            link: files.link_text(&found.path),
            path: found.path,
            alias: found.alias,
        })
        .collect()
}
