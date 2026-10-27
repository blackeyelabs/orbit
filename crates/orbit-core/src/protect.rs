//! Protected folders (ADR 0002, ADR 0003): protecting a folder, unlocking
//! blind by passphrase or recovery key, and locking. The storage and the
//! mounting into the tree are orbit-fs's; the rules are here.

use std::{
    collections::HashMap,
    str::FromStr as _,
    sync::Mutex,
    time::{Duration, Instant},
};

use orbit_crypto::{Header, KdfParams, MasterKey, PassphraseKey, RecoveryKey, SecretString};
use orbit_fs::{EntryType, Sealed, VaultPath};

use crate::{Error, Vault, in_tool_folder, search};

/// The first wait after a wrong attempt, doubled after each one after.
const FIRST_WAIT: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(5 * 60);

/// A new protected folder's keys, made before it's protected so its
/// recovery key can be shown and stored first.
pub struct ProtectionKeys {
    header: Header,
    recovery: RecoveryKey,
    master: MasterKey,
}

impl ProtectionKeys {
    /// The recovery key, as the person stores it.
    pub fn recovery_key(&self) -> orbit_crypto::Zeroizing<String> {
        self.recovery.reveal()
    }
}

/// Why an unlock didn't happen.
#[derive(Debug, PartialEq, Eq)]
pub enum UnlockError {
    /// No locked folder opened: the passphrase or recovery key is wrong, or
    /// there's nothing to unlock (the answer is the same). The next attempt
    /// may be made after `retry_in`.
    Wrong { retry_in: Duration },
    /// An attempt came before the wait after the last wrong one was over;
    /// it wasn't tried.
    TooSoon(Duration),
}

impl std::fmt::Display for UnlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnlockError::Wrong { .. } => f.write_str("that didn't unlock anything"),
            UnlockError::TooSoon(wait) => {
                write!(f, "you can try again in {} seconds", wait.as_secs().max(1))
            }
        }
    }
}

/// Wrong attempts so far, and when the next may be made.
#[derive(Default)]
pub(crate) struct Attempts {
    wrong: u32,
    next: Option<Instant>,
}

/// The protection state a Vault keeps beside its storage.
pub(crate) struct Protection {
    pub(crate) kdf: Mutex<KdfParams>,
    pub(crate) attempts: Mutex<Attempts>,
    /// Edits sealed as their folder locked whose blobs couldn't be written,
    /// only ciphertext: written when it next unlocks, gone when the vault
    /// closes.
    held: Mutex<Vec<Sealed>>,
}

impl Default for Protection {
    fn default() -> Self {
        Protection {
            kdf: Mutex::new(KdfParams::RECOMMENDED),
            attempts: Mutex::default(),
            held: Mutex::default(),
        }
    }
}

/// How a lock went.
#[derive(Debug, PartialEq, Eq)]
pub struct LockOutcome {
    pub folder: VaultPath,
    /// Notes whose edits couldn't be written: they're kept, encrypted, and
    /// written when the folder next unlocks (while the vault stays open).
    pub unsaved: Vec<VaultPath>,
    /// Notes whose edits couldn't even be sealed: they're gone.
    pub lost: Vec<VaultPath>,
}

/// How hard a passphrase is to guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strength {
    Weak,
    Fair,
    Strong,
}

/// Rates a passphrase: four or more unrelated words beat a short password
/// with symbols, as the protect flow says.
pub fn passphrase_strength(passphrase: &str) -> Strength {
    let mut distinct: Vec<char> = passphrase.chars().collect();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() < 6 {
        return Strength::Weak;
    }
    let mut words: Vec<String> = passphrase
        .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect();
    words.sort();
    words.dedup();
    let classes = [
        passphrase.chars().any(|c| c.is_lowercase()),
        passphrase.chars().any(|c| c.is_uppercase()),
        passphrase.chars().any(|c| c.is_numeric()),
        passphrase
            .chars()
            .any(|c| !c.is_alphanumeric() && !c.is_whitespace()),
    ]
    .iter()
    .filter(|&&has| has)
    .count();
    let length = passphrase.chars().count();
    if words.len() >= 4 || (length >= 24 && distinct.len() >= 10) {
        Strength::Strong
    } else if words.len() >= 3 || (length >= 12 && classes >= 3) {
        Strength::Fair
    } else {
        Strength::Weak
    }
}

impl Vault {
    /// Sets the Argon2 cost of new protected folders. Only for tests, with
    /// orbit-crypto's insecure test parameters: anything weaker than the
    /// recommended cost is refused when a folder is protected.
    #[doc(hidden)]
    pub fn set_kdf_params(&self, params: KdfParams) {
        *lock(&self.protection.kdf) = params;
    }

    /// The keys for a new protected folder with `passphrase`. It runs the
    /// slow key derivation, so do it off the UI thread.
    pub fn prepare_protection(&self, passphrase: &str) -> Result<ProtectionKeys, Error> {
        let params = *lock(&self.protection.kdf);
        let (header, recovery, master) = orbit_crypto::create(
            &SecretString::from(passphrase),
            params,
            &mut orbit_crypto::os_rng(),
        )
        .map_err(|e| Error::Io(std::io::Error::other(format!("{e:?}"))))?;
        Ok(ProtectionKeys {
            header,
            recovery,
            master,
        })
    }

    /// Makes `folder` a protected folder, unlocked: its notes, attachments,
    /// file names and folders are encrypted, and the plain ones deleted
    /// outright. A protected folder never holds another, and the vault
    /// itself, notes and tool folders can't be protected here.
    pub fn protect_folder(&self, folder: &VaultPath, keys: ProtectionKeys) -> Result<(), Error> {
        self.can_protect(folder)?;
        // Search can't be reading the folder's notes into its cache while
        // it's protected; what it read before is cleared after.
        let writing = search::hold_writes(self);
        let protected = self.fs.protect(folder, &keys.header, keys.master);
        drop(writing);
        protected?;
        // What Orbit kept of the folder in plain text goes: the search
        // index (rebuilt without it) and the session.
        search::forget_everything(self);
        self.save_session(&self.session())?;
        self.touched([folder.clone()]);
        Ok(())
    }

    /// Whether `folder` can be protected: a folder (not the vault itself or
    /// a tool folder) that isn't in, and doesn't hold, a protected folder.
    /// Protected folders never unlocked since the vault opened are the
    /// exception: where they are, Orbit can't know.
    pub fn can_protect(&self, folder: &VaultPath) -> Result<(), Error> {
        if folder.is_root() || in_tool_folder(folder) {
            return Err(Error::InvalidName(folder.to_string()));
        }
        if let Some(near) = self.fs.protected_near(folder) {
            return Err(Error::Nested(near));
        }
        match self.entry_at(folder)? {
            Some(e) if e.entry_type == EntryType::Dir => Ok(()),
            Some(_) => Err(Error::InvalidName(folder.to_string())),
            None => Err(Error::NotFound),
        }
    }

    /// Every folder [`Self::can_protect`] allows, in tree order.
    pub fn protectable_folders(&self) -> Vec<VaultPath> {
        fn folders(nodes: &[crate::TreeNode], out: &mut Vec<VaultPath>) {
            for node in nodes.iter().filter(|n| n.kind == crate::EntryKind::Folder) {
                out.push(node.path.clone());
                folders(&node.children, out);
            }
        }
        let mut all = Vec::new();
        folders(&self.tree().unwrap_or_default(), &mut all);
        all.retain(|folder| self.can_protect(folder).is_ok());
        all
    }

    /// Unlocks every locked protected folder `passphrase` opens, without
    /// naming any (ADR 0003): Argon2 runs once per distinct salt, so do it
    /// off the UI thread. The folders unlocked.
    pub fn unlock(&self, passphrase: &str) -> Result<Vec<VaultPath>, UnlockError> {
        let passphrase = SecretString::from(passphrase);
        if self.fs.locked().unwrap_or_default().is_empty() {
            // As long as one folder would take: the time an attempt takes
            // mustn't say there's nothing to unlock.
            let params = *lock(&self.protection.kdf);
            orbit_crypto::create(&passphrase, params, &mut orbit_crypto::os_rng()).ok();
        }
        let mut keys: HashMap<Vec<u8>, Option<PassphraseKey>> = HashMap::new();
        self.unlock_with(|header| {
            let salt = orbit_crypto::passphrase_salt(header).ok()?;
            let key = keys
                .entry(salt)
                .or_insert_with(|| orbit_crypto::passphrase_key(header, &passphrase).ok());
            orbit_crypto::unlock_with_passphrase_key(header, key.as_ref()?).ok()
        })
    }

    /// Unlocks every locked protected folder the recovery key opens.
    pub fn unlock_with_recovery_key(
        &self,
        recovery_key: &str,
    ) -> Result<Vec<VaultPath>, UnlockError> {
        let recovery = RecoveryKey::from_str(recovery_key).ok();
        self.unlock_with(|header| {
            orbit_crypto::unlock_with_recovery_key(header, recovery.as_ref()?).ok()
        })
    }

    fn unlock_with(
        &self,
        mut open: impl FnMut(&Header) -> Option<MasterKey>,
    ) -> Result<Vec<VaultPath>, UnlockError> {
        let mut attempts = lock(&self.protection.attempts);
        if let Some(next) = attempts.next {
            let now = Instant::now();
            if now < next {
                return Err(UnlockError::TooSoon(next - now));
            }
        }
        // Held edits land while their folders are still locked; whichever
        // is newer, one of them or a save since, is the note once mounted.
        self.write_held();
        let mut unlocked = Vec::new();
        for (store, header) in self.fs.locked().unwrap_or_default() {
            if let Some(master) = open(&header)
                && let Ok(root) = self.fs.mount(store, master)
            {
                unlocked.push(root);
            }
        }
        if unlocked.is_empty() {
            let wait = FIRST_WAIT
                .saturating_mul(1 << attempts.wrong.min(16))
                .min(LONGEST_WAIT);
            attempts.wrong += 1;
            attempts.next = Some(Instant::now() + wait);
            return Err(UnlockError::Wrong { retry_in: wait });
        }
        *attempts = Attempts::default();
        drop(attempts);
        self.touched(unlocked.iter().cloned());
        Ok(unlocked)
    }

    /// Locks `folder`, saving the edits not yet saved in it (each a note's
    /// path and text). Locking always wins (ADR 0004): the edits are sealed
    /// under the folder's key, the folder locks, and only then are their
    /// blobs written, which needs no key. One that can't be written is held,
    /// as ciphertext, until the folder next unlocks.
    pub fn lock_saving(
        &self,
        folder: &VaultPath,
        edits: Vec<(VaultPath, orbit_crypto::Zeroizing<String>)>,
    ) -> LockOutcome {
        let mut sealed = Vec::new();
        let mut lost = Vec::new();
        for (path, text) in edits.into_iter().filter(|(path, _)| path.is_within(folder)) {
            match self.fs.seal_edit(&path, text.as_bytes()) {
                Ok(edit) => sealed.push((path, edit)),
                Err(_) => lost.push(path),
            }
        }
        self.lock(folder);
        let mut unsaved = Vec::new();
        for (path, edit) in sealed {
            if self.fs.write_sealed(&edit).is_err() {
                unsaved.push(path);
                lock(&self.protection.held).push(edit);
            }
        }
        LockOutcome {
            folder: folder.clone(),
            unsaved,
            lost,
        }
    }

    /// Writes the held edits whose folders are locked; those that still
    /// can't be written stay held.
    fn write_held(&self) {
        lock(&self.protection.held).retain(|edit| self.fs.write_sealed(edit).is_err());
    }

    /// Locks the unlocked protected folder at `folder`: its key is dropped,
    /// and it leaves the tree. Whether it was unlocked.
    pub fn lock(&self, folder: &VaultPath) -> bool {
        let locked = self.fs.unmount(folder);
        if locked {
            search::forget_folder(self, folder);
            self.touched([folder.clone()]);
            // What links and tags knew of the folder goes now, not when
            // they're next asked.
            self.index_links();
        }
        locked
    }

    /// Locks every unlocked protected folder; the ones locked.
    pub fn lock_all(&self) -> Vec<VaultPath> {
        let folders = self.fs.mounted();
        for folder in &folders {
            self.lock(folder);
        }
        folders
    }

    /// The unlocked protected folders.
    pub fn unlocked_folders(&self) -> Vec<VaultPath> {
        self.fs.mounted()
    }

    /// The unlocked protected folder `path` is in, or is.
    pub fn protected_folder_of(&self, path: &VaultPath) -> Option<VaultPath> {
        self.fs.protected_folder_of(path)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
