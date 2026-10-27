//! Link resolution as Obsidian does it (`getLinkpathDest`, checked against
//! Obsidian 1.14.4 in tests/links.rs), plus front matter aliases as targets
//! where no file matches; and backlinks.
//!
//! The vault's files are indexed in memory: by name, and each note with its
//! aliases and links. The index is built the first time a link is
//! resolved, then kept current from the paths that change: Orbit's own
//! changes and the watcher's.

use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
};

use crate::{EntryKind, TreeNode, VaultPath, is_plain_text_note};

/// Characters of context kept on either side of a link.
const CONTEXT_CHARS: usize = 60;

/// A note that links to another, with each of its links there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlink {
    pub from: VaultPath,
    /// In the order they appear.
    pub mentions: Vec<Mention>,
}

/// One link, in context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    /// The link's line, cut at words to about 60 characters either side of
    /// it, with `…` where cut.
    pub context: String,
    /// Where the link is in `context`, in bytes, as written.
    pub link: Range<usize>,
}

/// What the index keeps of a file: if it's a note, its aliases and links.
#[derive(Clone, Debug, Default)]
pub(crate) struct FileLinks {
    aliases: Vec<String>,
    links: Vec<OutLink>,
}

/// A link in a note.
#[derive(Clone, Debug)]
struct OutLink {
    /// The path or name it points at, without `#heading`.
    target: String,
    mention: Mention,
}

impl FileLinks {
    /// A note's aliases (markdown only, as in Obsidian) and links (only
    /// wikilinks in a `.txt` note, which has no markdown).
    pub(crate) fn of_note(path: &VaultPath, source: &str) -> Self {
        let plain = is_plain_text_note(path);
        FileLinks {
            aliases: if plain {
                Vec::new()
            } else {
                md_engine::aliases(source)
            },
            links: md_engine::links(source)
                .into_iter()
                .filter(|link| !plain || link.style == md_engine::LinkStyle::Wiki)
                .map(|link| OutLink {
                    target: link.path().to_string(),
                    mention: mention(source, link.span),
                })
                .collect(),
        }
    }
}

#[derive(Default)]
pub(crate) struct LinkIndex {
    /// `None` until built, and after a change it can't follow file by file.
    files: Option<Files>,
    /// Changed since the index was last brought up to date.
    pending: Vec<VaultPath>,
}

#[derive(Default)]
pub(crate) struct Files {
    /// Every file (not folder), in path order.
    files: BTreeMap<VaultPath, FileLinks>,
    /// Lowercased file name → files with that name, in path order.
    by_name: HashMap<String, Vec<VaultPath>>,
    /// Lowercased alias → notes with that alias, in path order.
    by_alias: HashMap<String, Vec<VaultPath>>,
}

/// What a path holds now, for bringing the index up to date.
pub(crate) enum Found {
    /// A file, with its links if it's a note.
    File(FileLinks),
    Folder,
    Nothing,
    /// Something that can't be read now: the index is rebuilt.
    Unreadable,
}

impl LinkIndex {
    pub(crate) fn changed(&mut self, paths: impl IntoIterator<Item = VaultPath>) {
        self.pending.extend(paths);
    }

    /// The index, brought up to date: built from `scan` (a folder's
    /// entries, the vault root's for all) if there's none, else following
    /// each changed path with `read`.
    pub(crate) fn current(
        &mut self,
        mut scan: impl FnMut(&VaultPath) -> Vec<TreeNode>,
        mut read: impl FnMut(&VaultPath) -> Found,
    ) -> &Files {
        let pending = std::mem::take(&mut self.pending);
        if let Some(files) = &mut self.files
            && !pending.is_empty()
        {
            let followed = pending
                .iter()
                .all(|path| files.update(path, read(path), &mut scan, &mut read));
            if followed {
                files.reindex();
            } else {
                self.files = None;
            }
        }
        self.files.get_or_insert_with(|| {
            let mut files = Files::default();
            files.add(&scan(&VaultPath::root()), &mut read);
            files.reindex();
            files
        })
    }
}

impl Files {
    fn add(&mut self, nodes: &[TreeNode], read: &mut impl FnMut(&VaultPath) -> Found) {
        for node in nodes {
            if node.kind == EntryKind::Folder {
                self.add(&node.children, read);
            } else {
                let links = match (node.kind == EntryKind::Note).then(|| read(&node.path)) {
                    Some(Found::File(links)) => links,
                    _ => FileLinks::default(),
                };
                self.files.insert(node.path.clone(), links);
            }
        }
    }

    /// Follows one changed path; false if only a rebuild can.
    fn update(
        &mut self,
        path: &VaultPath,
        found: Found,
        scan: &mut impl FnMut(&VaultPath) -> Vec<TreeNode>,
        read: &mut impl FnMut(&VaultPath) -> Found,
    ) -> bool {
        match found {
            Found::File(links) => {
                self.files.insert(path.clone(), links);
            }
            // A folder that's gone takes everything in it; one that's there
            // (new, or moved here) is read afresh.
            Found::Nothing => self.files.retain(|file, _| !file.is_within(path)),
            Found::Folder => {
                self.files.retain(|file, _| !file.is_within(path));
                self.add(&scan(path), read);
            }
            Found::Unreadable => return false,
        }
        true
    }

    /// The file `link` (a path or name, without `#heading`) points at from
    /// the note `from`: Obsidian's `getLinkpathDest`, step for step, then
    /// a note with that alias.
    pub(crate) fn resolve(&self, from: &VaultPath, link: &str) -> Option<VaultPath> {
        if link.is_empty() {
            return Some(from.clone());
        }
        self.resolve_file(from, link)
            .or_else(|| self.resolve_alias(from, link))
    }

    fn resolve_file(&self, from: &VaultPath, link: &str) -> Option<VaultPath> {
        // The link's lowercased path, and the files with its name; a name
        // with no such file is tried as a markdown note's.
        let mut wanted = link.to_lowercase();
        let mut name = base_name(&wanted).to_string();
        let mut candidates = name
            .contains('.')
            .then(|| self.by_name.get(&name))
            .flatten();
        if candidates.is_none() {
            wanted = format!("{link}.md").to_lowercase();
            name = base_name(&wanted).to_string();
            candidates = self.by_name.get(&name);
        }
        let candidates = candidates?;
        if name == wanted && candidates.len() == 1 {
            return Some(candidates[0].clone());
        }
        let lower = |path: &VaultPath| path.as_str().to_lowercase();
        let mut folder = folder_of(&from.as_str().to_lowercase()).to_string();
        // Relative to the linking note's folder.
        if wanted.starts_with("./") || wanted.starts_with("../") {
            if wanted.starts_with("./../") {
                wanted = wanted[2..].to_string();
            }
            if let Some(rest) = wanted.strip_prefix("./") {
                if !folder.is_empty() {
                    folder.push('/');
                }
                wanted = format!("{folder}{rest}");
            } else {
                while let Some(rest) = wanted.strip_prefix("../") {
                    wanted = rest.to_string();
                    folder = folder_of(&folder).to_string();
                }
                if !folder.is_empty() {
                    folder.push('/');
                }
                wanted = format!("{folder}{wanted}");
            }
            if let Some(found) = candidates.iter().find(|c| lower(c) == wanted) {
                return Some(found.clone());
            }
        }
        // The exact path from the vault root.
        let wanted = wanted.strip_prefix('/').unwrap_or(&wanted);
        if let Some(found) = candidates.iter().find(|c| lower(c) == wanted) {
            return Some(found.clone());
        }
        if link.starts_with('/') {
            return None;
        }
        // Any path ending so: under the linking note's folder first, then
        // the rest, each shortest path first. (Obsidian compares the folder
        // as a plain prefix, so `Inbox` also takes in `Inboxes`.)
        let ending: Vec<&VaultPath> = candidates
            .iter()
            .filter(|c| lower(c).ends_with(wanted))
            .collect();
        nearest(ending, &folder)
    }

    fn resolve_alias(&self, from: &VaultPath, link: &str) -> Option<VaultPath> {
        let notes = self.by_alias.get(&link.to_lowercase())?;
        let folder = folder_of(&from.as_str().to_lowercase()).to_string();
        nearest(notes.iter().collect(), &folder)
    }

    /// The notes with links that resolve to `to`, in path order. A note's
    /// links to itself aren't backlinks.
    pub(crate) fn backlinks(&self, to: &VaultPath) -> Vec<Backlink> {
        // Only a link naming `to`'s file or one of its aliases can resolve
        // to it; only those are resolved.
        let name = to.name().to_lowercase();
        let aliases: Vec<String> = self.files.get(to).map_or_else(Vec::new, |file| {
            file.aliases.iter().map(|a| a.to_lowercase()).collect()
        });
        let may_name = |target: &str| {
            let target = target.to_lowercase();
            let base = base_name(&target);
            base == name || format!("{base}.md") == name || aliases.contains(&target)
        };
        self.files
            .iter()
            .filter(|(from, _)| *from != to)
            .filter_map(|(from, file)| {
                let mentions: Vec<Mention> = file
                    .links
                    .iter()
                    .filter(|link| may_name(&link.target))
                    .filter(|link| self.resolve(from, &link.target).as_ref() == Some(to))
                    .map(|link| link.mention.clone())
                    .collect();
                (!mentions.is_empty()).then(|| Backlink {
                    from: from.clone(),
                    mentions,
                })
            })
            .collect()
    }

    fn reindex(&mut self) {
        self.by_name.clear();
        self.by_alias.clear();
        for (path, FileLinks { aliases, .. }) in &self.files {
            self.by_name
                .entry(path.name().to_lowercase())
                .or_default()
                .push(path.clone());
            for alias in aliases {
                self.by_alias
                    .entry(alias.to_lowercase())
                    .or_default()
                    .push(path.clone());
            }
        }
    }
}

/// Obsidian's tie-break: paths under `folder` (a lowercased prefix) first,
/// then the others, each shortest first; equal lengths in path order.
fn nearest(mut candidates: Vec<&VaultPath>, folder: &str) -> Option<VaultPath> {
    candidates.sort_by_key(|c| {
        let under = c.as_str().to_lowercase().starts_with(folder);
        // Obsidian's lengths are in UTF-16 units.
        (!under, c.as_str().encode_utf16().count())
    });
    candidates.first().map(|c| (*c).clone())
}

fn base_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// The link at `span` in its line of `source`, cut to [`CONTEXT_CHARS`]
/// either side at word boundaries.
fn mention(source: &str, span: Range<usize>) -> Mention {
    let line_start = source[..span.start].rfind('\n').map_or(0, |n| n + 1);
    let line_end = source[span.end..]
        .find(['\n', '\r'])
        .map_or(source.len(), |n| span.end + n);
    let before = source[line_start..span.start].trim_start();
    let after = source[span.end..line_end].trim_end();
    let (before, cut_before) = match before.char_indices().rev().nth(CONTEXT_CHARS - 1) {
        // Start at the next word, if there's one before the link.
        Some((cut, _)) => {
            let rest = &before[cut..];
            let word = rest.find(char::is_whitespace).map_or(0, |n| n + 1);
            (rest[word..].trim_start(), true)
        }
        None => (before, false),
    };
    let (after, cut_after) = match after.char_indices().nth(CONTEXT_CHARS) {
        Some((cut, _)) => {
            let kept = &after[..cut];
            let word = kept.rfind(char::is_whitespace).unwrap_or(cut);
            (kept[..word].trim_end(), true)
        }
        None => (after, false),
    };
    let mut context = String::new();
    if cut_before {
        context.push('…');
    }
    context.push_str(before);
    let link = context.len()..context.len() + span.len();
    // A markdown link's text can run over a line end.
    context.push_str(&source[span].replace(['\n', '\r'], " "));
    context.push_str(after);
    if cut_after {
        context.push('…');
    }
    Mention { context, link }
}
