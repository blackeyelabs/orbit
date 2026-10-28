//! Vaults, notes, link resolution and front matter.
//!
//! [`Vault`] is the headless facade every layer above uses (and, later,
//! `orbit-api`); [`Settings`] holds the app-wide preferences beside it.
//! Neither has a GPUI dependency.

mod canvas;
mod excerpt;
mod fuzzy;
mod index;
mod open_note;
mod protect;
mod recent;
mod search;
mod session;
mod settings;
mod suggest;
mod tags;

use open_note::DiskVersion;
pub use open_note::{DiskCheck, OpenNote, Trouble};
pub use orbit_crypto::Zeroizing;
pub use protect::{
    LockOutcome, ProtectionKeys, Strength, UnlockError, UnprotectedCopy, passphrase_strength,
};

pub use excerpt::Snippet;
pub use fuzzy::{CommandMatch, NoteMatch, command_query, match_commands};
pub use index::{Backlink, Mention};
pub use recent::{RecentVault, RecentVaults};
pub use search::{IndexingProgress, SearchResult, SearchResults};
pub use session::{Session, SessionTab};
pub use settings::{Settings, ThemeChoice, default_config_dir, default_state_dir};
pub use suggest::{LinkSuggestion, LinkSuggestions};
pub use tags::{TagCount, same_tag};

use index::{Found, IndexedFile, VaultIndex};
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
use orbit_fs::{
    EntryType, ORBIT_DIR, PROTECTED_STORAGE_DIR, PlainFs, ProtectedFs, VaultFs, VaultLock,
};

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
    /// Notes and canvases are searched; other files aren't.
    pub(crate) fn is_searchable(self) -> bool {
        matches!(self, EntryKind::Note | EntryKind::Canvas)
    }

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
    /// A protected folder can't hold another: this one is in the way.
    Nested(VaultPath),
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

impl Error {
    /// What went wrong, without the paths its message may name: for logs
    /// (ADR 0007).
    pub fn kind_name(&self) -> &'static str {
        match self {
            Error::NotFound => "not found",
            Error::AlreadyOpen => "already open",
            Error::Nested(_) => "nested protected folder",
            Error::NotANote(_) => "not a note",
            Error::NotUtf8(_) => "not UTF-8",
            Error::AlreadyExists(_) => "already exists",
            Error::ChangedOnDisk(_) => "changed on disk",
            Error::DeletedOnDisk(_) => "deleted on disk",
            Error::IntoItself(_) => "into itself",
            Error::InvalidName(_) => "invalid name",
            Error::Io(_) => "I/O error",
        }
    }
}

/// A path as logs may show it (ADR 0007): as it is, or `<protected>` for
/// anything in a protected folder. From [`Vault::log_path`].
pub struct LogPath(Option<VaultPath>);

impl std::fmt::Display for LogPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(path) => f.write_str(path.as_str()),
            None => f.write_str("<protected>"),
        }
    }
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
            Error::Nested(p) => write!(
                f,
                "{p} is protected, and protected folders can't hold one another"
            ),
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
    fs: ProtectedFs,
    protection: protect::Protection,
    /// `None` where the vault can't be written to, so can't be locked.
    _lock: Option<VaultLock>,
    /// Shared with the watcher, which reports what changes.
    index: Arc<Mutex<VaultIndex>>,
    /// Vault search, told of changes the same way.
    search: Arc<search::Search>,
}

impl Vault {
    /// Opens the folder at `root` as a vault, holding it until the Vault is
    /// dropped: opening it again meanwhile, from this process or another,
    /// is [`Error::AlreadyOpen`]. A vault Orbit can't write to opens without
    /// that guard.
    pub fn open(root: &Path) -> Result<Vault, Error> {
        let _span = tracing::debug_span!("vault.open").entered();
        let plain = PlainFs::new(root).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Error::NotFound,
            _ => Error::Io(e),
        })?;
        let lock = match plain.lock() {
            Ok(lock) => Some(lock),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Err(Error::AlreadyOpen),
            Err(e) => {
                tracing::warn!(error = %e, "couldn't hold the vault: another window could open it too");
                None
            }
        };
        let root = plain.root().to_path_buf();
        let name = folder_name(&root);
        tracing::info!(vault = %name, "vault opened");
        Ok(Vault {
            root,
            name,
            fs: ProtectedFs::new(plain),
            protection: protect::Protection::default(),
            _lock: lock,
            index: Arc::default(),
            search: Arc::default(),
        })
    }

    /// `path` as logs may show it: hidden if it's in a protected folder,
    /// unlocked or locked since the vault opened (ADR 0007).
    pub fn log_path(&self, path: &VaultPath) -> LogPath {
        LogPath((!self.fs.is_protected(path)).then(|| path.clone()))
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
        check_note(path).is_ok()
            && !in_tool_folder(path)
            && self.fs.protected_folder_of(path).is_none()
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

    /// Whether the vault has notes, for its empty state: a note or canvas
    /// anywhere in the tree, or a locked protected folder (which holds
    /// some, unseen). Attachments alone don't count.
    pub fn has_notes(&self) -> bool {
        fn any_searchable(nodes: &[TreeNode]) -> bool {
            nodes
                .iter()
                .any(|n| n.kind.is_searchable() || any_searchable(&n.children))
        }
        any_searchable(&self.tree().unwrap_or_default())
            || !self.fs.locked().unwrap_or_default().is_empty()
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
    /// Links and search see these changes before `on_change` hears of them.
    pub fn watch(
        &self,
        mut on_change: impl FnMut(Vec<VaultPath>) + Send + 'static,
    ) -> Result<Watch, Error> {
        let (index, search) = (self.index.clone(), self.search.clone());
        tracing::debug!("watching the vault for changes");
        Ok(self.fs.watch(Box::new(move |paths: Vec<VaultPath>| {
            let paths: Vec<VaultPath> = paths.into_iter().filter(|p| !in_tool_folder(p)).collect();
            if !paths.is_empty() {
                tracing::trace!(changed = paths.len(), "the vault changed");
                lock_index(&index).changed(paths.iter().cloned());
                search.changed(paths.iter().cloned());
                on_change(paths);
            }
        }))?)
    }

    /// The note or file a link points at from the note `from`, as Obsidian
    /// resolves it: `link` is its path or name without `#heading`. A name
    /// no file has resolves to a note with that front matter alias. `None`
    /// is an unresolved link.
    pub fn resolve(&self, from: &VaultPath, link: &str) -> Option<VaultPath> {
        self.with_index(|files| files.resolve(from, link))
    }

    /// The notes that link to the note `to`, each with its links there in
    /// context, in path order: what the notes on disk say, so an open
    /// note's edits count once saved.
    pub fn backlinks(&self, to: &VaultPath) -> Vec<Backlink> {
        self.with_index(|files| files.backlinks(to))
    }

    /// Every tag in the vault's notes, by name ignoring case, with how
    /// often it's written: as Obsidian's tag pane counts them.
    pub fn tags(&self) -> Vec<TagCount> {
        self.with_index(|files| files.tags())
    }

    /// The notes with `tag` (with or without `#`) or a tag nested in it,
    /// ignoring case, in path order.
    pub fn notes_tagged(&self, tag: &str) -> Vec<VaultPath> {
        self.with_index(|files| files.notes_tagged(tag))
    }

    /// Up to `limit` notes for the Quick switcher's `query`, fuzzy-matched
    /// on their paths and aliases ignoring case, best first.
    pub fn find_notes(&self, query: &str, limit: usize) -> Vec<NoteMatch> {
        self.with_index(|files| fuzzy::find_notes(files, query, limit))
    }

    /// The text Orbit writes a link to `path` with, between `[[` and `]]`:
    /// the shortest unique path, as Obsidian writes it.
    pub fn link_text(&self, path: &VaultPath) -> String {
        self.with_index(|files| files.link_text(path))
    }

    /// What a `[[query` typed in the note `from` suggests: up to `limit`
    /// notes, fuzzy-matched on their paths and aliases ignoring case, best
    /// first; and the query as a new note's name if no link resolves to it.
    pub fn link_suggestions(&self, from: &VaultPath, query: &str, limit: usize) -> LinkSuggestions {
        let query = query.trim();
        self.with_index(|files| LinkSuggestions {
            notes: suggest::suggest(files, query, limit),
            new_note: (!query.is_empty() && files.resolve(from, query).is_none())
                .then(|| query.to_string()),
        })
    }

    /// Runs `f` on the vault index, brought up to date.
    fn with_index<T>(&self, f: impl FnOnce(&index::Files) -> T) -> T {
        let mut index = lock_index(&self.index);
        f(index.current(
            |folder| self.children_of(folder, 0).unwrap_or_default(),
            |path| self.find(path),
        ))
    }

    /// Builds the vault index, which reads every note; otherwise the first
    /// link resolved, or other question asked of it, builds it.
    pub fn index_links(&self) {
        self.resolve(&VaultPath::root(), "");
    }

    /// What's at `path` now, for the vault index: only notes are read.
    fn find(&self, path: &VaultPath) -> Found {
        if path.is_root() {
            return Found::Folder;
        }
        if in_tool_folder(path) {
            return Found::Nothing;
        }
        let Ok(entry) = self.entry_at(path) else {
            return Found::Unreadable;
        };
        match entry {
            None => Found::Nothing,
            Some(e) if e.entry_type == EntryType::Dir => Found::Folder,
            Some(_) if check_note(path).is_err() => Found::File(IndexedFile::default()),
            Some(_) => match self.read_disk(path) {
                Ok(Some(bytes)) => {
                    Found::File(IndexedFile::of_note(path, &String::from_utf8_lossy(&bytes)))
                }
                Ok(None) => Found::Nothing,
                Err(_) => Found::Unreadable,
            },
        }
    }

    /// What's at `path` (not the root) in its folder's listing; `None` if
    /// nothing is, or its folder went too.
    fn entry_at(&self, path: &VaultPath) -> io::Result<Option<orbit_fs::DirEntry>> {
        match self.fs.list_dir(&path.parent()) {
            Ok(entries) => Ok(entries.into_iter().find(|e| e.name == path.name())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
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

    /// Orbit changed these paths: links and search see it now, not when
    /// the watcher reports it.
    fn touched(&self, paths: impl IntoIterator<Item = VaultPath>) {
        let paths: Vec<VaultPath> = paths.into_iter().collect();
        lock_index(&self.index).changed(paths.iter().cloned());
        self.search.changed(paths);
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
        tracing::debug!(note = %self.log_path(path), written, "saved");
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
    /// its extension unless the new name has one of the same kind (`.md` to
    /// `.txt`, say): a note never becomes a canvas or an attachment, nor a
    /// canvas a note. Links to it aren't rewritten yet.
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
                    && EntryKind::of_file(new_name) != old_kind =>
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
        let shown = self.log_path(path);
        self.fs.trash(path)?;
        tracing::debug!(entry = %shown, "deleted");
        self.touched([path.clone()]);
        Ok(())
    }

    fn move_entry(&self, from: &VaultPath, to: VaultPath) -> Result<VaultPath, Error> {
        // Before the move: a protected folder moved is no longer at `from`.
        let shown_from = self.log_path(from);
        match self.fs.rename(from, &to) {
            Ok(()) => {
                tracing::debug!(from = %shown_from, to = %self.log_path(&to), "moved");
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

/// The vault index, locked. One whose holder panicked mid-update is
/// started afresh, to be rebuilt.
fn lock_index(index: &Mutex<VaultIndex>) -> std::sync::MutexGuard<'_, VaultIndex> {
    index.lock().unwrap_or_else(|poisoned| {
        let mut index = poisoned.into_inner();
        *index = VaultIndex::default();
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

/// A note's title: its file name without `.md` (only lowercase, as in
/// Obsidian); any other file's whole name.
pub fn note_title(path: &VaultPath) -> &str {
    without_md(path.name())
}

/// A path or name without a trailing `.md`.
fn without_md(path: &str) -> &str {
    path.strip_suffix(".md").unwrap_or(path)
}

/// Whether a path is shown as Source mode only: `.txt` notes are plain text.
pub fn is_plain_text_note(path: &VaultPath) -> bool {
    path.name().to_ascii_lowercase().ends_with(".txt")
}

/// Notes this large (2 MB) or larger open as source only, to keep typing
/// fast; the preview is one choice away.
pub const LARGE_NOTE_BYTES: usize = 2_000_000;

/// Whether a note's text is large enough to open as source only.
pub fn is_large_note(text: &str) -> bool {
    text.len() >= LARGE_NOTE_BYTES
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
