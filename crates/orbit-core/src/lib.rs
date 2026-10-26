//! Vaults, notes, link resolution and front matter.
//!
//! [`Vault`] is the headless facade every layer above uses (and, later,
//! `orbit-api`). It has no GPUI dependency.

use std::{io, path::Path};

pub use orbit_fs::VaultPath;
use orbit_fs::{EntryType, PROTECTED_STORAGE_DIR, PlainFs, VaultFs};

/// Folders that belong to tools, not to the vault's content: Orbit's own
/// cache, Obsidian's settings, git, and protected-folder storage.
const TOOL_FOLDERS: &[&str] = &[".vault", ".obsidian", ".git", PROTECTED_STORAGE_DIR];

/// A backstop against pathological nesting; PlainFs already refuses links
/// back up the tree.
const MAX_DEPTH: usize = 64;

fn is_tool_folder(name: &str) -> bool {
    // Ignoring case: on case-insensitive disks `.Git` is the same folder.
    TOOL_FOLDERS.iter().any(|t| t.eq_ignore_ascii_case(name))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    /// A `.md` or `.txt` file.
    Note,
    /// A `.canvas` file (JSON Canvas).
    Canvas,
    /// Any other file.
    Attachment,
}

impl EntryKind {
    fn of_file(name: &str) -> Self {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("md" | "txt") => EntryKind::Note,
            Some("canvas") => EntryKind::Canvas,
            _ => EntryKind::Attachment,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    pub name: String,
    pub path: VaultPath,
    pub kind: EntryKind,
    /// Empty unless this is a folder.
    pub children: Vec<TreeNode>,
}

#[derive(Debug)]
pub enum Error {
    /// The vault folder doesn't exist or isn't a folder.
    NotFound,
    /// The path isn't a note.
    NotANote(VaultPath),
    Io(io::Error),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound => f.write_str("vault folder not found"),
            Error::NotANote(p) => write!(f, "not a note: {p}"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

/// An open vault: a folder of notes.
pub struct Vault {
    name: String,
    fs: Box<dyn VaultFs>,
}

impl Vault {
    /// Opens the folder at `root` as a vault.
    pub fn open(root: &Path) -> Result<Vault, Error> {
        let plain = PlainFs::new(root).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Error::NotFound,
            _ => Error::Io(e),
        })?;
        let name = plain.root().file_name().map_or_else(
            || plain.root().display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        Ok(Vault {
            name,
            fs: Box::new(plain),
        })
    }

    /// The vault's display name: its folder's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The whole tree: folders first, then entries, each by name ignoring
    /// case. Tool folders are left out.
    pub fn tree(&self) -> Result<Vec<TreeNode>, Error> {
        self.children_of(&VaultPath::root(), 0)
    }

    /// A note's text. Invalid UTF-8 is replaced, not refused (read-only for now).
    pub fn read_note(&self, path: &VaultPath) -> Result<String, Error> {
        if EntryKind::of_file(path.name()) != EntryKind::Note {
            return Err(Error::NotANote(path.clone()));
        }
        let bytes = self.fs.read(path)?;
        Ok(String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
    }

    fn children_of(&self, dir: &VaultPath, depth: usize) -> Result<Vec<TreeNode>, Error> {
        let mut nodes = Vec::new();
        for entry in self.fs.list_dir(dir)? {
            let path = dir.join(&entry.name)?;
            let node = match entry.entry_type {
                // At any depth: a nested repo's .git is tooling too.
                EntryType::Dir if is_tool_folder(&entry.name) => continue,
                EntryType::Dir => TreeNode {
                    // An unreadable or too-deep folder shows as empty rather
                    // than hiding the whole tree.
                    children: if depth < MAX_DEPTH {
                        self.children_of(&path, depth + 1).unwrap_or_default()
                    } else {
                        Vec::new()
                    },
                    kind: EntryKind::Folder,
                    name: entry.name,
                    path,
                },
                EntryType::File => TreeNode {
                    kind: EntryKind::of_file(&entry.name),
                    name: entry.name,
                    path,
                    children: Vec::new(),
                },
            };
            nodes.push(node);
        }
        nodes.sort_by_cached_key(|n| (n.kind != EntryKind::Folder, n.name.to_lowercase()));
        Ok(nodes)
    }
}
