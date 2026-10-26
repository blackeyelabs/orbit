//! Link resolution as Obsidian does it (`getLinkpathDest`, checked against
//! Obsidian 1.14.4 in tests/links.rs), plus front matter aliases as targets
//! where no file matches.
//!
//! The vault's files are indexed by name in memory. The index is built the
//! first time a link is resolved, then kept current from the paths that
//! change: Orbit's own changes and the watcher's.

use std::collections::{BTreeMap, HashMap};

use crate::{EntryKind, TreeNode, VaultPath};

#[derive(Default)]
pub(crate) struct LinkIndex {
    /// `None` until built, and after a change it can't follow file by file.
    files: Option<Files>,
    /// Changed since the index was last brought up to date.
    pending: Vec<VaultPath>,
}

#[derive(Default)]
pub(crate) struct Files {
    /// Every file (not folder), with its aliases if it's a markdown note.
    aliases: BTreeMap<VaultPath, Vec<String>>,
    /// Lowercased file name → files with that name, in path order.
    by_name: HashMap<String, Vec<VaultPath>>,
    /// Lowercased alias → notes with that alias, in path order.
    by_alias: HashMap<String, Vec<VaultPath>>,
}

/// What a path holds now, for bringing the index up to date.
pub(crate) enum Found {
    /// A file, with its aliases.
    File(Vec<String>),
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
                let aliases = match is_markdown(&node.path).then(|| read(&node.path)) {
                    Some(Found::File(aliases)) => aliases,
                    _ => Vec::new(),
                };
                self.aliases.insert(node.path.clone(), aliases);
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
            Found::File(aliases) => {
                self.aliases.insert(path.clone(), aliases);
            }
            // A folder that's gone takes everything in it; one that's there
            // (new, or moved here) is read afresh.
            Found::Nothing => self.aliases.retain(|file, _| !file.is_within(path)),
            Found::Folder => {
                self.aliases.retain(|file, _| !file.is_within(path));
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

    fn reindex(&mut self) {
        self.by_name.clear();
        self.by_alias.clear();
        for (path, aliases) in &self.aliases {
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

/// Only markdown notes have front matter aliases, as in Obsidian.
pub(crate) fn is_markdown(path: &VaultPath) -> bool {
    path.name().to_ascii_lowercase().ends_with(".md")
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
