//! Vaults, notes, link resolution and front matter.
//!
//! [`Vault`] is the headless facade every layer above uses (and, later,
//! `orbit-api`); [`Settings`] holds the app-wide preferences beside it.
//! Neither has a GPUI dependency.

mod open_note;
mod settings;

use open_note::DiskVersion;
pub use open_note::{DiskCheck, OpenNote, Trouble};

pub use settings::{Settings, ThemeChoice, default_config_dir};

use std::{io, path::Path};

pub use orbit_fs::VaultPath;
pub use orbit_fs::Watch;
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
    /// The kind of a file, from its name's extension.
    pub fn of_file(name: &str) -> Self {
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
    /// Something already has this path.
    AlreadyExists(VaultPath),
    /// Another app changed the note since Orbit last read or wrote it.
    ChangedOnDisk(VaultPath),
    /// Another app deleted the note since Orbit last read or wrote it.
    DeletedOnDisk(VaultPath),
    /// A folder can't move into itself or a folder inside it.
    IntoItself(VaultPath),
    /// Not usable as a file or folder name.
    InvalidName(String),
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
            Error::AlreadyExists(p) => write!(f, "{p} already exists"),
            Error::ChangedOnDisk(p) => write!(f, "another app changed {p}"),
            Error::DeletedOnDisk(p) => write!(f, "another app deleted {p}"),
            Error::IntoItself(p) => write!(f, "{p} can't move into itself"),
            Error::InvalidName(n) => write!(f, "{n:?} can't be used as a name"),
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

    /// Opens a note for editing: its text, and an [`OpenNote`] that keeps
    /// track of other apps changing it.
    pub fn open_note(&self, path: &VaultPath) -> Result<(OpenNote, String), Error> {
        check_note(path)?;
        let bytes = self.fs.read(path)?;
        let version = DiskVersion::of(&bytes);
        let text = String::from_utf8(bytes).map_err(|_| Error::NotUtf8(path.clone()))?;
        Ok((OpenNote::new(path.clone(), version), text))
    }

    /// A note's bytes, or `None` if nothing is at its path.
    fn read_disk(&self, path: &VaultPath) -> Result<Option<Vec<u8>>, Error> {
        match self.fs.read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Reports paths changed by any app (Orbit included), in debounced
    /// batches, until the returned [`Watch`] is dropped. Tool folders are
    /// left out. `on_change` runs on a background thread.
    pub fn watch(
        &self,
        mut on_change: impl FnMut(Vec<VaultPath>) + Send + 'static,
    ) -> Result<Watch, Error> {
        Ok(self.fs.watch(Box::new(move |paths: Vec<VaultPath>| {
            let paths: Vec<VaultPath> = paths.into_iter().filter(|p| !in_tool_folder(p)).collect();
            if !paths.is_empty() {
                on_change(paths);
            }
        }))?)
    }

    /// Replaces (or creates) a note's text atomically. On failure the old
    /// note is intact. Tool folders aren't vault content, so nothing is saved
    /// into them. For an open note, [`OpenNote::save`] also never replaces
    /// another app's change.
    pub fn save_note(&self, path: &VaultPath, text: &str) -> Result<(), Error> {
        self.save_note_if(path, text, &mut |_| true).map(|_| ())
    }

    /// [`Self::save_note`] if `may_replace` accepts what's on disk just
    /// before the save lands; whether it saved.
    fn save_note_if(
        &self,
        path: &VaultPath,
        text: &str,
        may_replace: &mut dyn FnMut(Option<&[u8]>) -> bool,
    ) -> Result<bool, Error> {
        check_note(path)?;
        if in_tool_folder(path) {
            return Err(Error::NotANote(path.clone()));
        }
        Ok(self.fs.write_if(path, text.as_bytes(), may_replace)?)
    }

    /// Creates an empty note named `Untitled.md` (or `Untitled 1.md`, …) in
    /// `folder`.
    pub fn create_note(&self, folder: &VaultPath) -> Result<VaultPath, Error> {
        let path = self.untitled(folder, ".md")?;
        self.fs.create_file(&path)?;
        Ok(path)
    }

    /// Creates a folder named `Untitled` (or `Untitled 1`, …) in `parent`.
    pub fn create_folder(&self, parent: &VaultPath) -> Result<VaultPath, Error> {
        let path = self.untitled(parent, "")?;
        self.fs.create_dir(&path)?;
        Ok(path)
    }

    /// Renames a note or folder within its folder. A note or canvas keeps
    /// its extension unless the new name has a recognised one (`.md` to
    /// `.txt`, say). Links to it aren't rewritten yet.
    pub fn rename(&self, path: &VaultPath, new_name: &str) -> Result<VaultPath, Error> {
        if path.is_root() || in_tool_folder(path) {
            return Err(Error::InvalidName(path.to_string()));
        }
        check_name(new_name)?;
        let is_folder = self
            .fs
            .list_dir(&path.parent())?
            .iter()
            .any(|e| e.name == path.name() && e.entry_type == EntryType::Dir);
        let old_kind = EntryKind::of_file(path.name());
        let new_name = match path.name().rsplit_once('.') {
            Some((_, ext))
                if !is_folder
                    && old_kind != EntryKind::Attachment
                    && EntryKind::of_file(new_name) == EntryKind::Attachment =>
            {
                format!("{new_name}.{ext}")
            }
            _ => new_name.to_string(),
        };
        check_name(&new_name)?;
        let to = path.parent().join(&new_name)?;
        if &to == path {
            return Ok(to);
        }
        self.move_entry(path, to)
    }

    /// Moves a note or folder into `folder`, keeping its name. Links to it
    /// aren't rewritten yet.
    pub fn move_to(&self, path: &VaultPath, folder: &VaultPath) -> Result<VaultPath, Error> {
        if folder.is_within(path) {
            return Err(Error::IntoItself(path.clone()));
        }
        if in_tool_folder(folder) || in_tool_folder(path) {
            return Err(Error::InvalidName(folder.to_string()));
        }
        self.move_entry(path, folder.join(path.name())?)
    }

    /// Sends a note or folder to the system trash, where it can be restored.
    /// Orbit keeps no trash folder of its own.
    pub fn delete(&self, path: &VaultPath) -> Result<(), Error> {
        if path.is_root() || in_tool_folder(path) {
            return Err(Error::InvalidName(path.to_string()));
        }
        Ok(self.fs.trash(path)?)
    }

    fn move_entry(&self, from: &VaultPath, to: VaultPath) -> Result<VaultPath, Error> {
        match self.fs.rename(from, &to) {
            Ok(()) => Ok(to),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(Error::AlreadyExists(to)),
            Err(e) => Err(e.into()),
        }
    }

    /// The first `Untitled{n}{extension}` nothing in `folder` is named.
    fn untitled(&self, folder: &VaultPath, extension: &str) -> Result<VaultPath, Error> {
        let taken: Vec<String> = self
            .fs
            .list_dir(folder)?
            .into_iter()
            .map(|e| e.name)
            .collect();
        let name = (0..)
            .map(|n| match n {
                0 => format!("Untitled{extension}"),
                n => format!("Untitled {n}{extension}"),
            })
            .find(|name| !taken.contains(name))
            .expect("some Untitled name is free");
        Ok(folder.join(&name)?)
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

/// Saves a note's text to a file anywhere on disk, atomically, replacing
/// what's there: "Save elsewhere" after a save into the vault failed.
pub fn save_elsewhere(file: &Path, text: &str) -> Result<(), Error> {
    Ok(orbit_fs::write_atomic(file, text.as_bytes())?)
}

/// Whether a path is shown as Source mode only: `.txt` notes are plain text.
pub fn is_plain_text_note(path: &VaultPath) -> bool {
    path.name().to_ascii_lowercase().ends_with(".txt")
}

/// Whether a path is a tool folder or inside one.
fn in_tool_folder(path: &VaultPath) -> bool {
    path.as_str().split('/').any(is_tool_folder)
}

/// A name for a note or folder: one non-blank path segment that isn't a
/// tool folder.
fn check_name(name: &str) -> Result<(), Error> {
    let single_segment = VaultPath::new(name).is_ok_and(|p| !p.is_root() && p.name() == name);
    if single_segment && !name.trim().is_empty() && !is_tool_folder(name) {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_string()))
    }
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
