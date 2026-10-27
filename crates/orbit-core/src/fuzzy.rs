//! Fuzzy matching (nucleo) for the Switcher and wikilink autocomplete:
//! notes by path or alias, commands by name, ignoring case. Each match
//! comes with the parts of its label to mark.

use std::{cmp::Reverse, ops::Range};

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::index::Files;
use crate::{VaultPath, note_title, without_md};

/// A note a query matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteMatch {
    pub path: VaultPath,
    /// The alias that matched, if the note's path didn't match better.
    pub alias: Option<String>,
    /// What to show: the alias, else the note's title.
    pub label: String,
    /// The matched parts of `label`, in bytes.
    pub highlight: Vec<Range<usize>>,
}

/// A command a query matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandMatch {
    /// Which of the commands it is.
    pub index: usize,
    /// The matched parts of its name, in bytes.
    pub highlight: Vec<Range<usize>>,
}

/// What the Switcher's input asks for the Command palette: the query after
/// a leading `>`. Without one, it's a note's name.
pub fn command_query(input: &str) -> Option<&str> {
    input.strip_prefix('>').map(str::trim)
}

/// The commands `query` matches, by name, best first; the rest of the
/// order is theirs. An empty query matches them all.
pub fn match_commands(query: &str, names: &[&str]) -> Vec<CommandMatch> {
    let mut fuzzy = Fuzzy::new(query, Config::DEFAULT);
    let mut found: Vec<(u32, CommandMatch)> = names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            let (score, indices) = fuzzy.find(name)?;
            let highlight = char_ranges(name, &indices, 0);
            Some((score, CommandMatch { index, highlight }))
        })
        .collect();
    // A stable sort keeps their order among equals.
    found.sort_by_key(|(score, _)| Reverse(*score));
    found.into_iter().map(|(_, found)| found).collect()
}

/// Up to `limit` notes matching `query`, by path (as written in a link,
/// without `.md`) or alias, best first; then shorter paths, then path
/// order. An empty query lists the first notes in path order.
pub(crate) fn find_notes(files: &Files, query: &str, limit: usize) -> Vec<NoteMatch> {
    let mut fuzzy = Fuzzy::new(query, Config::DEFAULT.match_paths());
    let mut found: Vec<(u32, NoteMatch)> = files
        .notes()
        .filter_map(|(path, aliases)| {
            let title = note_title(path);
            let written = without_md(path.as_str());
            let by_path = fuzzy.find(written).map(|(score, indices)| {
                // Only the title's letters are marked; it ends the path.
                let skip = written.chars().count() - title.chars().count();
                (score, None, title, char_ranges(title, &indices, skip))
            });
            let by_alias = aliases
                .iter()
                .filter_map(|alias| {
                    let (score, indices) = fuzzy.find(alias)?;
                    Some((
                        score,
                        Some(alias),
                        alias.as_str(),
                        char_ranges(alias, &indices, 0),
                    ))
                })
                .max_by_key(|(score, ..)| *score);
            // The path wins a tie.
            let (score, alias, label, highlight) = match (by_path, by_alias) {
                (Some(p), Some(a)) if a.0 > p.0 => a,
                (Some(p), _) => p,
                (None, a) => a?,
            };
            Some((
                score,
                NoteMatch {
                    path: path.clone(),
                    alias: alias.cloned(),
                    label: label.to_string(),
                    highlight,
                },
            ))
        })
        .collect();
    found.sort_by_key(|(score, found)| (Reverse(*score), found.path.as_str().len()));
    found
        .into_iter()
        .take(limit)
        .map(|(_, found)| found)
        .collect()
}

/// A query, ready to match text against.
struct Fuzzy {
    pattern: Pattern,
    matcher: Matcher,
    chars: Vec<char>,
}

impl Fuzzy {
    fn new(query: &str, config: Config) -> Self {
        Fuzzy {
            pattern: Pattern::new(
                query,
                CaseMatching::Ignore,
                Normalization::Smart,
                AtomKind::Fuzzy,
            ),
            matcher: Matcher::new(config),
            chars: Vec::new(),
        }
    }

    /// `text`'s score and the indices of the characters that matched, if
    /// it matches.
    fn find(&mut self, text: &str) -> Option<(u32, Vec<u32>)> {
        let mut indices = Vec::new();
        let haystack = Utf32Str::new(text, &mut self.chars);
        let score = self
            .pattern
            .indices(haystack, &mut self.matcher, &mut indices)?;
        indices.sort_unstable();
        indices.dedup();
        Some((score, indices))
    }
}

/// The characters at `indices` (counted `skip` characters before `text`
/// starts) as byte ranges of `text`, neighbours joined.
fn char_ranges(text: &str, indices: &[u32], skip: usize) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (i, (at, c)) in text.char_indices().enumerate() {
        if indices.binary_search(&((i + skip) as u32)).is_err() {
            continue;
        }
        let end = at + c.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == at => last.end = end,
            _ => ranges.push(at..end),
        }
    }
    ranges
}
