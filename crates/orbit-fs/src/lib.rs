//! The VaultFs trait with plain and protected-folder implementations.
//!
//! Everything above this crate reaches the disk only through [`VaultFs`], so a
//! protected folder can later be mounted into the same logical tree.

use std::{
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

/// Where protected folders keep their encrypted storage, at the vault root
/// (docs/protected-folder-format.md).
pub const PROTECTED_STORAGE_DIR: &str = ".s";

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
}

/// Storage for a vault's files.
pub trait VaultFs: Send + Sync {
    /// Entries of a directory, in no particular order.
    fn list_dir(&self, dir: &VaultPath) -> io::Result<Vec<DirEntry>>;
    fn read(&self, file: &VaultPath) -> io::Result<Vec<u8>>;
    /// Replaces or creates a file atomically: afterwards it holds either the
    /// old contents or all of the new ones, even after a crash.
    fn write(&self, file: &VaultPath, contents: &[u8]) -> io::Result<()>;
    /// Creates an empty file; fails if anything already has that path.
    fn create_file(&self, file: &VaultPath) -> io::Result<()>;
    /// Creates a folder inside an existing one.
    fn create_dir(&self, dir: &VaultPath) -> io::Result<()>;
    /// Renames or moves a file or folder; fails if `to` already exists.
    fn rename(&self, from: &VaultPath, to: &VaultPath) -> io::Result<()>;
    /// Moves a file or folder to the system trash.
    fn trash(&self, path: &VaultPath) -> io::Result<()>;
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
            let entry_type = if real.is_dir() {
                EntryType::Dir
            } else {
                EntryType::File
            };
            let is_link = entry.file_type().is_ok_and(|t| t.is_symlink());
            if is_link && entry_type == EntryType::Dir && real_dir.starts_with(&real) {
                continue;
            }
            entries.push(DirEntry { name, entry_type });
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

    /// Writes a temporary file beside the target, fsyncs it, renames it over
    /// the target and fsyncs the folder. Through a symlink, the link's target
    /// is replaced and the link kept.
    fn write(&self, file: &VaultPath, contents: &[u8]) -> io::Result<()> {
        let target = self.resolve_for_write(file)?;
        let dir = target.parent().unwrap_or(&self.root);
        let mut temp = tempfile::Builder::new()
            .prefix(SAVE_TEMP_PREFIX)
            .tempfile_in(dir)?;
        temp.write_all(contents)?;
        // The temporary file is private (0600); keep the note's own mode.
        if let Ok(meta) = fs::metadata(&target) {
            temp.as_file().set_permissions(meta.permissions())?;
        }
        temp.as_file().sync_all()?;
        temp.persist(&target).map_err(|e| e.error)?;
        #[cfg(unix)]
        fs::File::open(dir)?.sync_all()?;
        Ok(())
    }
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
