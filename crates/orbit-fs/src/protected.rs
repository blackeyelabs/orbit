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
    /// Protected folders locked since the vault opened. (Those never
    /// unlocked meanwhile, Orbit can't know the paths of.)
    locked: RwLock<Vec<Locked>>,
    /// Held while a header is rewritten.
    header_writes: std::sync::Mutex<()>,
}

/// A file or folder on its way into, out of or between protected folders:
/// where it goes, and its contents (none for a folder).
struct Moving {
    path: VaultPath,
    kind: EntryType,
    contents: zeroize::Zeroizing<Vec<u8>>,
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

/// A protected folder that's locked: where it is, and the files it held,
/// which nothing is written at while it's locked (a save landing after the
/// lock would be their text, in plain). New files there are another
/// matter: like any, they're plain.
struct Locked {
    root: VaultPath,
    files: std::collections::HashSet<VaultPath>,
}

/// An edit sealed into a blob under its protected folder's key, ready to
/// write without the key: an edit saved as its folder locks (ADR 0004).
/// Only ciphertext; written later, the newest blob for a path still wins.
pub struct Sealed {
    store: VaultPath,
    id: String,
    blob: Vec<u8>,
    /// The blob it replaces, gone once it's written.
    replaces: Option<String>,
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
            header_writes: std::sync::Mutex::default(),
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
    /// Anything plain made at its path while it was locked is renamed
    /// `<name> (unprotected copy)`, returned beside it.
    pub fn mount(
        &self,
        store: VaultPath,
        master: MasterKey,
    ) -> io::Result<(VaultPath, Option<VaultPath>)> {
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
            // Of two blobs for one path (left by a crash mid-save, or a
            // sealed edit written late), the newer is the note, and the
            // older goes.
            let older = match entries.get(&path) {
                Some(kept) if kept.mtime_ms >= entry.mtime_ms => Some(entry.blob),
                _ => entries.insert(path, entry).map(|replaced| replaced.blob),
            };
            if let Some(older) = older {
                self.plain.remove_permanently(&blobs.join(&older)?).ok();
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
        let copy = match self.plain_entry_at(&root)? {
            true => {
                let copy = self.free_copy_name(&mounts, &root)?;
                self.plain.rename(&root, &copy)?;
                Some(copy)
            }
            false => None,
        };
        lock_list(&self.locked).retain(|l| l.root != root);
        mounts.push(Mount {
            store,
            root: root.clone(),
            master,
            entries,
        });
        Ok((root, copy))
    }

    /// Locks the protected folder at `root`: its key is dropped (and
    /// zeroized), and it leaves the tree. Whether it was unlocked.
    pub fn unmount(&self, root: &VaultPath) -> bool {
        let mut mounts = self.mounts_mut();
        let Some(ix) = mounts.iter().position(|m| &m.root == root) else {
            return false;
        };
        let mount = mounts.remove(ix);
        let files = mount
            .entries
            .iter()
            .filter(|(_, e)| e.kind == EntryType::File)
            .map(|(path, _)| path.clone())
            .collect();
        lock_list(&self.locked).push(Locked {
            root: root.clone(),
            files,
        });
        true
    }

    /// Refuses writing the file `path` if a locked protected folder held
    /// it: it would land in plain text. Said as for any taken path, so it
    /// gives nothing away.
    fn check_not_locked(&self, path: &VaultPath) -> io::Result<()> {
        let locked = self.locked.read().unwrap_or_else(|e| e.into_inner());
        if locked.iter().any(|l| l.files.contains(path)) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        Ok(())
    }

    /// Refuses moving or deleting a plain folder that holds a protected one,
    /// which would leave it behind.
    fn check_holds_none(&self, path: &VaultPath) -> io::Result<()> {
        self.check_holds_none_of(&self.mounts(), path)
    }

    /// [`Self::check_holds_none`], with the mounts in hand.
    fn check_holds_none_of(&self, mounts: &[Mount], path: &VaultPath) -> io::Result<()> {
        let holds = mounts
            .iter()
            .any(|m| m.root.is_within(path) && &m.root != path)
            || lock_list(&self.locked)
                .iter()
                .any(|l| l.root.is_within(path));
        if holds {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "it holds a protected folder, which can't be moved or deleted with it yet",
            ));
        }
        Ok(())
    }

    /// Seals `contents` as the note `path`, in an unlocked protected folder,
    /// under the folder's key, writing nothing: for [`Self::write_sealed`]
    /// once the folder is locked.
    pub fn seal_edit(&self, path: &VaultPath, contents: &[u8]) -> io::Result<Sealed> {
        let mounts = self.mounts();
        let ix = Self::mount_index(&mounts, path).ok_or(io::ErrorKind::NotFound)?;
        let mount = &mounts[ix];
        let blob = orbit_crypto::seal(
            &mount.master,
            &Metadata::new(path.to_string(), now_ms()),
            contents,
            &mut orbit_crypto::os_rng(),
        )
        .map_err(crypto)?;
        Ok(Sealed {
            store: mount.store.clone(),
            id: orbit_crypto::blob_id(&blob).map_err(crypto)?,
            blob,
            replaces: mount.entries.get(path).map(|e| e.blob.clone()),
        })
    }

    /// Writes a sealed edit into its folder's storage, while the folder is
    /// locked (no key is needed: it's ciphertext); then the blob it
    /// replaces goes. On the next unlock the newer blob is the note's.
    pub fn write_sealed(&self, sealed: &Sealed) -> io::Result<()> {
        if self.mounts().iter().any(|m| m.store == sealed.store) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let blobs = sealed.store.join("blobs")?;
        self.plain.write(&blobs.join(&sealed.id)?, &sealed.blob)?;
        if let Some(old) = &sealed.replaces {
            match self.plain.remove_permanently(&blobs.join(old)?) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
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
        mounted.or_else(|| {
            lock_list(&self.locked)
                .iter()
                .map(|l| &l.root)
                .find(|r| near(r))
                .cloned()
        })
    }

    /// Rewrites the header of the unlocked protected folder at `root` with
    /// `rewrite`, given the header and the folder's key: a new passphrase
    /// or recovery key. The blobs stay as they are.
    pub fn rewrite_header<T>(
        &self,
        root: &VaultPath,
        rewrite: impl FnOnce(&Header, &MasterKey) -> io::Result<(Header, T)>,
    ) -> io::Result<T> {
        // One rewrite at a time, so none is lost to another.
        let _one = self.header_writes.lock().unwrap_or_else(|e| e.into_inner());
        let (file, header, master) = {
            let mounts = self.mounts();
            let mount = mounts
                .iter()
                .find(|m| &m.root == root)
                .ok_or(io::ErrorKind::NotFound)?;
            let file = mount.store.join("header")?;
            let header = Header::from_bytes(self.plain.read(&file)?);
            (file, header, mount.master.clone())
        };
        // Not while holding the mounts: a new passphrase runs Argon2, and
        // locking mustn't wait for it (ADR 0004).
        let (header, out) = rewrite(&header, &master)?;
        // Its storage gone meanwhile (protection removed), this fails.
        self.plain.write(&file, header.as_bytes())?;
        Ok(out)
    }

    /// Makes the unlocked protected folder at `root` a plain folder again:
    /// everything in it is written in plain text, then its storage is
    /// deleted, header first (without it, the rest is never read again).
    /// Fails, leaving it protected, if anything plain is at its path or a
    /// write fails.
    pub fn unprotect(&self, root: &VaultPath) -> io::Result<()> {
        let mut mounts = self.mounts_mut();
        let ix = mounts
            .iter()
            .position(|m| &m.root == root)
            .ok_or(io::ErrorKind::NotFound)?;
        if self.plain_entry_at(root)? {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let mount = &mounts[ix];
        let written = (|| {
            // Folders come before what's in them.
            for (path, entry) in &mount.entries {
                match entry.kind {
                    EntryType::Dir => self.plain.create_dir(path)?,
                    EntryType::File => self.plain.write(path, &self.open(mount, entry)?)?,
                }
            }
            self.plain.sync_dir(&root.parent())
        })();
        let unprotected =
            written.and_then(|()| self.plain.remove_permanently(&mount.store.join("header")?));
        if let Err(e) = unprotected {
            self.plain.remove_permanently(root).ok();
            return Err(e);
        }
        let mount = mounts.remove(ix);
        // The blobs are inert now; any left over are harmless ciphertext.
        self.plain.remove_permanently(&mount.store).ok();
        Ok(())
    }

    /// Moves `from` into, out of or between protected folders (each side's
    /// mount, or `None` for plain): it's written at `to`, sealed or in
    /// plain text, then deleted outright from `from`; a plain copy isn't
    /// left in the trash. Fails, changing nothing, if anything is at `to`.
    fn move_across(
        &self,
        mounts: &mut [Mount],
        (from, from_mount): (&VaultPath, Option<usize>),
        (to, to_mount): (&VaultPath, Option<usize>),
    ) -> io::Result<()> {
        let moving: Vec<Moving> = match from_mount {
            Some(ix) => {
                let mount = &mounts[ix];
                let entries: Vec<_> = mount
                    .entries
                    .iter()
                    .filter(|(path, _)| path.is_within(from))
                    .collect();
                if entries.is_empty() {
                    return Err(io::ErrorKind::NotFound.into());
                }
                entries
                    .into_iter()
                    .map(|(path, entry)| {
                        let contents = match entry.kind {
                            EntryType::File => self.open(mount, entry)?,
                            EntryType::Dir => zeroize::Zeroizing::new(Vec::new()),
                        };
                        Ok(Moving {
                            path: path.rebase(from, to),
                            kind: entry.kind,
                            contents,
                        })
                    })
                    .collect::<io::Result<_>>()?
            }
            None => {
                self.check_holds_none_of(mounts, from)?;
                let mut moving = Vec::new();
                self.read_plain_tree(from, to, &mut moving)?;
                moving
            }
        };
        // Nothing replaced, nor written where a locked folder's note was.
        let taken = match to_mount {
            Some(ix) => mounts[ix].entries.contains_key(to),
            None => {
                self.plain_entry_at(to)?
                    || mounts.iter().any(|m| &m.root == to)
                    || moving
                        .iter()
                        .any(|m| self.check_not_locked(&m.path).is_err())
            }
        };
        if taken {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        match to_mount {
            Some(ix) => {
                let mount = &mut mounts[ix];
                if !matches!(mount.entries.get(&to.parent()), Some(e) if e.kind == EntryType::Dir) {
                    return Err(io::ErrorKind::NotFound.into());
                }
                for m in &moving {
                    if let Err(e) = self.seal(mount, &m.path, m.kind, &m.contents) {
                        // Undone: what was sealed of it goes.
                        self.remove_within(mount, to).ok();
                        return Err(e);
                    }
                }
            }
            None => {
                let written = moving.iter().try_for_each(|m| match m.kind {
                    EntryType::Dir => self.plain.create_dir(&m.path),
                    EntryType::File => self.plain.write(&m.path, &m.contents),
                });
                if let Err(e) = written {
                    self.plain.remove_permanently(to).ok();
                    return Err(e);
                }
            }
        }
        match from_mount {
            Some(ix) => self.remove_within(&mut mounts[ix], from),
            None => self.plain.remove_permanently(from),
        }
    }

    /// Deletes `path` and everything in it from `mount`, blobs and all.
    fn remove_within(&self, mount: &mut Mount, path: &VaultPath) -> io::Result<()> {
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

    /// The plain file or folder `path` and everything in it, read for
    /// writing at `to`, folders before what they hold.
    fn read_plain_tree(
        &self,
        path: &VaultPath,
        to: &VaultPath,
        out: &mut Vec<Moving>,
    ) -> io::Result<()> {
        let kind = self
            .plain
            .list_dir(&path.parent())?
            .into_iter()
            .find(|e| e.name == path.name())
            .ok_or(io::ErrorKind::NotFound)?
            .entry_type;
        match kind {
            EntryType::File => out.push(Moving {
                path: to.clone(),
                kind,
                contents: zeroize::Zeroizing::new(self.plain.read(path)?),
            }),
            EntryType::Dir => {
                out.push(Moving {
                    path: to.clone(),
                    kind,
                    contents: zeroize::Zeroizing::default(),
                });
                for entry in self.plain.list_dir(path)? {
                    self.read_plain_tree(&path.join(&entry.name)?, &to.join(&entry.name)?, out)?;
                }
            }
        }
        Ok(())
    }

    /// The first `<name> (unprotected copy)` (or `… copy 2`, …) free beside
    /// the plain `path`: nothing plain there, nor a protected folder,
    /// unlocked (in `mounts`) or locked.
    fn free_copy_name(&self, mounts: &[Mount], path: &VaultPath) -> io::Result<VaultPath> {
        for n in 1.. {
            let name = match n {
                1 => format!("{} (unprotected copy)", path.name()),
                n => format!("{} (unprotected copy {n})", path.name()),
            };
            let copy = path.parent().join(&name)?;
            let protected = mounts.iter().any(|m| m.root == copy)
                || lock_list(&self.locked).iter().any(|l| l.root == copy);
            if !protected && !self.plain_entry_at(&copy)? {
                return Ok(copy);
            }
        }
        unreachable!("some copy name is free")
    }

    /// Whether the plain tree has anything at `path`.
    fn plain_entry_at(&self, path: &VaultPath) -> io::Result<bool> {
        match self.plain.list_dir(&path.parent()) {
            Ok(entries) => Ok(entries.iter().any(|e| e.name == path.name())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Whether `path` is in (or is) a protected folder, unlocked or locked
    /// since the vault opened: what logs mustn't name.
    pub fn is_protected(&self, path: &VaultPath) -> bool {
        self.mounts().iter().any(|m| path.is_within(&m.root))
            || lock_list(&self.locked)
                .iter()
                .any(|l| path.is_within(&l.root))
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
    /// is sealed again at its new path. Into, out of or between protected
    /// folders, see [`Self::move_across`]; a protected folder never moves
    /// into another.
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
            _ if renaming_root => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "a protected folder can't hold another",
                ));
            }
            (from_mount, to_mount) => {
                return self.move_across(&mut mounts, (from, from_mount), (to, to_mount));
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
        self.remove_within(mount, path)
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

fn lock_list<T>(list: &RwLock<Vec<T>>) -> RwLockWriteGuard<'_, Vec<T>> {
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
