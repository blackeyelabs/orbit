//! Vaults, notes, link resolution and front matter.
//!
//! [`Vault`] is the headless facade every layer above uses (and, later,
//! `orbit-api`); [`Settings`] holds the app-wide preferences beside it.
//! Neither has a GPUI dependency.

mod settings;

pub use settings::{Settings, ThemeChoice, default_config_dir};

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
    /// The note isn't UTF-8 text, so editing it could lose bytes.
    NotUtf8(VaultPath),
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
            Error::NotUtf8(p) => write!(f, "{p} isn't UTF-8 text, so Orbit won't open it"),
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

    /// A note's text, exactly as on disk.
    pub fn read_note(&self, path: &VaultPath) -> Result<String, Error> {
        check_note(path)?;
        let bytes = self.fs.read(path)?;
        String::from_utf8(bytes).map_err(|_| Error::NotUtf8(path.clone()))
    }

    /// Replaces a note's text atomically. On failure the old note is intact.
    /// Tool folders aren't vault content, so nothing is saved into them.
    pub fn save_note(&self, path: &VaultPath, text: &str) -> Result<(), Error> {
        check_note(path)?;
        if path.as_str().split('/').any(is_tool_folder) {
            return Err(Error::NotANote(path.clone()));
        }
        Ok(self.fs.write(path, text.as_bytes())?)
    }

    /// The contents of the Attachment an image link in `note` points to:
    /// relative to the note's folder, percent-decoded, without any
    /// `#fragment` or `?query`. Remote, absolute and missing links, and links
    /// out of the vault, give `None`.
    pub fn read_linked_attachment(&self, note: &VaultPath, link: &str) -> Option<Vec<u8>> {
        let link = link.split(['#', '?']).next()?;
        let scheme = link.split_once(':').is_some_and(|(s, _)| !s.contains('/'));
        if link.is_empty() || link.starts_with('/') || scheme {
            return None;
        }
        let mut segments: Vec<String> = note
            .parent()
            .as_str()
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        for segment in percent_decode(link)?.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    segments.pop()?;
                }
                name => segments.push(name.to_string()),
            }
        }
        let path = VaultPath::new(&segments.join("/")).ok()?;
        self.fs.read(&path).ok()
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

/// Whether a path is shown as Source mode only: `.txt` notes are plain text.
pub fn is_plain_text_note(path: &VaultPath) -> bool {
    path.name().to_ascii_lowercase().ends_with(".txt")
}

fn check_note(path: &VaultPath) -> Result<(), Error> {
    if EntryKind::of_file(path.name()) == EntryKind::Note {
        Ok(())
    } else {
        Err(Error::NotANote(path.clone()))
    }
}

/// Decodes `%XX` escapes; `None` if they don't decode to UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}
