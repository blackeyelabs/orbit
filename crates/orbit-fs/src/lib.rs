//! The VaultFs trait with plain and protected-folder implementations.
//!
//! Everything above this crate reaches the disk only through [`VaultFs`], so a
//! protected folder can later be mounted into the same logical tree.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Where protected folders keep their encrypted storage, at the vault root
/// (docs/protected-folder-format.md).
pub const PROTECTED_STORAGE_DIR: &str = ".s";

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
}
