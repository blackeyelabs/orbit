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
