//! The vaults Orbit opened lately, newest first, for the Welcome screen.
//! App-wide like [`crate::Settings`]: `recent-vaults.json` in the config
//! folder. Only vault roots are kept, never the notes in them.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

const FILE_NAME: &str = "recent-vaults.json";
const MAX_RECENT: usize = 10;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentVaults {
    paths: Vec<PathBuf>,
}

/// One entry of the list, as the Welcome screen shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentVault {
    pub path: PathBuf,
    /// The folder's name.
    pub name: String,
    /// False once the folder has moved or gone: "Can't find …".
    pub found: bool,
}

impl RecentVaults {
    /// Reads the list from `config_dir`; anything unreadable means none.
    pub fn load(config_dir: &Path) -> RecentVaults {
        let paths = fs::read(config_dir.join(FILE_NAME))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<PathBuf>>(&bytes).ok())
            .unwrap_or_default();
        let mut recent = RecentVaults::default();
        for path in paths.into_iter().rev() {
            recent.add(&path);
        }
        recent
    }

    pub fn save(&self, config_dir: &Path) -> io::Result<()> {
        fs::create_dir_all(config_dir)?;
        let json = serde_json::to_vec_pretty(&self.paths).map_err(io::Error::other)?;
        orbit_fs::write_atomic(&config_dir.join(FILE_NAME), &json)
    }

    /// Newest first.
    pub fn vaults(&self) -> Vec<RecentVault> {
        self.paths
            .iter()
            .map(|path| RecentVault::at(path))
            .collect()
    }

    /// Puts a vault at the top: just opened.
    pub fn add(&mut self, root: &Path) {
        let root = canonical(root);
        self.paths.retain(|p| *p != root);
        self.paths.insert(0, root);
        self.paths.truncate(MAX_RECENT);
    }

    /// A vault that moved, found again at `new_root`: the old entry goes and
    /// the new one is at the top.
    pub fn replace(&mut self, old_root: &Path, new_root: &Path) {
        let old_root = canonical(old_root);
        self.paths.retain(|p| *p != old_root);
        self.add(new_root);
    }
}

impl RecentVault {
    /// The vault at `root`, as it is now.
    pub fn at(root: &Path) -> Self {
        RecentVault {
            path: root.to_path_buf(),
            name: crate::folder_name(root),
            found: root.is_dir(),
        }
    }
}

/// The real path where there is one, so one vault is listed once however
/// it was reached; a vault that's gone keeps the path it had.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
