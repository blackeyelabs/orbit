//! Property types set by hand (ADR 0010). The vault's are saved in
//! `.vault/types.json`, as Obsidian's `types.json` holds them; one set in
//! a protected note is saved in that folder's own `.vault/types.json`,
//! which its encrypted storage seals, and used only while it's unlocked.
//! Types are never guessed into these files.

use std::{collections::BTreeMap, io, sync::atomic::Ordering};

use md_engine::PropertyType;
use orbit_fs::{ORBIT_DIR, VaultFs as _};

use crate::{Error, Vault, VaultPath};

const FILE_NAME: &str = "types.json";

/// The types set by hand that apply in one note: its protected folder's,
/// over the vault's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PropertyTypes(Vec<(String, PropertyType)>);

impl PropertyTypes {
    /// The type set for `key`, matched ignoring case as Obsidian does.
    pub fn get(&self, key: &str) -> Option<PropertyType> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, t)| *t)
    }
}

/// A `types.json`: each key's type name, a name Orbit doesn't know kept
/// as it is.
#[derive(Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct TypesFile {
    #[serde(default)]
    types: BTreeMap<String, String>,
}

impl Vault {
    /// The property types set by hand that apply in the note at `note`.
    pub fn property_types(&self, note: &VaultPath) -> PropertyTypes {
        let mut types = Vec::new();
        // The folder's first, so they win.
        let folders = self
            .fs
            .protected_folder_of(note)
            .into_iter()
            .chain([VaultPath::root()]);
        for folder in folders {
            let file = self.read_types(&folder).unwrap_or_default();
            types.extend(
                file.types
                    .into_iter()
                    .filter_map(|(k, name)| Some((k, PropertyType::from_name(&name)?))),
            );
        }
        PropertyTypes(types)
    }

    /// Sets `key`'s type, by hand, for the vault; or for the protected
    /// folder `note` is in, and only there: never for a note in a locked
    /// one, whose type would land in the plain file.
    pub fn set_property_type(
        &self,
        note: &VaultPath,
        key: &str,
        kind: PropertyType,
    ) -> Result<(), Error> {
        let folder = match self.fs.protected_folder_of(note) {
            Some(folder) => folder,
            None if self.fs.is_protected(note) => return Err(Error::NotFound),
            None => VaultPath::root(),
        };
        let mut file = self.read_types(&folder).unwrap_or_default();
        set_in(&mut file, key, kind.name());
        self.write_types(&folder, &file)?;
        self.types_revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// A protected folder's types, as its protection is removed: they
    /// join the vault's (it's plain now, so they may), the vault's own
    /// kept where both have a key. Its own file goes.
    pub(crate) fn move_types_to_vault(&self, folder: &VaultPath, types: TypesFile) {
        if types.types.is_empty() {
            return;
        }
        let mut vault = self.read_types(&VaultPath::root()).unwrap_or_default();
        for (key, name) in types.types {
            if !vault.types.keys().any(|k| k.eq_ignore_ascii_case(&key)) {
                vault.types.insert(key, name);
            }
        }
        if let Err(e) = self.write_types(&VaultPath::root(), &vault) {
            tracing::warn!(
                error = e.kind_name(),
                "couldn't keep a folder's property types"
            );
            return;
        }
        // Plain now, it's an ordinary file on disk.
        let own = self.root().join(folder.as_str()).join(ORBIT_DIR);
        std::fs::remove_file(own.join(FILE_NAME)).ok();
        std::fs::remove_dir(own).ok();
        self.types_revision.fetch_add(1, Ordering::Relaxed);
    }

    fn write_types(&self, folder: &VaultPath, file: &TypesFile) -> Result<(), Error> {
        let json = serde_json::to_vec_pretty(file).map_err(io::Error::other)?;
        let dir = folder.join(ORBIT_DIR)?;
        match self.fs.create_dir(&dir) {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e.into()),
            _ => {}
        }
        Ok(self.fs.write(&dir.join(FILE_NAME)?, &json)?)
    }

    /// Goes up each time a type is set, so what shows types can show them
    /// again.
    pub fn property_types_revision(&self) -> u64 {
        self.types_revision.load(Ordering::Relaxed)
    }

    /// The `types.json` in `folder`'s `.vault`, if there's one to read.
    pub(crate) fn read_types(&self, folder: &VaultPath) -> Option<TypesFile> {
        let path = folder.join(ORBIT_DIR).ok()?.join(FILE_NAME).ok()?;
        let bytes = self.fs.read(&path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// Sets `key`'s type to `name`, under the name the key was first given
/// a type by, in any case, as Obsidian keeps it.
fn set_in(file: &mut TypesFile, key: &str, name: &str) {
    let first = file
        .types
        .keys()
        .find(|k| k.eq_ignore_ascii_case(key))
        .cloned()
        .unwrap_or_else(|| key.to_string());
    file.types.insert(first, name.to_string());
}
