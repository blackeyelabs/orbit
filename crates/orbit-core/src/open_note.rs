//! A note open for editing, and what to do when another app changes it.
//!
//! The editor holds the text (ADR 0001); an [`OpenNote`] holds what Orbit
//! knows is on disk, so it can tell another app's change from Orbit's own,
//! never save over one, and say what the user has to decide.

use crate::{Error, Vault, VaultPath};

/// Which contents of a note Orbit last read or wrote. Compares contents,
/// not times: a touch isn't a change, and two writes in one second are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DiskVersion {
    len: usize,
    hash: u64,
}

impl DiskVersion {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::hash::DefaultHasher::new();
        bytes.hash(&mut hasher);
        DiskVersion {
            len: bytes.len(),
            hash: hasher.finish(),
        }
    }
}

/// Why an open note can't simply be saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// Another app changed the note while it had unsaved edits; this is
    /// their text. Load theirs or Keep mine.
    Changed(String),
    /// Another app deleted the note. Save a copy or close.
    Deleted,
    /// Another app changed the note into something that isn't UTF-8 text.
    Unreadable(String),
    /// Saving failed; the edits are still in memory. Try again.
    SaveFailed(String),
}

/// What [`OpenNote::check_disk`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskCheck {
    /// Nothing to do: the note on disk is what Orbit knows.
    Unchanged,
    /// Another app changed a note with no unsaved edits: show this text.
    Reload(String),
    /// See [`OpenNote::trouble`].
    Trouble,
}

pub struct OpenNote {
    path: VaultPath,
    /// What Orbit last read or wrote.
    version: DiskVersion,
    trouble: Option<Trouble>,
    /// The version behind a `Changed` or `Unreadable` trouble, which Keep
    /// mine and Load theirs may replace.
    theirs: Option<DiskVersion>,
}

impl OpenNote {
    pub(crate) fn new(path: VaultPath, version: DiskVersion) -> Self {
        OpenNote {
            path,
            version,
            trouble: None,
            theirs: None,
        }
    }

    pub fn path(&self) -> &VaultPath {
        &self.path
    }

    /// Follows the note after Orbit renames or moves it.
    pub fn set_path(&mut self, path: VaultPath) {
        self.path = path;
    }

    pub fn trouble(&self) -> Option<&Trouble> {
        self.trouble.as_ref()
    }

    /// Whether saving has to wait for the user's choice.
    pub fn is_blocked(&self) -> bool {
        matches!(
            self.trouble,
            Some(Trouble::Changed(_) | Trouble::Deleted | Trouble::Unreadable(_))
        )
    }

    /// Compares the note on disk with what Orbit knows, after the watcher
    /// saw it change. A note without unsaved edits (`has_edits`) reloads;
    /// otherwise the change becomes a [`Trouble`]. A note back at the known
    /// version (a sync undoing its change, say) has no trouble any more. A
    /// note that can't be read right now is left alone.
    pub fn check_disk(&mut self, vault: &Vault, has_edits: bool) -> DiskCheck {
        let bytes = match vault.read_disk(&self.path) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                self.trouble = Some(Trouble::Deleted);
                return DiskCheck::Trouble;
            }
            Err(_) => return DiskCheck::Unchanged,
        };
        let version = DiskVersion::of(&bytes);
        if version == self.version {
            if self.is_blocked() {
                self.trouble = None;
            }
            return DiskCheck::Unchanged;
        }
        let failed = matches!(self.trouble, Some(Trouble::SaveFailed(_)));
        match String::from_utf8(bytes) {
            Ok(text) if !has_edits && !failed => {
                self.version = version;
                self.trouble = None;
                DiskCheck::Reload(text)
            }
            Ok(text) => self.troubled(Trouble::Changed(text), version),
            Err(_) => self.troubled(
                Trouble::Unreadable(format!("{} isn't UTF-8 text any more", self.path)),
                version,
            ),
        }
    }

    fn troubled(&mut self, trouble: Trouble, theirs: DiskVersion) -> DiskCheck {
        self.trouble = Some(trouble);
        self.theirs = Some(theirs);
        DiskCheck::Trouble
    }

    /// Saves `text` unless another app changed or deleted the note since
    /// Orbit last read or wrote it; then the save is refused, nothing is
    /// overwritten, and the change becomes a [`Trouble`]. A failed write is a
    /// [`Trouble::SaveFailed`] until a save works.
    pub fn save(&mut self, vault: &Vault, text: &str) -> Result<(), Error> {
        match self.trouble {
            Some(Trouble::Deleted) => return Err(Error::DeletedOnDisk(self.path.clone())),
            Some(Trouble::Changed(_) | Trouble::Unreadable(_)) => {
                return Err(Error::ChangedOnDisk(self.path.clone()));
            }
            _ => {}
        }
        let expected = Some(self.version);
        self.write(vault, text, expected)
    }

    /// Load theirs: the text another app saved, which is now what Orbit
    /// knows. The editor's unsaved edits are dropped.
    pub fn load_theirs(&mut self) -> Option<String> {
        let Some(Trouble::Changed(text)) = self.trouble.clone() else {
            return None;
        };
        self.version = self.theirs.take()?;
        self.trouble = None;
        Some(text)
    }

    /// Keep mine: saves `text` over the other app's change that was shown,
    /// but not over a newer one, which becomes the trouble instead.
    pub fn keep_mine(&mut self, vault: &Vault, text: &str) -> Result<(), Error> {
        let expected = match self.trouble {
            Some(Trouble::Changed(_) | Trouble::Unreadable(_)) => self.theirs,
            Some(Trouble::Deleted) => None,
            _ => Some(self.version),
        };
        self.write(vault, text, expected)
    }

    /// Save a copy of a deleted note: `name (copy).md` beside where it was
    /// (or `name (copy 2).md`, …), which the note then is. The deleted note
    /// isn't recreated.
    pub fn save_copy(&mut self, vault: &Vault, text: &str) -> Result<VaultPath, Error> {
        let name = self.path.name();
        let (stem, extension) = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
            _ => (name, String::new()),
        };
        for n in 1.. {
            let copy_name = match n {
                1 => format!("{stem} (copy){extension}"),
                n => format!("{stem} (copy {n}){extension}"),
            };
            let copy = self.path.parent().join(&copy_name)?;
            // Only where nothing is, so nothing is ever replaced.
            if vault.save_note_if(&copy, text, &mut |current| current.is_none())? {
                self.path = copy.clone();
                self.version = DiskVersion::of(text.as_bytes());
                self.trouble = None;
                self.theirs = None;
                return Ok(copy);
            }
        }
        unreachable!("some copy name is free")
    }

    /// Writes `text` if what's on disk is still `expected` (`None`: nothing).
    fn write(
        &mut self,
        vault: &Vault,
        text: &str,
        expected: Option<DiskVersion>,
    ) -> Result<(), Error> {
        let written = vault.save_note_if(&self.path, text, &mut |current| {
            current.map(DiskVersion::of) == expected
        });
        match written {
            Ok(true) => {
                self.version = DiskVersion::of(text.as_bytes());
                self.trouble = None;
                self.theirs = None;
                Ok(())
            }
            // Someone else's change came first: show it instead.
            Ok(false) => {
                tracing::info!("another app changed a note before Orbit's save");
                tracing::debug!(note = %vault.log_path(&self.path), "the note another app changed");
                self.check_disk(vault, true);
                match self.trouble {
                    Some(Trouble::Deleted) => Err(Error::DeletedOnDisk(self.path.clone())),
                    _ => Err(Error::ChangedOnDisk(self.path.clone())),
                }
            }
            Err(e) => {
                tracing::warn!(error = e.kind_name(), "couldn't save a note");
                tracing::debug!(note = %vault.log_path(&self.path), "the note that couldn't be saved");
                self.trouble = Some(Trouble::SaveFailed(e.to_string()));
                Err(e)
            }
        }
    }
}
