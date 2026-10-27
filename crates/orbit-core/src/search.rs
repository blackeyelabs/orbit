//! Vault search: the full-text index (orbit-index) fed the vault's notes,
//! kept current from the paths that change, and cached where the vault's
//! storage allows (`.vault/search` for a plain vault), so reopening only
//! reads the notes that changed since.

use std::{
    collections::HashSet,
    ops::Range,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use orbit_fs::EntryType;
use orbit_index::{NoteDoc, Query, SearchIndex};

use crate::excerpt::{Snippet, excerpt};
use crate::{
    EntryKind, MAX_DEPTH, Vault, VaultPath, is_plain_text_note, is_tool_folder, note_title, tags,
};

/// Snippets shown for each note.
const SNIPPETS: usize = 3;
/// Notes indexed between commits, so results arrive while indexing runs.
const BATCH: usize = 500;

/// What a search found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResults {
    /// The best notes, best first.
    pub results: Vec<SearchResult>,
    /// How many notes match in all.
    pub matching_notes: usize,
    /// How many times the words were found in the notes listed.
    pub matches: usize,
    /// The vault was fully indexed: otherwise results may be missing.
    pub complete: bool,
}

/// A note a search found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub path: VaultPath,
    pub title: String,
    /// How many times the words were found in it.
    pub matches: usize,
    /// The first lines they were found on, marked.
    pub snippets: Vec<Snippet>,
}

/// How far indexing the vault has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexingProgress {
    /// Notes read so far.
    pub done: usize,
    /// Notes to read in all.
    pub total: usize,
}

/// The vault's search index and what it has yet to catch up with.
///
/// Its index is cached on disk, so a protected note must never reach it:
/// once protected folders mount into the tree, their notes need an index
/// of their own, in memory only (#22).
#[derive(Default)]
pub(crate) struct Search {
    /// Opened on first use: from the cache, or new.
    index: OnceLock<SearchIndex>,
    /// Changed since the index last caught up.
    pending: Mutex<Vec<VaultPath>>,
    /// While the vault is being indexed.
    progress: Mutex<Option<IndexingProgress>>,
    complete: AtomicBool,
}

impl Search {
    pub(crate) fn changed(&self, paths: impl IntoIterator<Item = VaultPath>) {
        lock(&self.pending).extend(paths);
    }
}

/// A note on disk, with when it last changed: nanoseconds since 1970, or
/// `None` if that isn't known.
type OnDisk = (VaultPath, Option<u64>);

impl Vault {
    /// Brings the search index up to date with the whole vault: from the
    /// cache, only the notes changed since it was saved are read. It can
    /// take a while; searches meanwhile say they may be incomplete, and
    /// [`Self::search_progress`] says how far it's got.
    pub fn index_search(&self) {
        let index = self.search_index();
        *lock(&self.search.progress) = Some(IndexingProgress { done: 0, total: 0 });
        self.reindex_within(index, &VaultPath::root(), true);
        self.search.complete.store(true, Ordering::Release);
        *lock(&self.search.progress) = None;
    }

    /// While the vault is being indexed: how far it's got.
    pub fn search_progress(&self) -> Option<IndexingProgress> {
        *lock(&self.search.progress)
    }

    /// Indexes the notes changed since the index last caught up (searches
    /// do this first too): to call when notes change, so searches don't
    /// wait for it.
    pub fn update_search(&self) {
        self.catch_up(self.search_index());
    }

    /// Up to `limit` notes for `input`: words to find (the last may be
    /// unfinished), with `tag:` and `in:` filters. Changes not yet indexed
    /// are indexed first.
    pub fn search(&self, input: &str, limit: usize) -> SearchResults {
        let index = self.search_index();
        self.catch_up(index);
        let complete = self.search.complete.load(Ordering::Acquire);
        let Ok(hits) = index.search(&Query::parse(input), limit) else {
            return SearchResults {
                complete,
                ..SearchResults::default()
            };
        };
        let results: Vec<SearchResult> = hits
            .hits
            .into_iter()
            .filter_map(|hit| {
                let path = VaultPath::new(&hit.path).ok()?;
                Some(SearchResult {
                    title: note_title(&path).to_string(),
                    matches: hit.matches.len(),
                    snippets: snippets(&hit.body, &hit.matches),
                    path,
                })
            })
            .collect();
        SearchResults {
            matching_notes: hits.total,
            matches: results.iter().map(|r| r.matches).sum(),
            results,
            complete,
        }
    }

    fn search_index(&self) -> &SearchIndex {
        self.search.index.get_or_init(|| {
            let cached = self.fs.cache_dir("search");
            SearchIndex::open(cached.as_deref())
                .or_else(|_| SearchIndex::open(None))
                .expect("an index in memory opens")
        })
    }

    /// Indexes the paths changed since the index last caught up.
    fn catch_up(&self, index: &SearchIndex) {
        let changed = std::mem::take(&mut *lock(&self.search.pending));
        for path in &changed {
            let entry = if path.is_root() {
                None
            } else {
                self.entry_at(path).ok().flatten()
            };
            match entry {
                Some(e) if e.entry_type == EntryType::File => {
                    if EntryKind::of_file(&e.name) == EntryKind::Note {
                        self.index_note(index, path, e.modified.map(nanos));
                    }
                }
                // A folder (made, moved here, or the whole vault), or one
                // that's gone: brought up to date with what's there now.
                _ => self.reindex_within(index, path, false),
            }
        }
        if !changed.is_empty() {
            commit(index);
        }
    }

    /// Brings what the index has in `folder` up to date with the disk:
    /// notes gone are taken out, and notes new or changed since they were
    /// indexed are read. With `progress`, says how far it's got.
    fn reindex_within(&self, index: &SearchIndex, folder: &VaultPath, progress: bool) {
        let mut on_disk = Vec::new();
        let depth = folder.as_str().split('/').filter(|s| !s.is_empty()).count();
        self.notes_on_disk(folder, depth, &mut on_disk);
        let inside = format!("{folder}/");
        let within =
            |path: &str| folder.is_root() || path == folder.as_str() || path.starts_with(&inside);
        let known = index.modified().unwrap_or_default();
        let present: HashSet<&str> = on_disk.iter().map(|(path, _)| path.as_str()).collect();
        for gone in known
            .keys()
            .filter(|path| within(path) && !present.contains(path.as_str()))
        {
            index.remove(gone);
        }
        // A note whose change time isn't known is read again every time.
        let changed: Vec<&OnDisk> = on_disk
            .iter()
            .filter(|(path, modified)| {
                modified.is_none() || known.get(path.as_str()) != modified.as_ref()
            })
            .collect();
        let total = changed.len();
        for (done, (path, modified)) in changed.into_iter().enumerate() {
            self.index_note(index, path, *modified);
            if progress && (done + 1).is_multiple_of(BATCH) {
                commit(index);
                *lock(&self.search.progress) = Some(IndexingProgress {
                    done: done + 1,
                    total,
                });
            } else if progress && done == 0 {
                *lock(&self.search.progress) = Some(IndexingProgress { done: 0, total });
            }
        }
        commit(index);
    }

    /// Reads a note into the index; one that can't be read is left out.
    fn index_note(&self, index: &SearchIndex, path: &VaultPath, modified: Option<u64>) {
        match self.read_note(path) {
            Ok(text) => {
                index.put(note_doc(path, &text, modified.unwrap_or(0))).ok();
            }
            Err(_) => index.remove(path.as_str()),
        }
    }

    /// Every note in `folder` (`depth` folders down from the vault root),
    /// with when it last changed.
    fn notes_on_disk(&self, folder: &VaultPath, depth: usize, found: &mut Vec<OnDisk>) {
        if (!folder.is_root() && crate::in_tool_folder(folder)) || depth > MAX_DEPTH {
            return;
        }
        let Ok(entries) = self.fs.list_dir(folder) else {
            return;
        };
        for entry in entries {
            let Ok(path) = folder.join(&entry.name) else {
                continue;
            };
            match entry.entry_type {
                EntryType::Dir if !is_tool_folder(&entry.name) => {
                    self.notes_on_disk(&path, depth + 1, found)
                }
                EntryType::File if EntryKind::of_file(&entry.name) == EntryKind::Note => {
                    found.push((path, entry.modified.map(nanos)))
                }
                _ => {}
            }
        }
    }
}

/// A note as the index takes it. A `.txt` note is all text; a markdown
/// note's front matter gives its aliases and tags.
fn note_doc(path: &VaultPath, source: &str, modified: u64) -> NoteDoc {
    let plain = is_plain_text_note(path);
    let mut title = note_title(path).to_string();
    let mut note_tags = Vec::new();
    if !plain {
        for alias in md_engine::aliases(source) {
            title.push(' ');
            title.push_str(&alias);
        }
        note_tags = md_engine::front_matter_tags(source);
        note_tags.extend(md_engine::inline_tags(source));
    }
    let folders = path
        .as_str()
        .match_indices('/')
        .map(|(i, _)| path.as_str()[..i].to_lowercase())
        .collect();
    NoteDoc {
        path: path.to_string(),
        title,
        body: if plain {
            source.to_string()
        } else {
            md_engine::without_front_matter(source).to_string()
        },
        tags: tags::with_all_parents(&note_tags),
        folders,
        modified,
    }
}

/// The first lines with matches, each with its matches marked.
fn snippets(body: &str, matches: &[Range<usize>]) -> Vec<Snippet> {
    let line_of = |at: usize| body[..at].rfind('\n').map_or(0, |n| n + 1);
    let mut lines: Vec<(usize, Vec<Range<usize>>)> = Vec::new();
    for found in matches {
        let line = line_of(found.start);
        if let Some((last, on_line)) = lines.last_mut()
            && *last == line
        {
            on_line.push(found.clone());
        } else if lines.len() == SNIPPETS {
            break;
        } else {
            lines.push((line, vec![found.clone()]));
        }
    }
    lines
        .into_iter()
        .map(|(_, on_line)| excerpt(body, on_line[0].clone(), &on_line))
        .collect()
}

fn commit(index: &SearchIndex) {
    if let Err(e) = index.commit() {
        eprintln!("orbit: couldn't update the search index: {e}");
    }
}

/// A change time for the index: nanoseconds since 1970.
fn nanos(modified: SystemTime) -> u64 {
    modified
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
