//! Vault search: the full-text index (orbit-index) fed the vault's notes,
//! kept current from the paths that change, and cached where the vault's
//! storage allows (`.vault/search` for a plain vault), so reopening only
//! reads the notes that changed since.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use orbit_fs::{EntryType, VaultFs as _};
use orbit_index::{NoteDoc, Query, SearchIndex};

use crate::excerpt::{Snippet, excerpt};
use crate::{
    EntryKind, MAX_DEPTH, Vault, VaultPath, canvas, is_plain_text_note, is_tool_folder, note_title,
    tags,
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

/// The vault's search indexes and what they have yet to catch up with.
///
/// Plain notes are in an index cached on disk; a protected note must never
/// reach it. Each unlocked protected folder's notes are in an index of
/// their own, in memory only, dropped when the folder locks.
#[derive(Default)]
pub(crate) struct Search {
    /// For plain notes: opened on first use, from the cache, or new.
    index: OnceLock<SearchIndex>,
    /// For each unlocked protected folder's notes, by where it is.
    protected: Mutex<HashMap<VaultPath, Arc<SearchIndex>>>,
    /// Changed since the indexes last caught up.
    pending: Mutex<Vec<VaultPath>>,
    /// While the vault is being indexed.
    progress: Mutex<Option<IndexingProgress>>,
    complete: AtomicBool,
    /// Held while a note is read into an index, and while the plain index
    /// is emptied or a folder protected: nothing read before a folder is
    /// protected lands in the cache after it's emptied.
    writing: Mutex<()>,
    /// Bumped each time the plain index is emptied, so a pass of indexing
    /// that ran meanwhile knows it isn't complete.
    emptied: AtomicU64,
}

impl Search {
    pub(crate) fn changed(&self, paths: impl IntoIterator<Item = VaultPath>) {
        lock(&self.pending).extend(paths);
    }
}

/// A note on disk, with when it last changed: nanoseconds since 1970, or
/// `None` if that isn't known.
type OnDisk = (VaultPath, Option<u64>);

/// The index a note goes in.
enum IndexFor<'a> {
    Plain(&'a SearchIndex),
    Protected(Arc<SearchIndex>),
}

impl std::ops::Deref for IndexFor<'_> {
    type Target = SearchIndex;

    fn deref(&self) -> &SearchIndex {
        match self {
            IndexFor::Plain(index) => index,
            IndexFor::Protected(index) => index,
        }
    }
}

impl Vault {
    /// Brings the search indexes up to date with the whole vault: from the
    /// cache, only the plain notes changed since it was saved are read. It
    /// can take a while; searches meanwhile say they may be incomplete,
    /// and [`Self::search_progress`] says how far it's got.
    pub fn index_search(&self) {
        let _span = tracing::debug_span!("search.index").entered();
        let started = std::time::Instant::now();
        tracing::debug!("bringing the search index up to date");
        *lock(&self.search.progress) = Some(IndexingProgress { done: 0, total: 0 });
        loop {
            let emptied = self.search.emptied.load(Ordering::Acquire);
            self.reindex_within(&VaultPath::root(), true);
            // Emptied meanwhile: what was read before is gone; again.
            if self.search.emptied.load(Ordering::Acquire) == emptied {
                break;
            }
        }
        self.search.complete.store(true, Ordering::Release);
        let progress = lock(&self.search.progress).take();
        tracing::info!(
            notes_read = progress.map_or(0, |p| p.total),
            ms = started.elapsed().as_millis() as u64,
            "search index up to date"
        );
    }

    /// While the vault is being indexed: how far it's got.
    pub fn search_progress(&self) -> Option<IndexingProgress> {
        *lock(&self.search.progress)
    }

    /// Indexes the notes changed since the indexes last caught up (searches
    /// do this first too): to call when notes change, so searches don't
    /// wait for it.
    pub fn update_search(&self) {
        self.catch_up();
    }

    /// Up to `limit` notes for `input`: words to find (the last may be
    /// unfinished), with `tag:` and `in:` filters; plain and unlocked
    /// protected notes alike. Changes not yet indexed are indexed first.
    pub fn search(&self, input: &str, limit: usize) -> SearchResults {
        self.catch_up();
        let complete = self.search.complete.load(Ordering::Acquire);
        let query = Query::parse(input);
        let mut hits = Vec::new();
        let mut matching_notes = 0;
        for index in self.all_indexes() {
            if let Ok(found) = index.search(&query, limit) {
                matching_notes += found.total;
                // Scores aren't comparable between indexes (each has its own
                // word statistics): each index's are scaled by its best, so
                // the order across them is rough but none crowds out another.
                let best = found.hits.first().map_or(1., |h| h.score.max(f32::EPSILON));
                hits.extend(found.hits.into_iter().map(|h| (h.score / best, h)));
            }
        }
        // Stable: on a tie, plain notes first.
        hits.sort_by(|(a, _), (b, _)| b.total_cmp(a));
        hits.truncate(limit);
        let results: Vec<SearchResult> = hits
            .into_iter()
            .filter_map(|(_, hit)| {
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
            matching_notes,
            matches: results.iter().map(|r| r.matches).sum(),
            results,
            complete,
        }
    }

    fn search_index(&self) -> &SearchIndex {
        self.search.index.get_or_init(|| {
            let cached = self.fs.cache_dir("search");
            SearchIndex::open(cached.as_deref())
                .or_else(|e| {
                    tracing::warn!(error = %e, "couldn't open the saved search index; indexing in memory");
                    SearchIndex::open(None)
                })
                .expect("an index in memory opens")
        })
    }

    /// The unlocked protected folder `root`'s index, in memory: made when
    /// first needed. `None` if it isn't unlocked (it may have just locked).
    fn protected_index(&self, root: &VaultPath) -> Option<Arc<SearchIndex>> {
        let mut indexes = self.unlocked_indexes();
        if !self.fs.mounted().contains(root) {
            return None;
        }
        let index = indexes.entry(root.clone()).or_insert_with(|| {
            Arc::new(SearchIndex::open(None).expect("an index in memory opens"))
        });
        Some(index.clone())
    }

    /// The protected folders' indexes, each of a folder still unlocked
    /// where it was: those of folders locked, moved or deleted are dropped.
    fn unlocked_indexes(&self) -> std::sync::MutexGuard<'_, HashMap<VaultPath, Arc<SearchIndex>>> {
        let mut indexes = lock(&self.search.protected);
        let mounted = self.fs.mounted();
        indexes.retain(|root, _| mounted.contains(root));
        indexes
    }

    /// The index for `path`: its protected folder's, or the plain one.
    /// `None` for a protected folder that's no longer unlocked.
    fn index_for(&self, path: &VaultPath) -> Option<IndexFor<'_>> {
        match self.fs.protected_folder_of(path) {
            Some(root) => self.protected_index(&root).map(IndexFor::Protected),
            None => Some(IndexFor::Plain(self.search_index())),
        }
    }

    /// The plain index, then each unlocked protected folder's.
    fn all_indexes(&self) -> Vec<IndexFor<'_>> {
        let mut all = vec![IndexFor::Plain(self.search_index())];
        all.extend(
            self.unlocked_indexes()
                .values()
                .cloned()
                .map(IndexFor::Protected),
        );
        all
    }

    /// Indexes the paths changed since the indexes last caught up.
    fn catch_up(&self) {
        let changed = std::mem::take(&mut *lock(&self.search.pending));
        if changed.is_empty() {
            return;
        }
        let _span = tracing::debug_span!("search.update", changed = changed.len()).entered();
        for path in &changed {
            let entry = if path.is_root() {
                None
            } else {
                self.entry_at(path).ok().flatten()
            };
            match entry {
                Some(e) if e.entry_type == EntryType::File => {
                    if EntryKind::of_file(&e.name).is_searchable() {
                        self.index_file(path, e.modified.map(nanos));
                    }
                }
                // A folder (made, moved here, unlocked, or the whole vault),
                // or one that's gone: brought up to date with what's there.
                _ => self.reindex_within(path, false),
            }
        }
        for index in self.all_indexes() {
            commit(&index);
        }
    }

    /// Brings what the indexes have in `folder` up to date with the disk.
    /// With `progress`, says how far the plain index has got.
    fn reindex_within(&self, folder: &VaultPath, progress: bool) {
        if let Some(root) = self.fs.protected_folder_of(folder) {
            if let Some(index) = self.protected_index(&root) {
                self.reindex_into(&index, folder, false);
            }
            return;
        }
        self.reindex_into(self.search_index(), folder, progress);
        for root in self
            .fs
            .mounted()
            .into_iter()
            .filter(|r| r.is_within(folder))
        {
            if let Some(index) = self.protected_index(&root) {
                self.reindex_into(&index, &root, false);
            }
        }
    }

    /// Brings what `index` has in `folder` up to date: notes gone are
    /// taken out, and notes new or changed since they were indexed are
    /// read. A protected folder in a plain one is another index's.
    fn reindex_into(&self, index: &SearchIndex, folder: &VaultPath, progress: bool) {
        let mut on_disk = Vec::new();
        let depth = folder.as_str().split('/').filter(|s| !s.is_empty()).count();
        let into_protected = self.fs.protected_folder_of(folder).is_some();
        self.searchable_on_disk(folder, depth, into_protected, &mut on_disk);
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
            self.index_file(path, *modified);
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

    /// Reads a note or canvas into its index: a protected one's into its
    /// folder's, in memory. One that can't be read is left out.
    fn index_file(&self, path: &VaultPath, modified: Option<u64>) {
        let _writing = lock(&self.search.writing);
        let Some(index) = self.index_for(path) else {
            return;
        };
        let modified = modified.unwrap_or(0);
        let doc = match EntryKind::of_file(path.name()) {
            EntryKind::Canvas => self.fs.read(path).ok().map(|bytes| {
                let cards = canvas::card_text(&String::from_utf8_lossy(&bytes));
                canvas_doc(path, cards, modified)
            }),
            _ => self
                .read_note(path)
                .ok()
                .map(|text| note_doc(path, &text, modified)),
        };
        match doc {
            Some(doc) => {
                index.put(doc).ok();
            }
            None => index.remove(path.as_str()),
        }
    }

    /// Every note and canvas in `folder` (`depth` folders down from the
    /// vault root), with when it last changed. Protected folders in it are
    /// left out unless `protected` (it's one, being indexed).
    fn searchable_on_disk(
        &self,
        folder: &VaultPath,
        depth: usize,
        protected: bool,
        found: &mut Vec<OnDisk>,
    ) {
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
                EntryType::Dir
                    if !is_tool_folder(&entry.name)
                        && (protected || self.fs.protected_folder_of(&path).is_none()) =>
                {
                    self.searchable_on_disk(&path, depth + 1, protected, found)
                }
                EntryType::File if EntryKind::of_file(&entry.name).is_searchable() => {
                    found.push((path, entry.modified.map(nanos)))
                }
                _ => {}
            }
        }
    }
}

/// Drops the protected folder `root`'s index, as it locks.
pub(crate) fn forget_folder(vault: &Vault, root: &VaultPath) {
    let _writing = lock(&vault.search.writing);
    lock(&vault.search.protected).remove(root);
}

/// A canvas as the index takes it: its name, and its cards' text.
fn canvas_doc(path: &VaultPath, cards: String, modified: u64) -> NoteDoc {
    NoteDoc {
        path: path.to_string(),
        title: note_title(path).to_string(),
        body: cards,
        tags: Vec::new(),
        folders: folders_of(path),
        modified,
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
    let folders = folders_of(path);
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

/// Every folder `path` is in, lowercased, from the vault root down.
fn folders_of(path: &VaultPath) -> Vec<String> {
    path.as_str()
        .match_indices('/')
        .map(|(i, _)| path.as_str()[..i].to_lowercase())
        .collect()
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
        tracing::warn!(error = %e, "couldn't update the search index");
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

/// Empties the search index, cache files and all, to be built again: what
/// it held of a folder just protected mustn't stay on disk.
pub(crate) fn forget_everything(vault: &Vault) {
    let _writing = lock(&vault.search.writing);
    let index = vault.search_index();
    if let Err(e) = index.clear() {
        tracing::warn!(error = %e, "couldn't clear the search index");
    }
    vault.search.emptied.fetch_add(1, Ordering::AcqRel);
    vault.search.complete.store(false, Ordering::Release);
}

/// Holds off reading notes into the search index until dropped.
pub(crate) fn hold_writes(vault: &Vault) -> std::sync::MutexGuard<'_, ()> {
    lock(&vault.search.writing)
}
