//! Protected folders mounted into the vault's tree. A protected folder is
//! stored as `.s/<id>/header` and one blob per note, attachment and folder
//! under `.s/<id>/blobs/` (docs/protected-folder-format.md); each blob
//! holds its vault path, so unlocking rebuilds the folder from the blobs
//! alone (ADR 0002). While a folder is unlocked its notes are listed, read
//! and written at their vault paths like plain ones; locked, it isn't in
//! the tree at all.

use std::{
    collections::BTreeMap,
    io,
    path::PathBuf,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use orbit_crypto::{Header, MasterKey, Metadata};
use rand_core::Rng as _;

use crate::{
    ChangeHandler, DirEntry, EntryType, PROTECTED_STORAGE_DIR, PlainFs, VaultFs, VaultLock,
    VaultPath, Watch,
};

/// A folder's blob names its path with this at the end.
const FOLDER_MARK: char = '/';

/// The vault's plain files, with its unlocked protected folders mounted in.
pub struct ProtectedFs {
    plain: PlainFs,
    mounts: RwLock<Vec<Mount>>,
    /// Protected folders locked since the vault opened: nothing is written
    /// at their paths while they're locked, where it would be plain text.
    /// (Those never unlocked meanwhile, Orbit can't know the paths of.)
    locked: RwLock<Vec<VaultPath>>,
}

/// An unlocked protected folder.
struct Mount {
    /// Its storage: `.s/<id>`.
    store: VaultPath,
    /// Where it is in the vault.
    root: VaultPath,
    master: MasterKey,
    /// Everything in it, itself included, by vault path.
    entries: BTreeMap<VaultPath, Entry>,
}

#[derive(Clone)]
struct Entry {
    /// The blob's id: its file name under `blobs/`.
    blob: String,
    kind: EntryType,
    mtime_ms: i64,
}

impl ProtectedFs {
    pub fn new(plain: PlainFs) -> Self {
        ProtectedFs {
            plain,
            mounts: RwLock::default(),
            locked: RwLock::default(),
        }
    }

    pub fn root(&self) -> &std::path::Path {
        self.plain.root()
    }

    /// The headers of the protected folders that aren't unlocked, each with
    /// its storage.
    pub fn locked(&self) -> io::Result<Vec<(VaultPath, Header)>> {
        let storage = VaultPath::new(PROTECTED_STORAGE_DIR)?;
        let entries = match self.plain.list_dir(&storage) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            entries => entries?,
        };
        let mounted: Vec<VaultPath> = self.mounts().iter().map(|m| m.store.clone()).collect();
        let mut locked = Vec::new();
        for entry in entries
            .into_iter()
            .filter(|e| e.entry_type == EntryType::Dir)
        {
            let store = storage.join(&entry.name)?;
            if mounted.contains(&store) {
                continue;
            }
            if let Ok(bytes) = self.plain.read(&store.join("header")?) {
                locked.push((store, Header::from_bytes(bytes)));
            }
        }
        Ok(locked)
    }

    /// Mounts the protected folder stored at `store`, its key in hand:
    /// reads every blob's path and returns where the folder is. Of two
    /// blobs for one path (left by a crash mid-save), the newer counts.
    pub fn mount(&self, store: VaultPath, master: MasterKey) -> io::Result<VaultPath> {
        let blobs = store.join("blobs")?;
        let mut entries: BTreeMap<VaultPath, Entry> = BTreeMap::new();
        for file in self.plain.list_dir(&blobs)? {
            let Ok(bytes) = self.plain.read(&blobs.join(&file.name)?) else {
                continue;
            };
            let Ok(metadata) = orbit_crypto::open_metadata(&master, &bytes) else {
                continue;
            };
            let (path, kind) = match metadata.path().strip_suffix(FOLDER_MARK) {
                Some(folder) => (folder, EntryType::Dir),
                None => (metadata.path(), EntryType::File),
            };
            let Ok(path) = VaultPath::new(path) else {
                continue;
            };
            let entry = Entry {
                blob: file.name,
                kind,
                mtime_ms: metadata.mtime_ms(),
            };
            match entries.get(&path) {
                Some(kept) if kept.mtime_ms >= entry.mtime_ms => {}
                _ => {
                    entries.insert(path, entry);
                }
            }
        }
        // The folder itself is the shallowest folder in it, and the only one
        // that shallow; everything else is inside it.
        let depth = |path: &VaultPath| path.as_str().split('/').count();
        let folders: Vec<&VaultPath> = entries
            .iter()
            .filter(|(_, e)| e.kind == EntryType::Dir)
            .map(|(path, _)| path)
            .collect();
        let shallowest = folders.iter().map(|p| depth(p)).min();
        let mut roots = folders.into_iter().filter(|p| Some(depth(p)) == shallowest);
        let (Some(root), None) = (roots.next().cloned(), roots.next()) else {
            return Err(invalid("a protected folder without one root"));
        };
        if root
            .as_str()
            .split('/')
            .any(|segment| segment.starts_with('.'))
        {
            return Err(invalid("a protected folder in a tool folder"));
        }
        entries.retain(|path, _| path.is_within(&root));
        let mut mounts = self.mounts_mut();
        if mounts
            .iter()
            .any(|m| root.is_within(&m.root) || m.root.is_within(&root))
        {
            return Err(invalid("a protected folder in another's place"));
        }
        lock_list(&self.locked).retain(|p| p != &root);
        mounts.push(Mount {
            store,
            root: root.clone(),
            master,
            entries,
        });
        Ok(root)
    }

    /// Locks the protected folder at `root`: its key is dropped (and
    /// zeroized), and it leaves the tree. Whether it was unlocked.
    pub fn unmount(&self, root: &VaultPath) -> bool {
        let mut mounts = self.mounts_mut();
        let before = mounts.len();
        mounts.retain(|m| &m.root != root);
        let unmounted = mounts.len() < before;
        if unmounted {
            lock_list(&self.locked).push(root.clone());
        }
        unmounted
    }

    /// Refuses writing at `path` if it's in a protected folder that's
    /// locked: it would land in plain text.
    fn check_not_locked(&self, path: &VaultPath) -> io::Result<()> {
        let locked = self.locked.read().unwrap_or_else(|e| e.into_inner());
        if locked.iter().any(|root| path.is_within(root)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "it's in a locked protected folder",
            ));
        }
        Ok(())
    }

    /// Refuses moving or deleting a plain folder that holds a protected one,
    /// which would leave it behind.
    fn check_holds_none(&self, path: &VaultPath) -> io::Result<()> {
        let holds = self
            .mounts()
            .iter()
            .any(|m| m.root.is_within(path) && &m.root != path)
            || lock_list(&self.locked)
                .iter()
                .any(|root| root.is_within(path));
        if holds {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "it holds a protected folder, which can't be moved or deleted with it yet",
            ));
        }
        Ok(())
    }

    /// The unlocked protected folders, by where they are.
    pub fn mounted(&self) -> Vec<VaultPath> {
        self.mounts().iter().map(|m| m.root.clone()).collect()
    }

    /// A protected folder, unlocked or locked since the vault opened, that
    /// `folder` is in, is, or holds: one it can't be protected beside.
    pub fn protected_near(&self, folder: &VaultPath) -> Option<VaultPath> {
        let near = |root: &VaultPath| folder.is_within(root) || root.is_within(folder);
        let mounted = self
            .mounts()
            .iter()
            .map(|m| m.root.clone())
            .find(|r| near(r));
        mounted.or_else(|| lock_list(&self.locked).iter().find(|r| near(r)).cloned())
    }

    /// The unlocked protected folder `path` is in (or is), if any.
    pub fn protected_folder_of(&self, path: &VaultPath) -> Option<VaultPath> {
        self.mounts()
            .iter()
            .find(|m| path.is_within(&m.root))
            .map(|m| m.root.clone())
    }

    /// Makes the plain folder `folder` a protected folder with `header` and
    /// its `master` key: everything in it is sealed into blobs, then the
    /// plain files are deleted outright (not to the trash, which would keep
    /// them), and the folder is mounted, unlocked.
    pub fn protect(
        &self,
        folder: &VaultPath,
        header: &Header,
        master: MasterKey,
    ) -> io::Result<()> {
        let storage = VaultPath::new(PROTECTED_STORAGE_DIR)?;
        match self.plain.create_dir(&storage) {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
        let mut rng = orbit_crypto::os_rng();
        let mut id = [0u8; 16];
        rng.fill_bytes(&mut id);
        let store = storage.join(&hex(&id))?;
        self.plain.create_dir(&store)?;
        self.plain.create_dir(&store.join("blobs")?)?;
        self.plain
            .write(&store.join("header")?, header.as_bytes())?;
        // On disk for good before any plain file is deleted.
        for dir in [VaultPath::root(), storage.clone(), store.clone()] {
            self.plain.sync_dir(&dir)?;
        }
        let mut mount = Mount {
            store: store.clone(),
            root: folder.clone(),
            master,
            entries: BTreeMap::new(),
        };
        let sealed = self.seal_tree(&mut mount, folder);
        if let Err(e) = sealed {
            // Nothing half-protected is left: the plain folder is intact.
            self.plain.remove_permanently(&store).ok();
            return Err(e);
        }
        // Mounted even if deleting the plain files fails part way: the
        // folder is protected either way, and it's unlocked.
        let removed = self.plain.remove_permanently(folder);
        self.mounts_mut().push(mount);
        removed
    }

    /// Seals the plain `folder` and everything in it into `mount`.
    fn seal_tree(&self, mount: &mut Mount, folder: &VaultPath) -> io::Result<()> {
        self.seal(mount, folder, EntryType::Dir, &[])?;
        for entry in self.plain.list_dir(folder)? {
            let path = folder.join(&entry.name)?;
            match entry.entry_type {
                EntryType::Dir => self.seal_tree(mount, &path)?,
                EntryType::File => {
                    let contents = zeroize::Zeroizing::new(self.plain.read(&path)?);
                    self.seal(mount, &path, EntryType::File, &contents)?;
                }
            }
        }
        Ok(())
    }

    /// Writes `path`'s new blob, then lets go of the old one.
    fn seal(
        &self,
        mount: &mut Mount,
        path: &VaultPath,
        kind: EntryType,
        contents: &[u8],
    ) -> io::Result<()> {
        let mtime_ms = now_ms();
        let name = match kind {
            EntryType::Dir => format!("{path}{FOLDER_MARK}"),
            EntryType::File => path.to_string(),
        };
        let blob = orbit_crypto::seal(
            &mount.master,
            &Metadata::new(name, mtime_ms),
            contents,
            &mut orbit_crypto::os_rng(),
        )
        .map_err(crypto)?;
        let id = orbit_crypto::blob_id(&blob).map_err(crypto)?;
        self.plain.write(&mount.blob_path(&id)?, &blob)?;
        let old = mount.entries.insert(
            path.clone(),
            Entry {
                blob: id,
                kind,
                mtime_ms,
            },
        );
        if let Some(old) = old {
            self.plain
                .remove_permanently(&mount.blob_path(&old.blob)?)?;
        }
        Ok(())
    }

    /// A blob's contents, decrypted.
    fn open(&self, mount: &Mount, entry: &Entry) -> io::Result<zeroize::Zeroizing<Vec<u8>>> {
        let bytes = self.plain.read(&mount.blob_path(&entry.blob)?)?;
        let (_, contents) = orbit_crypto::open(&mount.master, &bytes).map_err(crypto)?;
        Ok(contents)
    }

    fn mounts(&self) -> RwLockReadGuard<'_, Vec<Mount>> {
        self.mounts.read().unwrap_or_else(|e| e.into_inner())
    }

    fn mounts_mut(&self) -> RwLockWriteGuard<'_, Vec<Mount>> {
        self.mounts.write().unwrap_or_else(|e| e.into_inner())
    }

    /// The index of the mount `path` is in, if any.
    fn mount_index(mounts: &[Mount], path: &VaultPath) -> Option<usize> {
        mounts.iter().position(|m| path.is_within(&m.root))
    }
}

impl VaultFs for ProtectedFs {
    /// A protected folder's entries are its blobs'; elsewhere, the plain
    /// entries and any unlocked protected folder there (which takes the
    /// place of a plain one of the same name).
    fn list_dir(&self, dir: &VaultPath) -> io::Result<Vec<DirEntry>> {
        let mounts = self.mounts();
        if let Some(ix) = Self::mount_index(&mounts, dir) {
            let mount = &mounts[ix];
            match mount.entries.get(dir) {
                Some(e) if e.kind == EntryType::Dir => {}
                Some(_) => return Err(io::ErrorKind::NotADirectory.into()),
                None => return Err(io::ErrorKind::NotFound.into()),
            }
            return Ok(mount
                .entries
                .iter()
                .filter(|(path, _)| !path.is_root() && path.parent() == *dir && *path != dir)
                .map(|(path, e)| DirEntry {
                    name: path.name().to_string(),
                    entry_type: e.kind,
                    modified: Some(time_of(e.mtime_ms)),
                })
                .collect());
        }
        let here: Vec<&Mount> = mounts.iter().filter(|m| m.root.parent() == *dir).collect();
        let mut entries: Vec<DirEntry> = self
            .plain
            .list_dir(dir)?
            .into_iter()
            .filter(|e| !here.iter().any(|m| m.root.name() == e.name))
            .collect();
        entries.extend(here.iter().map(|m| DirEntry {
            name: m.root.name().to_string(),
            entry_type: EntryType::Dir,
            modified: m.entries.get(&m.root).map(|e| time_of(e.mtime_ms)),
        }));
        Ok(entries)
    }

    fn read(&self, file: &VaultPath) -> io::Result<Vec<u8>> {
        let mounts = self.mounts();
        let Some(ix) = Self::mount_index(&mounts, file) else {
            return self.plain.read(file);
        };
        let mount = &mounts[ix];
        match mount.entries.get(file) {
            Some(e) if e.kind == EntryType::File => Ok(self.open(mount, e)?.to_vec()),
            Some(_) => Err(io::ErrorKind::IsADirectory.into()),
            None => Err(io::ErrorKind::NotFound.into()),
        }
    }

    fn write_if(
        &self,
        file: &VaultPath,
        contents: &[u8],
        may_replace: &mut dyn FnMut(Option<&[u8]>) -> bool,
    ) -> io::Result<bool> {
        let mut mounts = self.mounts_mut();
        let Some(ix) = Self::mount_index(&mounts, file) else {
            drop(mounts);
            self.check_not_locked(file)?;
            return self.plain.write_if(file, contents, may_replace);
        };
        let mount = &mut mounts[ix];
        if file == &mount.root {
            return Err(io::ErrorKind::IsADirectory.into());
        }
        if !matches!(mount.entries.get(&file.parent()), Some(e) if e.kind == EntryType::Dir) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let current = match mount.entries.get(file) {
            Some(e) if e.kind == EntryType::Dir => return Err(io::ErrorKind::IsADirectory.into()),
            Some(e) => Some(self.open(mount, &e.clone())?),
            None => None,
        };
        if !may_replace(current.as_deref().map(Vec::as_slice)) {
            return Ok(false);
        }
        self.seal(mount, file, EntryType::File, contents)?;
        Ok(true)
    }

    fn create_file(&self, file: &VaultPath) -> io::Result<()> {
        if self.protected_folder_of(file).is_none() {
            self.check_not_locked(file)?;
            return self.plain.create_file(file);
        }
        let created = self.write_if(file, &[], &mut |existing| existing.is_none())?;
        if created {
            Ok(())
        } else {
            Err(io::ErrorKind::AlreadyExists.into())
        }
    }

    fn create_dir(&self, dir: &VaultPath) -> io::Result<()> {
        let mut mounts = self.mounts_mut();
        let Some(ix) = Self::mount_index(&mounts, dir) else {
            drop(mounts);
            self.check_not_locked(dir)?;
            return self.plain.create_dir(dir);
        };
        let mount = &mut mounts[ix];
        if mount.entries.contains_key(dir) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        if !matches!(mount.entries.get(&dir.parent()), Some(e) if e.kind == EntryType::Dir) {
            return Err(io::ErrorKind::NotFound.into());
        }
        self.seal(mount, dir, EntryType::Dir, &[])
    }

    /// Within one protected folder, or the folder itself: everything moved
    /// is sealed again at its new path. Into or out of a protected folder
    /// isn't possible yet.
    fn rename(&self, from: &VaultPath, to: &VaultPath) -> io::Result<()> {
        let mut mounts = self.mounts_mut();
        let from_mount = Self::mount_index(&mounts, from);
        let renaming_root = from_mount.is_some_and(|ix| mounts[ix].root == *from);
        let to_mount = match renaming_root {
            // The protected folder itself moves among plain folders.
            true => Self::mount_index(&mounts, &to.parent()).map(|_| usize::MAX),
            false => Self::mount_index(&mounts, to),
        };
        let ix = match (from_mount, to_mount) {
            (None, None) => {
                drop(mounts);
                self.check_not_locked(to)?;
                self.check_holds_none(from)?;
                return self.plain.rename(from, to);
            }
            (Some(ix), None) if renaming_root => ix,
            (Some(ix), Some(to)) if ix == to => ix,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "moving into or out of a protected folder isn't possible yet",
                ));
            }
        };
        let taken = if renaming_root {
            self.plain
                .list_dir(&to.parent())?
                .iter()
                .any(|e| e.name == to.name())
                || mounts.iter().any(|m| m.root == *to)
        } else {
            mounts[ix].entries.contains_key(to)
        };
        if taken {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let mount = &mut mounts[ix];
        if !renaming_root
            && !matches!(mount.entries.get(&to.parent()), Some(e) if e.kind == EntryType::Dir)
        {
            return Err(io::ErrorKind::NotFound.into());
        }
        let moving: Vec<(VaultPath, Entry)> = mount
            .entries
            .iter()
            .filter(|(path, _)| path.is_within(from))
            .map(|(path, e)| (path.clone(), e.clone()))
            .collect();
        for (path, entry) in moving {
            let contents = match entry.kind {
                EntryType::File => self.open(mount, &entry)?,
                EntryType::Dir => zeroize::Zeroizing::new(Vec::new()),
            };
            let moved = path.rebase(from, to);
            self.seal(mount, &moved, entry.kind, &contents)?;
            mount.entries.remove(&path);
            self.plain
                .remove_permanently(&mount.blob_path(&entry.blob)?)?;
        }
        if renaming_root {
            mount.root = to.clone();
        }
        Ok(())
    }

    /// In a protected folder, deleted outright: the system trash would keep
    /// plaintext. The protected folder itself goes with its storage.
    fn trash(&self, path: &VaultPath) -> io::Result<()> {
        let mut mounts = self.mounts_mut();
        let Some(ix) = Self::mount_index(&mounts, path) else {
            drop(mounts);
            self.check_holds_none(path)?;
            return self.plain.trash(path);
        };
        if mounts[ix].root == *path {
            let mount = mounts.remove(ix);
            return self.plain.remove_permanently(&mount.store);
        }
        let mount = &mut mounts[ix];
        if !mount.entries.contains_key(path) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let gone: Vec<VaultPath> = mount
            .entries
            .keys()
            .filter(|p| p.is_within(path))
            .cloned()
            .collect();
        for p in gone {
            let entry = mount.entries.remove(&p).expect("listed");
            self.plain
                .remove_permanently(&mount.blob_path(&entry.blob)?)?;
        }
        Ok(())
    }

    fn watch(&self, on_change: ChangeHandler) -> io::Result<Watch> {
        self.plain.watch(on_change)
    }

    fn lock(&self) -> io::Result<VaultLock> {
        self.plain.lock()
    }

    fn cache_dir(&self, name: &str) -> Option<PathBuf> {
        self.plain.cache_dir(name)
    }
}

impl Mount {
    /// Where a blob of this folder is stored.
    fn blob_path(&self, blob: &str) -> io::Result<VaultPath> {
        self.store.join("blobs")?.join(blob)
    }
}

fn lock_list(list: &RwLock<Vec<VaultPath>>) -> RwLockWriteGuard<'_, Vec<VaultPath>> {
    list.write().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn time_of(mtime_ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(mtime_ms.max(0) as u64)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

fn crypto(e: orbit_crypto::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}"))
}
