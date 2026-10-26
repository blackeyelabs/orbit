//! One encrypted blob per note or attachment: a plaintext prefix, then
//! STREAM-chunked XChaCha20-Poly1305. Chunk 0 holds only the logical path
//! and mtime, so unlocking can rebuild the tree without decrypting contents.

use aead_stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, aead::Payload};
use rand_core::CryptoRng;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{Error, FORMAT_VERSION, KEY_LEN, MasterKey, TAG_LEN, derive};

const BLOB_MAGIC: &[u8; 8] = b"ORBITBL\0";
const ID_LEN: usize = 16;
/// XChaCha20's 24-byte nonce minus STREAM's 5-byte counter and flag.
const NONCE_PREFIX_LEN: usize = 19;

// Byte offsets of the blob prefix (see docs/protected-folder-format.md).
const VERSION_AT: usize = BLOB_MAGIC.len();
const ID_AT: usize = VERSION_AT + 1;
const NONCE_PREFIX_AT: usize = ID_AT + ID_LEN;
const META_LEN_AT: usize = NONCE_PREFIX_AT + NONCE_PREFIX_LEN;
const PREFIX_LEN: usize = META_LEN_AT + 4;

/// Plaintext bytes per content chunk.
const CHUNK: usize = 64 * 1024;

/// A blob's logical path (vault-relative) and modification time.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Metadata {
    path: String,
    mtime_ms: i64,
}

/// Redacted: a decrypted path must never reach logs or panic messages.
impl std::fmt::Debug for Metadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metadata")
            .field("path", &"<redacted>")
            .field("mtime_ms", &self.mtime_ms)
            .finish()
    }
}

impl Metadata {
    pub fn new(path: impl Into<String>, mtime_ms: i64) -> Self {
        Metadata {
            path: path.into(),
            mtime_ms,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Milliseconds since the Unix epoch.
    pub fn mtime_ms(&self) -> i64 {
        self.mtime_ms
    }

    fn encode(&self) -> Result<Zeroizing<Vec<u8>>, Error> {
        let path = self.path.as_bytes();
        let len = u16::try_from(path.len()).map_err(|_| Error::TooLarge)?;
        let mut out = Zeroizing::new(Vec::with_capacity(2 + path.len() + 8));
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(path);
        out.extend_from_slice(&self.mtime_ms.to_le_bytes());
        Ok(out)
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let len_bytes = bytes.get(..2).ok_or(Error::Malformed)?;
        let len = u16::from_le_bytes(len_bytes.try_into().unwrap()) as usize;
        let path = bytes.get(2..2 + len).ok_or(Error::Malformed)?;
        let mtime = bytes
            .get(2 + len..)
            .filter(|m| m.len() == 8)
            .ok_or(Error::Malformed)?;
        // Validate in place, so a rejected path is never copied out unwiped.
        let path = std::str::from_utf8(path).map_err(|_| Error::Malformed)?;
        Ok(Metadata {
            path: path.to_string(),
            mtime_ms: i64::from_le_bytes(mtime.try_into().unwrap()),
        })
    }
}

/// Encrypts a note or attachment into a blob under the folder's master key.
pub fn seal(
    master: &MasterKey,
    metadata: &Metadata,
    contents: &[u8],
    rng: &mut impl CryptoRng,
) -> Result<Vec<u8>, Error> {
    let encoded = metadata.encode()?;
    let mut id = [0u8; ID_LEN];
    rng.fill_bytes(&mut id);
    let mut nonce_prefix = [0u8; NONCE_PREFIX_LEN];
    rng.fill_bytes(&mut nonce_prefix);

    let mut prefix = Vec::with_capacity(PREFIX_LEN);
    prefix.extend_from_slice(BLOB_MAGIC);
    prefix.push(FORMAT_VERSION);
    prefix.extend_from_slice(&id);
    prefix.extend_from_slice(&nonce_prefix);
    prefix.extend_from_slice(&((encoded.len() + TAG_LEN) as u32).to_le_bytes());

    let mut stream = EncryptorBE32::from_aead(aead_for(master, &id), &nonce_prefix.into());
    // STREAM only fails past ~2^32 chunks (256 TiB).
    let too_large = |_| Error::TooLarge;
    let mut out = prefix.clone();
    out.extend(
        stream
            .encrypt_next(Payload {
                msg: &encoded,
                aad: &prefix,
            })
            .map_err(too_large)?,
    );
    // At least one content chunk, possibly empty, carries the last flag;
    // `encrypt_last` consumes the stream, so nothing can follow it.
    let chunks: Vec<&[u8]> = contents.chunks(CHUNK).collect();
    let (last, middle) = chunks
        .split_last()
        .map_or((&[][..], &[][..]), |(l, m)| (*l, m));
    for chunk in middle {
        out.extend(
            stream
                .encrypt_next(Payload {
                    msg: chunk,
                    aad: &prefix,
                })
                .map_err(too_large)?,
        );
    }
    out.extend(
        stream
            .encrypt_last(Payload {
                msg: last,
                aad: &prefix,
            })
            .map_err(too_large)?,
    );
    Ok(out)
}

/// Decrypts only the metadata chunk: enough to list the folder.
pub fn open_metadata(master: &MasterKey, blob: &[u8]) -> Result<Metadata, Error> {
    split_blob(master, blob)?.metadata()
}

/// Decrypts the metadata and the full contents.
pub fn open(master: &MasterKey, blob: &[u8]) -> Result<(Metadata, Zeroizing<Vec<u8>>), Error> {
    let mut parts = split_blob(master, blob)?;
    let metadata = parts.metadata()?;
    let mut rest = parts.contents_ct;
    let mut contents = Zeroizing::new(Vec::with_capacity(rest.len()));
    while rest.len() > CHUNK + TAG_LEN {
        let (chunk, tail) = rest.split_at(CHUNK + TAG_LEN);
        contents.extend(parts.decrypt_next(chunk)?.iter());
        rest = tail;
    }
    contents.extend(parts.decrypt_last(rest)?.iter());
    Ok((metadata, contents))
}

/// The blob's id as lowercase hex: its file name under `blobs/`.
pub fn blob_id(blob: &[u8]) -> Result<String, Error> {
    check_prefix(blob)?;
    Ok(blob[ID_AT..ID_AT + ID_LEN]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn check_prefix(blob: &[u8]) -> Result<(), Error> {
    if blob.len() <= VERSION_AT || &blob[..BLOB_MAGIC.len()] != BLOB_MAGIC {
        return Err(Error::Malformed);
    }
    let version = blob[VERSION_AT];
    if version != FORMAT_VERSION {
        return Err(Error::UnsupportedVersion(version));
    }
    if blob.len() < PREFIX_LEN {
        return Err(Error::Malformed);
    }
    Ok(())
}

/// A blob split for decryption, with its STREAM decryptor ready.
struct SplitBlob<'a> {
    /// The plaintext prefix: every chunk's associated data.
    prefix: &'a [u8],
    stream: DecryptorBE32<XChaCha20Poly1305>,
    meta_ct: &'a [u8],
    contents_ct: &'a [u8],
}

impl SplitBlob<'_> {
    fn decrypt_next(&mut self, chunk: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
        self.stream
            .decrypt_next(Payload {
                msg: chunk,
                aad: self.prefix,
            })
            .map(Zeroizing::new)
            .map_err(|_| Error::WrongSecret)
    }

    /// Consumes the stream: nothing may follow the last chunk.
    fn decrypt_last(self, chunk: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
        self.stream
            .decrypt_last(Payload {
                msg: chunk,
                aad: self.prefix,
            })
            .map(Zeroizing::new)
            .map_err(|_| Error::WrongSecret)
    }

    fn metadata(&mut self) -> Result<Metadata, Error> {
        let meta_ct = self.meta_ct;
        Metadata::decode(&self.decrypt_next(meta_ct)?)
    }
}

/// Checks a blob's prefix and splits it for decryption.
fn split_blob<'a>(master: &MasterKey, blob: &'a [u8]) -> Result<SplitBlob<'a>, Error> {
    check_prefix(blob)?;
    let (prefix, body) = blob.split_at(PREFIX_LEN);
    let id: [u8; ID_LEN] = prefix[ID_AT..NONCE_PREFIX_AT].try_into().unwrap();
    let nonce_prefix: [u8; NONCE_PREFIX_LEN] =
        prefix[NONCE_PREFIX_AT..META_LEN_AT].try_into().unwrap();
    let meta_len = u32::from_le_bytes(prefix[META_LEN_AT..].try_into().unwrap()) as usize;
    // At least the metadata chunk and one (possibly empty) last chunk.
    if meta_len < TAG_LEN || body.len() < meta_len + TAG_LEN {
        return Err(Error::Malformed);
    }
    let (meta_ct, contents_ct) = body.split_at(meta_len);
    let stream = DecryptorBE32::from_aead(aead_for(master, &id), &nonce_prefix.into());
    Ok(SplitBlob {
        prefix,
        stream,
        meta_ct,
        contents_ct,
    })
}

/// Per-blob key: HKDF of the master key and the blob id.
fn aead_for(master: &MasterKey, id: &[u8; ID_LEN]) -> XChaCha20Poly1305 {
    let info = [b"orbit/pf/v1/blob".as_slice(), id].concat();
    let key: Zeroizing<[u8; KEY_LEN]> = derive(&master.0, &info);
    let key: &[u8; KEY_LEN] = &key;
    XChaCha20Poly1305::new(key.into())
}
