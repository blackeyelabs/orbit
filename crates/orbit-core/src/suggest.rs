//! Wikilink autocomplete: notes fuzzy-matched by path, name and aliases.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::index::Files;
use crate::{VaultPath, without_md};

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

/// Up to `limit` notes matching `query`, best first; for an empty query,
/// the first notes in path order.
pub(crate) fn suggest(files: &Files, query: &str, limit: usize) -> Vec<LinkSuggestion> {
    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buf = Vec::new();
    let mut score = |text: &str| pattern.score(Utf32Str::new(text, &mut buf), &mut matcher);
    let mut found: Vec<(u32, &VaultPath, Option<&String>)> = files
        .notes()
        .filter_map(|(path, aliases)| {
            // A markdown note's path without `.md`, as it's written.
            let written = without_md(path.as_str());
            let by_path = score(written).map(|s| (s, None));
            let by_alias = aliases
                .iter()
                .filter_map(|alias| score(alias).map(|s| (s, Some(alias))))
                .max_by_key(|(s, _)| *s);
            // The path wins a tie.
            let (score, alias) = match (by_path, by_alias) {
                (Some(p), Some(a)) if a.0 > p.0 => a,
                (Some(p), _) => p,
                (None, a) => a?,
            };
            Some((score, path, alias))
        })
        .collect();
    // Best first; then shorter paths, then path order.
    found.sort_by_key(|(score, path, _)| (std::cmp::Reverse(*score), path.as_str().len()));
    found
        .into_iter()
        .take(limit)
        .map(|(_, path, alias)| LinkSuggestion {
            link: files.link_text(path),
            path: path.clone(),
            alias: alias.cloned(),
        })
        .collect()
}
