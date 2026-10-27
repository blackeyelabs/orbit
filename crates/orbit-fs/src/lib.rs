//! The VaultFs trait with plain and protected-folder implementations.
//!
//! Everything above this crate reaches the disk only through [`VaultFs`], so a
//! protected folder can later be mounted into the same logical tree.

use std::{
    fs,
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime},
};

mod protected;

pub use protected::ProtectedFs;

use notify_debouncer_mini::{DebounceEventResult, new_debouncer, notify::RecursiveMode};

/// Where protected folders keep their encrypted storage, at the vault root
/// (docs/protected-folder-format.md).
pub const PROTECTED_STORAGE_DIR: &str = ".s";

/// Orbit's own folder at the vault root: session state and the lock.
pub const ORBIT_DIR: &str = ".vault";

/// How long the vault must be quiet before a batch of changes is reported:
/// a sync or `git checkout` touching many files arrives as one batch.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);

/// Called with the paths that changed in the vault, in batches.
pub type ChangeHandler = Box<dyn FnMut(Vec<VaultPath>) + Send>;

/// Watching continues until this is dropped.
pub struct Watch(#[allow(dead_code)] Box<dyn Send>);

/// The vault is held until this is dropped (or the process ends).
pub struct VaultLock(#[allow(dead_code)] Box<dyn Send + Sync>);

/// Name prefix of the temporary file an atomic write renames into place.
/// One left behind by a crash is never listed.
const SAVE_TEMP_PREFIX: &str = ".orbit-save-";

/// A path inside a vault: `/`-separated, relative to the vault root, never
/// escaping it. The root itself is the empty path.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VaultPath(String);

impl VaultPath {
    pub fn root() -> Self {
        VaultPath(String::new())
    }

    /// Parses a `/`-separated relative path, refusing `..`, `.`, empty
    /// segments, backslashes and absolute paths.
    pub fn new(path: &str) -> io::Result<Self> {
        if path.is_empty() {
            return Ok(Self::root());
        }
        let ok = !path.starts_with('/')
            && !path.contains('\\')
            && path
                .split('/')
                .all(|s| !s.is_empty() && s != "." && s != "..");
        if ok {
            Ok(VaultPath(path.to_string()))
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a vault path: {path:?}"),
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The last segment, or "" for the root.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or("")
    }

    /// The containing folder; the root's parent is the root.
    pub fn parent(&self) -> Self {
        VaultPath(self.0.rsplit_once('/').map_or("", |(p, _)| p).to_string())
    }

    /// Whether this is `folder` or something inside it, segment by segment.
    pub fn is_within(&self, folder: &VaultPath) -> bool {
        folder.is_root()
            || self == folder
            || self
                .0
                .strip_prefix(&folder.0)
                .is_some_and(|rest| rest.starts_with('/'))
    }

    /// Where this path is after `from` moved to `to`: itself unless it's
    /// `from` or inside it.
    pub fn rebase(&self, from: &VaultPath, to: &VaultPath) -> VaultPath {
        match self.0.strip_prefix(&from.0) {
            Some(rest) if self.is_within(from) && !from.is_root() => {
                VaultPath(format!("{}{rest}", to.0))
            }
            _ => self.clone(),
        }
    }

    pub fn join(&self, name: &str) -> io::Result<Self> {
        if self.is_root() {
            Self::new(name)
        } else {
            Self::new(&format!("{}/{name}", self.0))
        }
    }
}

impl std::fmt::Display for VaultPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryType {
    Dir,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub entry_type: EntryType,
    /// When it last changed, where that's known.
    pub modified: Option<SystemTime>,
}

/// Storage for a vault's files.
pub trait VaultFs: Send + Sync {
    /// Entries of a directory, in no particular order.
    fn list_dir(&self, dir: &VaultPath) -> io::Result<Vec<DirEntry>>;
    fn read(&self, file: &VaultPath) -> io::Result<Vec<u8>>;
    /// Replaces or creates a file atomically: afterwards it holds either the
    /// old contents or all of the new ones, even after a crash.
    fn write(&self, file: &VaultPath, contents: &[u8]) -> io::Result<()> {
        self.write_if(file, contents, &mut |_| true).map(|_| ())
    }
    /// Like [`Self::write`], but only if `may_replace` accepts what's there
    /// (`None` when nothing is), checked as late as possible: after the new
    /// contents are safely on disk, right before they replace the old.
    /// Returns whether it wrote.
    fn write_if(
        &self,
        file: &VaultPath,
        contents: &[u8],
        may_replace: &mut dyn FnMut(Option<&[u8]>) -> bool,
    ) -> io::Result<bool>;
    /// Creates an empty file; fails if anything already has that path.
    fn create_file(&self, file: &VaultPath) -> io::Result<()>;
    /// Creates a folder inside an existing one.
    fn create_dir(&self, dir: &VaultPath) -> io::Result<()>;
    /// Renames or moves a file or folder; fails if `to` already exists.
    fn rename(&self, from: &VaultPath, to: &VaultPath) -> io::Result<()>;
    /// Moves a file or folder to the system trash.
    fn trash(&self, path: &VaultPath) -> io::Result<()>;
    /// Reports changes to the vault's files, by any app, until the returned
    /// [`Watch`] is dropped.
    fn watch(&self, on_change: ChangeHandler) -> io::Result<Watch>;
    /// Holds the vault, so it's open in one window at a time: fails with
    /// `WouldBlock` while anything else holds it, in this process or another.
    fn lock(&self) -> io::Result<VaultLock>;
    /// A folder on the local disk, made if need be, where Orbit may keep
    /// the cache `name` (such as the search index) for this vault; `None`
    /// where its contents mustn't be kept on disk. The cache's owner reads
    /// and writes in it directly: the one exception to vault files going
    /// through this trait (ADR 0006).
    fn cache_dir(&self, _name: &str) -> Option<PathBuf> {
        None
    }
}

/// A vault folder on the local disk.
pub struct PlainFs {
    root: PathBuf,
}

impl PlainFs {
    pub fn new(root: &Path) -> io::Result<Self> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "vault root is not a folder",
            ));
        }
        Ok(PlainFs { root })
    }

    /// Makes a folder's entries survive a crash: those just made in it.
    pub fn sync_dir(&self, dir: &VaultPath) -> io::Result<()> {
        fs::File::open(self.resolve(dir)?)?.sync_all()
    }

    /// Deletes a file, or a folder and everything in it, for good: what
    /// protecting a folder leaves behind, and protected storage.
    pub fn remove_permanently(&self, path: &VaultPath) -> io::Result<()> {
        let target = self.entry_path(path)?;
        if target.symlink_metadata()?.is_dir() {
            fs::remove_dir_all(target)
        } else {
            fs::remove_file(target)
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The real path, refusing anything (via symlinks) outside the vault.
    fn resolve(&self, path: &VaultPath) -> io::Result<PathBuf> {
        let real = self.root.join(path.as_str()).canonicalize()?;
        if real.starts_with(&self.root) {
            Ok(real)
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "path leaves the vault",
            ))
        }
    }

    /// The real path of the entry itself, not following a symlink at the end
    /// of `path`. The vault root isn't an entry.
    fn entry_path(&self, path: &VaultPath) -> io::Result<PathBuf> {
        if path.is_root() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the vault root can't be moved or deleted",
            ));
        }
        Ok(self.resolve(&path.parent())?.join(path.name()))
    }

    /// Like [`Self::resolve`], but a file that doesn't exist yet resolves
    /// inside its (existing) folder. A dangling symlink is refused rather
    /// than replaced.
    fn resolve_for_write(&self, path: &VaultPath) -> io::Result<PathBuf> {
        match self.resolve(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound && !path.is_root() => {
                let target = self.resolve(&path.parent())?.join(path.name());
                if target.symlink_metadata().is_ok() {
                    return Err(e);
                }
                Ok(target)
            }
            resolved => resolved,
        }
    }
}

impl VaultFs for PlainFs {
    /// Skips entries it can't represent safely instead of failing the
    /// listing: names that aren't UTF-8 or valid vault paths, symlinks that
    /// leave the vault, and directory symlinks back to this folder or an
    /// ancestor (which would make the tree infinite).
    fn list_dir(&self, dir: &VaultPath) -> io::Result<Vec<DirEntry>> {
        let real_dir = self.resolve(dir)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&real_dir)?.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with(SAVE_TEMP_PREFIX) {
                continue;
            }
            let Ok(path) = dir.join(&name) else { continue };
            let Ok(real) = self.resolve(&path) else {
                continue;
            };
            let metadata = fs::metadata(&real).ok();
            let entry_type = if metadata.as_ref().is_some_and(fs::Metadata::is_dir) {
                EntryType::Dir
            } else {
                EntryType::File
            };
            let modified = metadata.and_then(|m| m.modified().ok());
            let is_link = entry.file_type().is_ok_and(|t| t.is_symlink());
            if is_link && entry_type == EntryType::Dir && real_dir.starts_with(&real) {
                continue;
            }
            entries.push(DirEntry {
                name,
                entry_type,
                modified,
            });
        }
        Ok(entries)
    }

    fn read(&self, file: &VaultPath) -> io::Result<Vec<u8>> {
        fs::read(self.resolve(file)?)
    }

    fn create_file(&self, file: &VaultPath) -> io::Result<()> {
        let target = self.resolve_for_write(file)?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        Ok(())
    }

    fn create_dir(&self, dir: &VaultPath) -> io::Result<()> {
        fs::create_dir(self.resolve_for_write(dir)?)
    }

    fn rename(&self, from: &VaultPath, to: &VaultPath) -> io::Result<()> {
        // Not `resolve`: a symlink is moved itself, not its target.
        let source = self.entry_path(from)?;
        let target = self.resolve_for_write(to)?;
        rename_no_replace(&source, &target)
    }

    fn trash(&self, path: &VaultPath) -> io::Result<()> {
        let target = self.entry_path(path)?;
        trash::delete(&target).map_err(io::Error::other)
    }

    /// Debounced and recursive. Orbit's own temporary save files aren't
    /// reported; paths that aren't valid vault paths are skipped.
    fn watch(&self, mut on_change: ChangeHandler) -> io::Result<Watch> {
        let root = self.root.clone();
        let mut debouncer = new_debouncer(WATCH_DEBOUNCE, move |events: DebounceEventResult| {
            // Events were lost (a full inotify queue, say): anything may have
            // changed.
            let Ok(events) = events else {
                return on_change(vec![VaultPath::root()]);
            };
            let mut paths: Vec<VaultPath> = events
                .iter()
                .filter_map(|event| vault_path(&root, &event.path))
                .filter(|path| !path.name().starts_with(SAVE_TEMP_PREFIX))
                .collect();
            paths.sort();
            paths.dedup();
            if !paths.is_empty() {
                on_change(paths);
            }
        })
        .map_err(io::Error::other)?;
        debouncer
            .watcher()
            .watch(&self.root, RecursiveMode::Recursive)
            .map_err(io::Error::other)?;
        Ok(Watch(Box::new(debouncer)))
    }

    /// An advisory lock on `.vault/lock`, which the system releases if
    /// Orbit crashes.
    fn lock(&self) -> io::Result<VaultLock> {
        let dir = self.root.join(ORBIT_DIR);
        fs::create_dir_all(&dir)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("lock"))?;
        file.try_lock().map_err(|e| match e {
            fs::TryLockError::WouldBlock => io::ErrorKind::WouldBlock.into(),
            fs::TryLockError::Error(e) => e,
        })?;
        Ok(VaultLock(Box::new(file)))
    }

    /// In the vault's `.vault/`, where Orbit keeps its own things.
    fn cache_dir(&self, name: &str) -> Option<PathBuf> {
        let dir = self.root.join(ORBIT_DIR).join(name);
        fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }

    /// Through a symlink, the link's target is replaced and the link kept.
    fn write_if(
        &self,
        file: &VaultPath,
        contents: &[u8],
        may_replace: &mut dyn FnMut(Option<&[u8]>) -> bool,
    ) -> io::Result<bool> {
        write_atomic_if(&self.resolve_for_write(file)?, contents, may_replace)
    }
}

/// Replaces or creates `target` atomically, anywhere on disk: a temporary
/// file beside it is written and fsynced, renamed over it, and the folder
/// fsynced.
pub fn write_atomic(target: &Path, contents: &[u8]) -> io::Result<()> {
    write_atomic_if(target, contents, &mut |_| true).map(|_| ())
}

/// [`write_atomic`], if `may_replace` accepts the current contents just
/// before the rename. Another app writing in between could still be
/// replaced; the window is a read and a rename.
fn write_atomic_if(
    target: &Path,
    contents: &[u8],
    may_replace: &mut dyn FnMut(Option<&[u8]>) -> bool,
) -> io::Result<bool> {
    let dir = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no folder to write in"))?;
    let mut temp = tempfile::Builder::new()
        .prefix(SAVE_TEMP_PREFIX)
        .tempfile_in(dir)?;
    temp.write_all(contents)?;
    // The temporary file is private (0600); keep the existing file's mode.
    if let Ok(meta) = fs::metadata(target) {
        temp.as_file().set_permissions(meta.permissions())?;
    }
    temp.as_file().sync_all()?;
    let current = match fs::read(target) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if !may_replace(current.as_deref()) {
        return Ok(false);
    }
    temp.persist(target).map_err(|e| e.error)?;
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(true)
}

/// Renames, failing with `AlreadyExists` rather than replacing `target`. On
/// Linux the kernel checks and renames in one step; where the filesystem
/// can't, and elsewhere, another app creating `target` between the check and
/// the rename could still be replaced.
fn rename_no_replace(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        match renameat_with(CWD, source, CWD, target, RenameFlags::NOREPLACE) {
            Err(rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS) => {}
            result => return result.map_err(io::Error::from),
        }
    }
    if target.symlink_metadata().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", target.display()),
        ));
    }
    fs::rename(source, target)
}

/// The vault path of a real path under `root`, if it is one.
fn vault_path(root: &Path, path: &Path) -> Option<VaultPath> {
    let segments: Option<Vec<&str>> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|c| match c {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    VaultPath::new(&segments?.join("/")).ok()
}
