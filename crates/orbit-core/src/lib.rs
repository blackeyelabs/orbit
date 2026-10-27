//! Vaults, notes, link resolution and front matter.
//!
//! [`Vault`] is the headless facade every layer above uses (and, later,
//! `orbit-api`); [`Settings`] holds the app-wide preferences beside it.
//! Neither has a GPUI dependency.

mod links;
mod open_note;
mod recent;
mod session;
mod settings;

use open_note::DiskVersion;
pub use open_note::{DiskCheck, OpenNote, Trouble};

pub use links::{Backlink, Mention};
pub use recent::{RecentVault, RecentVaults};
pub use session::{Session, SessionTab};
pub use settings::{Settings, ThemeChoice, default_config_dir};

use links::{FileLinks, Found, LinkIndex};
use session::SessionFile;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub use orbit_fs::VaultPath;

/// Where following a link leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Followed {
    pub path: VaultPath,
    /// The heading (or `^block`) the link names, to show.
    pub heading: Option<String>,
    /// The link was unresolved, and its note was just created.
    pub created: bool,
}
pub use orbit_fs::Watch;
use orbit_fs::{EntryType, ORBIT_DIR, PROTECTED_STORAGE_DIR, PlainFs, VaultFs, VaultLock};

/// Folders that belong to tools, not to the vault's content: Orbit's own
/// cache, Obsidian's settings, git, and protected-folder storage.
const TOOL_FOLDERS: &[&str] = &[ORBIT_DIR, ".obsidian", ".git", PROTECTED_STORAGE_DIR];

/// Session state, in Orbit's folder.
const SESSION_FILE: &str = "session.json";

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
    /// The vault is open already, in another window.
    AlreadyOpen,
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
            Error::AlreadyOpen => f.write_str("the vault is already open in another window"),
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
    root: PathBuf,
    name: String,
    fs: Box<dyn VaultFs>,
    /// `None` where the vault can't be written to, so can't be locked.
    _lock: Option<VaultLock>,
    /// Shared with the watcher, which reports what changes.
    links: Arc<Mutex<LinkIndex>>,
}

impl Vault {
    /// Opens the folder at `root` as a vault, holding it until the Vault is
    /// dropped: opening it again meanwhile, from this process or another,
    /// is [`Error::AlreadyOpen`]. A vault Orbit can't write to opens without
    /// that guard.
    pub fn open(root: &Path) -> Result<Vault, Error> {
        let plain = PlainFs::new(root).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Error::NotFound,
            _ => Error::Io(e),
        })?;
        let lock = match plain.lock() {
            Ok(lock) => Some(lock),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Err(Error::AlreadyOpen),
            Err(_) => None,
        };
        let root = plain.root().to_path_buf();
        let name = folder_name(&root);
        Ok(Vault {
            root,
            name,
            fs: Box::new(plain),
            _lock: lock,
            links: Arc::default(),
        })
    }

    /// The vault folder's real path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// What was open when the vault last closed: the tabs of notes that are
    /// still there. Missing or unreadable session state means none.
    pub fn session(&self) -> Session {
        let file: SessionFile = self
            .fs
            .read(&session_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        file.session(|path| self.is_restorable(path) && matches!(self.read_disk(path), Ok(Some(_))))
    }

    /// Remembers what's open, for [`Self::session`] next time. Only notes
    /// are kept. Protected notes must never be (they'd leak their names):
    /// once protected folders mount into the tree, [`Self::is_restorable`]
    /// has to refuse them.
    pub fn save_session(&self, session: &Session) -> Result<(), Error> {
        let file = SessionFile::of(session, |path| self.is_restorable(path));
        let json = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
        let dir = VaultPath::new(ORBIT_DIR)?;
        match self.fs.create_dir(&dir) {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e.into()),
            _ => {}
        }
        Ok(self.fs.write(&session_path(), &json)?)
    }

    /// Whether a tab can be part of the session: a note in the vault's content.
    fn is_restorable(&self, path: &VaultPath) -> bool {
        check_note(path).is_ok() && !in_tool_folder(path)
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
    /// Links resolve with these changes before `on_change` hears of them.
    pub fn watch(
        &self,
        mut on_change: impl FnMut(Vec<VaultPath>) + Send + 'static,
    ) -> Result<Watch, Error> {
        let links = self.links.clone();
        Ok(self.fs.watch(Box::new(move |paths: Vec<VaultPath>| {
            let paths: Vec<VaultPath> = paths.into_iter().filter(|p| !in_tool_folder(p)).collect();
            if !paths.is_empty() {
                link_index(&links).changed(paths.iter().cloned());
                on_change(paths);
            }
        }))?)
    }

    /// The note or file a link points at from the note `from`, as Obsidian
    /// resolves it: `link` is its path or name without `#heading`. A name
    /// no file has resolves to a note with that front matter alias. `None`
    /// is an unresolved link.
    pub fn resolve(&self, from: &VaultPath, link: &str) -> Option<VaultPath> {
        self.with_links(|files| files.resolve(from, link))
    }

    /// The notes that link to the note `to`, each with its links there in
    /// context, in path order: what the notes on disk say, so an open
    /// note's edits count once saved.
    pub fn backlinks(&self, to: &VaultPath) -> Vec<Backlink> {
        self.with_links(|files| files.backlinks(to))
    }

    /// Runs `f` on the link index, brought up to date.
    fn with_links<T>(&self, f: impl FnOnce(&links::Files) -> T) -> T {
        let mut index = link_index(&self.links);
        f(index.current(
            |folder| self.children_of(folder, 0).unwrap_or_default(),
            |path| self.find(path),
        ))
    }

    /// Builds the index links resolve with, which reads every markdown
    /// note's front matter; otherwise the first link resolved builds it.
    pub fn index_links(&self) {
        self.resolve(&VaultPath::root(), "");
    }

    /// What's at `path` now, for the link index: only notes are read.
    fn find(&self, path: &VaultPath) -> Found {
        if path.is_root() {
            return Found::Folder;
        }
        if in_tool_folder(path) {
            return Found::Nothing;
        }
        let entries = match self.fs.list_dir(&path.parent()) {
            Ok(entries) => entries,
            // Its folder went with it.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Found::Nothing,
            Err(_) => return Found::Unreadable,
        };
        match entries.iter().find(|e| e.name == path.name()) {
            None => Found::Nothing,
            Some(e) if e.entry_type == EntryType::Dir => Found::Folder,
            Some(_) if check_note(path).is_err() => Found::File(FileLinks::default()),
            Some(_) => match self.read_disk(path) {
                Ok(Some(bytes)) => {
                    Found::File(FileLinks::of_note(path, &String::from_utf8_lossy(&bytes)))
                }
                Ok(None) => Found::Nothing,
                Err(_) => Found::Unreadable,
            },
        }
    }

    /// Follows a link clicked in the note `from` (`Note#Heading`): to the
    /// note or file it resolves to, or, unresolved, to the note
    /// [`Self::create_linked_note`] makes for it.
    pub fn follow_link(&self, from: &VaultPath, target: &str) -> Result<Followed, Error> {
        let (link, heading) = md_engine::split_target(target);
        let heading = heading.map(str::to_string);
        if let Some(path) = self.resolve(from, link) {
            return Ok(Followed {
                path,
                heading,
                created: false,
            });
        }
        let path = self.create_linked_note(from, link)?;
        Ok(Followed {
            path,
            heading,
            created: true,
        })
    }

    /// Where a vault file is on the local disk, for opening it in its app.
    pub fn file_on_disk(&self, path: &VaultPath) -> PathBuf {
        self.root.join(path.as_str())
    }

    /// Following an unresolved link: creates its (empty) note. A name goes
    /// at the vault root, a path where it says, with any folders it needs,
    /// and `./` or `../` from `from`'s folder. Without a note's extension,
    /// `.md` is added, but a missing image or other Attachment isn't made a
    /// note. Nothing that exists is replaced.
    pub fn create_linked_note(&self, from: &VaultPath, link: &str) -> Result<VaultPath, Error> {
        let invalid = || Error::InvalidName(link.to_string());
        let from_folder = from.parent();
        let mut segments: Vec<&str> = Vec::new();
        let relative = link.starts_with("./") || link.starts_with("../");
        if relative {
            segments.extend(from_folder.as_str().split('/').filter(|s| !s.is_empty()));
        }
        for segment in link.trim_start_matches('/').split('/') {
            match segment {
                "." if relative => {}
                ".." if relative => {
                    segments.pop().ok_or_else(invalid)?;
                }
                name => {
                    check_name(name)?;
                    segments.push(name);
                }
            }
        }
        let mut path = VaultPath::new(&segments.join("/")).map_err(|_| invalid())?;
        if path.is_root() {
            return Err(invalid());
        }
        if is_attachment_name(path.name()) {
            return Err(Error::NotANote(path));
        }
        if EntryKind::of_file(path.name()) != EntryKind::Note {
            path = path.parent().join(&format!("{}.md", path.name()))?;
        }
        if self.read_disk(&path)?.is_some() {
            return Err(Error::AlreadyExists(path));
        }
        let mut folder = VaultPath::root();
        for name in segments.iter().take(segments.len() - 1) {
            folder = folder.join(name)?;
            match self.fs.create_dir(&folder) {
                Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e.into()),
                _ => {}
            }
        }
        match self.fs.create_file(&path) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(Error::AlreadyExists(path));
            }
            result => result?,
        }
        self.touched([path.clone()]);
        Ok(path)
    }

    /// Orbit changed these paths: links see it now, not when the watcher
    /// reports it.
    fn touched(&self, paths: impl IntoIterator<Item = VaultPath>) {
        link_index(&self.links).changed(paths);
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
        let written = self.fs.write_if(path, text.as_bytes(), may_replace)?;
        // Its aliases may have changed.
        self.touched([path.clone()]);
        Ok(written)
    }

    /// Creates an empty note named `Untitled.md` (or `Untitled 1.md`, …) in
    /// `folder`.
    pub fn create_note(&self, folder: &VaultPath) -> Result<VaultPath, Error> {
        let path = self.untitled(folder, ".md")?;
        self.fs.create_file(&path)?;
        self.touched([path.clone()]);
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
        self.fs.trash(path)?;
        self.touched([path.clone()]);
        Ok(())
    }

    fn move_entry(&self, from: &VaultPath, to: VaultPath) -> Result<VaultPath, Error> {
        match self.fs.rename(from, &to) {
            Ok(()) => {
                self.touched([from.clone(), to.clone()]);
                Ok(to)
            }
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

    /// The contents of the Attachment an image link or embed in `note`
    /// points to, resolved as any link is (percent-decoded, without any
    /// `#fragment` or `?query`). Remote, missing and unresolved links give
    /// `None`; nothing outside the vault is ever read.
    pub fn read_linked_attachment(&self, note: &VaultPath, link: &str) -> Option<Vec<u8>> {
        let path = link.split(['#', '?']).next()?;
        if path.is_empty() || md_engine::has_scheme(path) {
            return None;
        }
        let target = self.resolve(note, &md_engine::percent_decode(path)?)?;
        self.fs.read(&target).ok()
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

/// A folder's name for display; the whole path if it has none (`/`).
fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The link index, locked. One whose holder panicked mid-update is
/// started afresh, to be rebuilt.
fn link_index(links: &Mutex<LinkIndex>) -> std::sync::MutexGuard<'_, LinkIndex> {
    links.lock().unwrap_or_else(|poisoned| {
        let mut index = poisoned.into_inner();
        *index = LinkIndex::default();
        index
    })
}

/// A file name that's plainly an Attachment's: an image, audio, video or PDF.
fn is_attachment_name(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some(
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "svg"
                | "bmp"
                | "avif"
                | "pdf"
                | "mp3"
                | "wav"
                | "ogg"
                | "m4a"
                | "flac"
                | "mp4"
                | "webm"
                | "mov"
                | "mkv"
        )
    )
}

fn session_path() -> VaultPath {
    VaultPath::new(&format!("{ORBIT_DIR}/{SESSION_FILE}")).expect("a valid vault path")
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
