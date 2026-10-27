//! Key derivation, key wrapping and the protected-folder blob format.
//!
//! A protected folder has one header and one encrypted blob per note or
//! attachment (ADR 0002). The header wraps a random master key twice: under a
//! key derived from the passphrase with Argon2id, and under a key derived from
//! the recovery key. Byte layouts are specified in
//! `docs/protected-folder-format.md`.

mod blob;

pub use blob::{Metadata, blob_id, open, open_metadata, seal};
/// For passing passphrases in, and plaintext out, without copies lingering.
pub use secrecy::SecretString;
pub use zeroize::Zeroizing;

use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::CryptoRng;
use secrecy::ExposeSecret;
use sha2::Sha256;
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, ZeroizeOnDrop};

const HEADER_MAGIC: &[u8; 8] = b"ORBITPF\0";
const FORMAT_VERSION: u8 = 1;
const KEY_LEN: usize = 32;
const SALT_LEN: usize = 16;
const XNONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const RECOVERY_KEY_LEN: usize = 20;
const MAC_LEN: usize = 32;
const PARAMS_LEN: usize = 12;
const PREFIX_LEN: usize = HEADER_MAGIC.len() + 1;
/// The master key wrapped: nonce, then ciphertext with its tag.
const WRAPPED_LEN: usize = XNONCE_LEN + KEY_LEN + TAG_LEN;
/// Passphrase slot: Argon2id params, salt, wrapped key.
const PASSPHRASE_SLOT_LEN: usize = PARAMS_LEN + SALT_LEN + WRAPPED_LEN;
/// Recovery slot: HKDF salt, wrapped key.
const RECOVERY_SLOT_LEN: usize = SALT_LEN + WRAPPED_LEN;
const HEADER_LEN: usize = PREFIX_LEN + PASSPHRASE_SLOT_LEN + RECOVERY_SLOT_LEN + MAC_LEN;

const SLOT_PASSPHRASE: &[u8] = b"passphrase";
const SLOT_RECOVERY: &[u8] = b"recovery";

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The bytes are not a protected-folder header or blob of a known layout.
    Malformed,
    /// A newer format version than this build understands.
    UnsupportedVersion(u8),
    /// Wrong passphrase, recovery key or master key, or the data was tampered with.
    WrongSecret,
    /// The KDF parameters are out of range.
    BadParams,
    /// A path longer than 65,535 bytes, or contents beyond STREAM's chunk limit.
    TooLarge,
}

/// Argon2id cost parameters, stored in the header's passphrase slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes: the design's floor; tune towards ~1 s on target hardware.
    pub const RECOMMENDED: KdfParams = KdfParams {
        memory_kib: 64 * 1024,
        iterations: 3,
        parallelism: 1,
    };

    /// Argon2 at 8 KiB and one pass, for tests only. Behind the
    /// `insecure-test-kdf` feature, this exact value bypasses the floor below.
    #[cfg(feature = "insecure-test-kdf")]
    pub const INSECURE_TEST: KdfParams = KdfParams {
        memory_kib: 8,
        iterations: 1,
        parallelism: 1,
    };

    /// Orbit's ceiling (8x the recommended memory). Parameters come from a
    /// header on disk, and blind unlock tries every locked folder's header,
    /// so a planted header must not be able to allocate or spin for long.
    const MAX_MEMORY_KIB: u32 = 512 * 1024;
    const MAX_ITERATIONS: u32 = 10;
    const MAX_PARALLELISM: u32 = 8;

    /// New headers must be at least as strong as [`KdfParams::RECOMMENDED`].
    /// (Unlocking still accepts weaker headers: an attacker can't produce a
    /// valid header for someone else's folder without its master key.)
    fn check_for_new_header(self) -> Result<Self, Error> {
        #[cfg(feature = "insecure-test-kdf")]
        if self == Self::INSECURE_TEST {
            return Ok(self);
        }
        let floor = Self::RECOMMENDED;
        if self.memory_kib < floor.memory_kib || self.iterations < floor.iterations {
            return Err(Error::BadParams);
        }
        self.check()
    }

    fn check(self) -> Result<Self, Error> {
        let ok = (1..=Self::MAX_MEMORY_KIB).contains(&self.memory_kib)
            && (1..=Self::MAX_ITERATIONS).contains(&self.iterations)
            && (1..=Self::MAX_PARALLELISM).contains(&self.parallelism);
        if ok { Ok(self) } else { Err(Error::BadParams) }
    }

    fn to_bytes(self) -> [u8; PARAMS_LEN] {
        let mut b = [0u8; PARAMS_LEN];
        b[0..4].copy_from_slice(&self.memory_kib.to_le_bytes());
        b[4..8].copy_from_slice(&self.iterations.to_le_bytes());
        b[8..12].copy_from_slice(&self.parallelism.to_le_bytes());
        b
    }

    fn from_bytes(b: &[u8]) -> Self {
        let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        KdfParams {
            memory_kib: u32_at(0),
            iterations: u32_at(4),
            parallelism: u32_at(8),
        }
    }
}

/// The folder's master key. Zeroized on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; KEY_LEN]);

impl PartialEq for MasterKey {
    fn eq(&self, other: &Self) -> bool {
        // No early exit, so the compare doesn't leak where keys differ.
        self.0
            .iter()
            .zip(other.0.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }
}

/// The one-time recovery key, 160 bits, shown to the person once.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RecoveryKey([u8; RECOVERY_KEY_LEN]);

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

impl RecoveryKey {
    fn random(rng: &mut impl CryptoRng) -> Self {
        let mut key = RecoveryKey([0; RECOVERY_KEY_LEN]);
        rng.fill_bytes(&mut key.0);
        key
    }

    /// The key as the person sees it: 8 groups of 4 Crockford base32
    /// characters (160 bits, no padding).
    pub fn reveal(&self) -> Zeroizing<String> {
        let mut chars = Zeroizing::new(String::with_capacity(39));
        let mut acc: u32 = 0;
        let mut bits = 0;
        let mut n = 0;
        for &byte in &self.0 {
            acc = (acc << 8) | u32::from(byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                if n > 0 && n % 4 == 0 {
                    chars.push('-');
                }
                chars.push(CROCKFORD[((acc >> bits) & 31) as usize] as char);
                n += 1;
            }
        }
        acc.zeroize();
        chars
    }
}

impl std::str::FromStr for RecoveryKey {
    type Err = Error;

    /// Accepts any case, dashes or spaces, and O/I/L for 0/1/1 (Crockford).
    fn from_str(typed: &str) -> Result<Self, Error> {
        let mut key = RecoveryKey([0; RECOVERY_KEY_LEN]);
        // The bit accumulator holds key material; wiped on every return path.
        let mut acc = Zeroizing::new(0u32);
        let (mut bits, mut out, mut count) = (0, 0, 0);
        for c in typed.chars().filter(|c| *c != '-' && !c.is_whitespace()) {
            let c = match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            let value = CROCKFORD
                .iter()
                .position(|&a| a as char == c)
                .ok_or(Error::Malformed)?;
            count += 1;
            if count > 32 {
                return Err(Error::Malformed);
            }
            *acc = (*acc << 5) | value as u32;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                key.0[out] = (*acc >> bits) as u8;
                out += 1;
            }
        }
        if count != 32 {
            return Err(Error::Malformed);
        }
        Ok(key)
    }
}

/// A protected folder's header, as stored on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header(Vec<u8>);

impl Header {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Header(bytes)
    }
}

struct ParsedHeader<'a> {
    passphrase_slot: &'a [u8],
    recovery_slot: &'a [u8],
    mac: &'a [u8],
}

fn parse_header(bytes: &[u8]) -> Result<ParsedHeader<'_>, Error> {
    if bytes.len() < PREFIX_LEN || &bytes[..HEADER_MAGIC.len()] != HEADER_MAGIC {
        return Err(Error::Malformed);
    }
    let version = bytes[HEADER_MAGIC.len()];
    if version != FORMAT_VERSION {
        return Err(Error::UnsupportedVersion(version));
    }
    if bytes.len() != HEADER_LEN {
        return Err(Error::Malformed);
    }
    let (passphrase_slot, rest) = bytes[PREFIX_LEN..].split_at(PASSPHRASE_SLOT_LEN);
    let (recovery_slot, mac) = rest.split_at(RECOVERY_SLOT_LEN);
    Ok(ParsedHeader {
        passphrase_slot,
        recovery_slot,
        mac,
    })
}

/// Creates a protected folder: a fresh master key, its header and a
/// recovery key. The caller writes the header and shows the recovery key once.
pub fn create(
    passphrase: &SecretString,
    params: KdfParams,
    rng: &mut impl CryptoRng,
) -> Result<(Header, RecoveryKey, MasterKey), Error> {
    let mut master = MasterKey([0; KEY_LEN]);
    rng.fill_bytes(&mut master.0);
    let recovery = RecoveryKey::random(rng);
    let passphrase_slot = passphrase_slot(passphrase, params, &master, rng)?;
    let recovery_slot = recovery_slot(&recovery, &master, rng);
    let header = assemble_header(&passphrase_slot, &recovery_slot, &master);
    Ok((header, recovery, master))
}

/// Unlocks with the passphrase. Tampering with any part of the header is
/// reported as [`Error::WrongSecret`], like a wrong passphrase.
pub fn unlock(header: &Header, passphrase: &SecretString) -> Result<MasterKey, Error> {
    unlock_with_passphrase_key(header, &passphrase_key(header, passphrase)?)
}

/// What a passphrase derives for a header's passphrase slot: Argon2's
/// output, for its salt and parameters. Any header with the same salt and
/// parameters takes it, so trying one passphrase against several folders
/// runs Argon2 once per distinct salt. Zeroized on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PassphraseKey {
    /// The slot's parameters and salt, which the key is for.
    salt: [u8; PARAMS_LEN + SALT_LEN],
    kek: [u8; KEY_LEN],
}

impl PassphraseKey {
    /// The parameters and salt the key was derived for: equal for headers
    /// it opens alike.
    pub fn salt(&self) -> &[u8] {
        &self.salt
    }
}

/// The parameters and salt of a header's passphrase slot: two headers with
/// the same take the same [`PassphraseKey`].
pub fn passphrase_salt(header: &Header) -> Result<Vec<u8>, Error> {
    let h = parse_header(header.as_bytes())?;
    Ok(h.passphrase_slot[..PARAMS_LEN + SALT_LEN].to_vec())
}

/// Runs Argon2 for `passphrase` with the header's salt and parameters
/// (refusing parameters past Orbit's ceiling first).
pub fn passphrase_key(header: &Header, passphrase: &SecretString) -> Result<PassphraseKey, Error> {
    let h = parse_header(header.as_bytes())?;
    let (params, rest) = h.passphrase_slot.split_at(PARAMS_LEN);
    let salt = &rest[..SALT_LEN];
    let kek = passphrase_kek(passphrase, KdfParams::from_bytes(params), salt)?;
    let mut key = PassphraseKey {
        salt: [0; PARAMS_LEN + SALT_LEN],
        kek: *kek,
    };
    key.salt
        .copy_from_slice(&h.passphrase_slot[..PARAMS_LEN + SALT_LEN]);
    Ok(key)
}

/// Unlocks with a key derived for this header's salt and parameters. A key
/// for others, like a wrong passphrase's, is [`Error::WrongSecret`].
pub fn unlock_with_passphrase_key(
    header: &Header,
    key: &PassphraseKey,
) -> Result<MasterKey, Error> {
    let h = parse_header(header.as_bytes())?;
    let fields = &h.passphrase_slot[..PARAMS_LEN + SALT_LEN];
    if fields != key.salt {
        return Err(Error::WrongSecret);
    }
    let wrapped = &h.passphrase_slot[PARAMS_LEN + SALT_LEN..];
    let master = unwrap_key(&key.kek, wrapped, &slot_aad(SLOT_PASSPHRASE, fields))?;
    verify_header_mac(header.as_bytes(), h.mac, &master)?;
    Ok(master)
}

/// The operating system's random number generator, for new folders, keys
/// and blobs. Panics if the system can't provide randomness.
pub fn os_rng() -> impl CryptoRng {
    rand_core::UnwrapErr(getrandom::SysRng)
}

/// Unlocks with the recovery key instead of the passphrase.
pub fn unlock_with_recovery_key(
    header: &Header,
    recovery: &RecoveryKey,
) -> Result<MasterKey, Error> {
    let h = parse_header(header.as_bytes())?;
    let (salt, wrapped) = h.recovery_slot.split_at(SALT_LEN);
    let kek = recovery_kek(recovery, salt);
    let master = unwrap_key(&kek, wrapped, &slot_aad(SLOT_RECOVERY, salt))?;
    verify_header_mac(header.as_bytes(), h.mac, &master)?;
    Ok(master)
}

/// Replaces the passphrase (and its Argon2 parameters) by re-wrapping the
/// master key. Notes are not re-encrypted; the recovery key keeps working.
pub fn change_passphrase(
    header: &Header,
    master: &MasterKey,
    new_passphrase: &SecretString,
    params: KdfParams,
    rng: &mut impl CryptoRng,
) -> Result<Header, Error> {
    let h = verified_header(header, master)?;
    let passphrase_slot = passphrase_slot(new_passphrase, params, master, rng)?;
    Ok(assemble_header(&passphrase_slot, h.recovery_slot, master))
}

/// Issues a new recovery key; the old one stops working.
pub fn new_recovery_key(
    header: &Header,
    master: &MasterKey,
    rng: &mut impl CryptoRng,
) -> Result<(Header, RecoveryKey), Error> {
    let h = verified_header(header, master)?;
    let recovery = RecoveryKey::random(rng);
    let recovery_slot = recovery_slot(&recovery, master, rng);
    Ok((
        assemble_header(h.passphrase_slot, &recovery_slot, master),
        recovery,
    ))
}

/// Parses a header and proves `master` belongs to it, before rewriting a slot.
fn verified_header<'a>(header: &'a Header, master: &MasterKey) -> Result<ParsedHeader<'a>, Error> {
    let h = parse_header(header.as_bytes())?;
    verify_header_mac(header.as_bytes(), h.mac, master)?;
    Ok(h)
}

fn passphrase_slot(
    passphrase: &SecretString,
    params: KdfParams,
    master: &MasterKey,
    rng: &mut impl CryptoRng,
) -> Result<Vec<u8>, Error> {
    let params = params.check_for_new_header()?;
    let mut salt = [0u8; SALT_LEN];
    rng.fill_bytes(&mut salt);
    let kek = passphrase_kek(passphrase, params, &salt)?;
    let mut slot = params.to_bytes().to_vec();
    slot.extend_from_slice(&salt);
    let aad = slot_aad(SLOT_PASSPHRASE, &slot);
    slot.extend_from_slice(&wrap_key(&kek, master, &aad, rng));
    Ok(slot)
}

fn recovery_slot(recovery: &RecoveryKey, master: &MasterKey, rng: &mut impl CryptoRng) -> Vec<u8> {
    let mut salt = [0u8; SALT_LEN];
    rng.fill_bytes(&mut salt);
    let kek = recovery_kek(recovery, &salt);
    let mut slot = salt.to_vec();
    slot.extend_from_slice(&wrap_key(
        &kek,
        master,
        &slot_aad(SLOT_RECOVERY, &salt),
        rng,
    ));
    slot
}

/// Each slot is bound to the format version, its name and its own
/// parameters, so either slot can be replaced without the other's secret.
fn slot_aad(name: &[u8], fields: &[u8]) -> Vec<u8> {
    [HEADER_MAGIC.as_slice(), &[FORMAT_VERSION], name, fields].concat()
}

fn assemble_header(passphrase_slot: &[u8], recovery_slot: &[u8], master: &MasterKey) -> Header {
    let mut bytes = Vec::with_capacity(HEADER_LEN);
    bytes.extend_from_slice(HEADER_MAGIC);
    bytes.push(FORMAT_VERSION);
    bytes.extend_from_slice(passphrase_slot);
    bytes.extend_from_slice(recovery_slot);
    let mac = header_mac(&bytes, master).finalize().into_bytes();
    bytes.extend_from_slice(&mac);
    Header(bytes)
}

/// HMAC over everything before the MAC, keyed from the master key, so a
/// change to either slot (even the one not used to unlock) is detected.
fn header_mac(covered: &[u8], master: &MasterKey) -> Hmac<Sha256> {
    let key = derive(&master.0, b"orbit/pf/v1/header-mac");
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_ref()).expect("any key length");
    mac.update(covered);
    mac
}

fn verify_header_mac(bytes: &[u8], mac: &[u8], master: &MasterKey) -> Result<(), Error> {
    header_mac(&bytes[..bytes.len() - MAC_LEN], master)
        .verify_slice(mac)
        .map_err(|_| Error::WrongSecret)
}

fn passphrase_kek(
    passphrase: &SecretString,
    params: KdfParams,
    salt: &[u8],
) -> Result<Zeroizing<[u8; KEY_LEN]>, Error> {
    let params = params.check()?;
    let argon_params = argon2::Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(KEY_LEN),
    )
    .map_err(|_| Error::BadParams)?;
    let argon = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon_params,
    );
    // NFC, so the same passphrase typed through different keyboards or IMEs
    // (composed vs combining accents) derives the same key.
    let normalized: Zeroizing<String> = Zeroizing::new(passphrase.expose_secret().nfc().collect());
    let mut kek = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(normalized.as_bytes(), salt, kek.as_mut())
        .map_err(|_| Error::BadParams)?;
    Ok(kek)
}

/// The recovery key has 160 bits of entropy, so HKDF suffices (no Argon2).
fn recovery_kek(recovery: &RecoveryKey, salt: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
    let hk = Hkdf::<Sha256>::new(Some(salt), &recovery.0);
    let mut kek = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(b"orbit/pf/v1/recovery", kek.as_mut())
        .expect("32 bytes is a valid length");
    kek
}

fn derive(ikm: &[u8], info: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
    let hk = Hkdf::<Sha256>::new(None, ikm);
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(info, out.as_mut())
        .expect("32 bytes is a valid length");
    out
}

fn wrap_key(
    kek: &[u8; KEY_LEN],
    master: &MasterKey,
    aad: &[u8],
    rng: &mut impl CryptoRng,
) -> Vec<u8> {
    let mut nonce = [0u8; XNONCE_LEN];
    rng.fill_bytes(&mut nonce);
    let ct = XChaCha20Poly1305::new(kek.into())
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &master.0,
                aad,
            },
        )
        .expect("encrypting 32 bytes cannot fail");
    [nonce.as_slice(), &ct].concat()
}

fn unwrap_key(kek: &[u8; KEY_LEN], wrapped: &[u8], aad: &[u8]) -> Result<MasterKey, Error> {
    let (nonce, ct) = wrapped.split_at(XNONCE_LEN);
    let nonce = XNonce::try_from(nonce).map_err(|_| Error::Malformed)?;
    let pt = Zeroizing::new(
        XChaCha20Poly1305::new(kek.into())
            .decrypt(&nonce, Payload { msg: ct, aad })
            .map_err(|_| Error::WrongSecret)?,
    );
    let mut master = MasterKey([0; KEY_LEN]);
    master.0.copy_from_slice(&pt);
    Ok(master)
}
