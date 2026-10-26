//! Format v1 known-answer vectors (tests/vectors/v1.txt).
//!
//! `stored_v1_files_still_open` must pass forever: later builds have to read
//! v1 folders. `regenerating_v1_is_byte_identical` catches accidental format
//! changes; regenerate with `ORBIT_WRITE_VECTORS=1 cargo test -p orbit-crypto
//! --test vectors` only when a change is intended (and then bump the version).

use std::{collections::BTreeMap, fs, path::PathBuf};

use orbit_crypto::{
    Header, KdfParams, Metadata, RecoveryKey, create, open, seal, unlock, unlock_with_recovery_key,
};
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng};
use secrecy::SecretString;

const FAST: KdfParams = KdfParams::INSECURE_TEST;
const PASSPHRASE: &str = "correct horse battery staple";
const PATH: &str = "Journal/2026/Therapy notes.md";
const MTIME_MS: i64 = 1_793_000_000_123;
const CONTENTS: &str = "# Session\nFelt better today.\n";

fn vector_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/v1.txt")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn generate() -> BTreeMap<&'static str, String> {
    let mut rng = ChaCha20Rng::seed_from_u64(0x0_7b17);
    let (header, recovery, master) =
        create(&SecretString::from(PASSPHRASE), FAST, &mut rng).unwrap();
    let blob = seal(
        &master,
        &Metadata::new(PATH, MTIME_MS),
        CONTENTS.as_bytes(),
        &mut rng,
    )
    .unwrap();
    BTreeMap::from([
        ("passphrase", PASSPHRASE.to_string()),
        ("recovery_key", recovery.reveal().to_string()),
        ("header", hex(header.as_bytes())),
        ("blob", hex(&blob)),
        ("path", PATH.to_string()),
        ("mtime_ms", MTIME_MS.to_string()),
        ("contents_hex", hex(CONTENTS.as_bytes())),
    ])
}

fn load() -> BTreeMap<String, String> {
    let text = fs::read_to_string(vector_path()).expect("tests/vectors/v1.txt exists");
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let (k, v) = l.split_once('=').expect("key=value");
            (k.trim().to_string(), v.trim().to_string())
        })
        .collect()
}

#[test]
fn regenerating_v1_is_byte_identical() {
    let fresh = generate();
    if std::env::var_os("ORBIT_WRITE_VECTORS").is_some() {
        let mut out = String::from(
            "# Orbit protected-folder format v1 known-answer vector.\n\
             # Argon2id m=8 KiB t=1 p=1 (test-only cost). See docs/protected-folder-format.md.\n",
        );
        for (k, v) in &fresh {
            out.push_str(&format!("{k} = {v}\n"));
        }
        fs::write(vector_path(), out).unwrap();
    }
    let stored = load();
    for (k, v) in &fresh {
        assert_eq!(
            stored.get(*k),
            Some(v),
            "{k} differs from tests/vectors/v1.txt"
        );
    }
}

#[test]
fn stored_v1_files_still_open() {
    let v = load();
    let header = Header::from_bytes(unhex(&v["header"]));
    let blob = unhex(&v["blob"]);
    let by_passphrase = unlock(&header, &SecretString::from(v["passphrase"].as_str())).unwrap();
    let recovery: RecoveryKey = v["recovery_key"].parse().unwrap();
    let by_recovery = unlock_with_recovery_key(&header, &recovery).unwrap();
    assert!(by_passphrase == by_recovery);
    let (meta, contents) = open(&by_passphrase, &blob).unwrap();
    assert_eq!(meta.path(), v["path"]);
    assert_eq!(meta.mtime_ms().to_string(), v["mtime_ms"]);
    assert_eq!(contents.as_slice(), unhex(&v["contents_hex"]).as_slice());
}

/// 64 KiB + 10 bytes: one full content chunk and a short last one.
fn multichunk_contents() -> Vec<u8> {
    (0..65_546u32).map(|i| (i % 241) as u8).collect()
}

fn multichunk_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/v1-multichunk.blob")
}

fn generate_multichunk() -> Vec<u8> {
    let mut rng = ChaCha20Rng::seed_from_u64(0x0_7b18);
    let (_, _, master) = create(
        &SecretString::from(PASSPHRASE),
        FAST,
        &mut ChaCha20Rng::seed_from_u64(0x0_7b17),
    )
    .unwrap();
    seal(
        &master,
        &Metadata::new("Attachments/scan.pdf", MTIME_MS),
        &multichunk_contents(),
        &mut rng,
    )
    .unwrap()
}

#[test]
fn regenerating_the_multichunk_vector_is_byte_identical_and_it_opens() {
    let fresh = generate_multichunk();
    if std::env::var_os("ORBIT_WRITE_VECTORS").is_some() {
        fs::write(multichunk_path(), &fresh).unwrap();
    }
    let stored = fs::read(multichunk_path()).expect("tests/vectors/v1-multichunk.blob exists");
    assert!(
        stored == fresh,
        "v1-multichunk.blob differs from a fresh encryption"
    );
    let v = load();
    let master = unlock(
        &Header::from_bytes(unhex(&v["header"])),
        &SecretString::from(PASSPHRASE),
    )
    .unwrap();
    let (meta, contents) = open(&master, &stored).unwrap();
    assert_eq!(meta.path(), "Attachments/scan.pdf");
    assert_eq!(contents.as_slice(), multichunk_contents().as_slice());
}

/// Decrypts the stored vectors following docs/protected-folder-format.md
/// with raw primitives only (no orbit-crypto decryption code), so the spec
/// and the implementation are checked against each other.
mod spec_conformance {
    use super::*;
    use chacha20poly1305::{
        KeyInit, XChaCha20Poly1305, XNonce,
        aead::{Aead, Payload},
    };
    use hkdf::Hkdf;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    fn le32(b: &[u8]) -> u32 {
        u32::from_le_bytes(b.try_into().unwrap())
    }

    fn master_key_per_spec(header: &[u8], passphrase: &str) -> [u8; 32] {
        assert_eq!(header.len(), 229);
        assert_eq!(&header[0..8], b"ORBITPF\0");
        assert_eq!(header[8], 1);
        let (m, t, p) = (
            le32(&header[9..13]),
            le32(&header[13..17]),
            le32(&header[17..21]),
        );
        let salt = &header[21..37];
        let params = argon2::Params::new(m, t, p, Some(32)).unwrap();
        let mut kek = [0u8; 32];
        argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
            .hash_password_into(passphrase.as_bytes(), salt, &mut kek)
            .unwrap();
        let aad = [b"ORBITPF\0".as_slice(), &[1], b"passphrase", &header[9..37]].concat();
        let master = XChaCha20Poly1305::new(&kek.into())
            .decrypt(
                &XNonce::try_from(&header[37..61]).unwrap(),
                Payload {
                    msg: &header[61..109],
                    aad: &aad,
                },
            )
            .expect("passphrase slot opens per spec");
        let master: [u8; 32] = master.try_into().unwrap();

        let mut mac_key = [0u8; 32];
        Hkdf::<Sha256>::new(None, &master)
            .expand(b"orbit/pf/v1/header-mac", &mut mac_key)
            .unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(&mac_key).unwrap();
        mac.update(&header[..197]);
        mac.verify_slice(&header[197..229])
            .expect("header MAC per spec");
        master
    }

    /// Returns (metadata plaintext, contents plaintext).
    fn open_blob_per_spec(blob: &[u8], master: &[u8; 32]) -> (Vec<u8>, Vec<u8>) {
        assert_eq!(&blob[0..8], b"ORBITBL\0");
        assert_eq!(blob[8], 1);
        let (id, nonce_prefix) = (&blob[9..25], &blob[25..44]);
        let meta_len = le32(&blob[44..48]) as usize;
        let prefix = &blob[..48];
        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(None, master)
            .expand(&[b"orbit/pf/v1/blob".as_slice(), id].concat(), &mut key)
            .unwrap();
        let aead = XChaCha20Poly1305::new(&key.into());
        let chunk = |counter: u32, last: bool, ct: &[u8]| {
            let nonce = [nonce_prefix, &counter.to_be_bytes(), &[last as u8]].concat();
            aead.decrypt(
                &XNonce::try_from(nonce.as_slice()).unwrap(),
                Payload {
                    msg: ct,
                    aad: prefix,
                },
            )
            .expect("chunk opens per spec")
        };
        let meta = chunk(0, false, &blob[48..48 + meta_len]);
        let mut rest = &blob[48 + meta_len..];
        let (mut counter, mut contents) = (1u32, Vec::new());
        while rest.len() > 65_552 {
            contents.extend(chunk(counter, false, &rest[..65_552]));
            rest = &rest[65_552..];
            counter += 1;
        }
        contents.extend(chunk(counter, true, rest));
        (meta, contents)
    }

    fn expected_meta(path: &str) -> Vec<u8> {
        [
            &(path.len() as u16).to_le_bytes()[..],
            path.as_bytes(),
            &MTIME_MS.to_le_bytes(),
        ]
        .concat()
    }

    #[test]
    fn the_v1_vectors_decrypt_by_the_written_spec() {
        let v = load();
        let master = master_key_per_spec(&unhex(&v["header"]), PASSPHRASE);

        let (meta, contents) = open_blob_per_spec(&unhex(&v["blob"]), &master);
        assert_eq!(meta, expected_meta(PATH));
        assert_eq!(contents, CONTENTS.as_bytes());

        let multi = fs::read(multichunk_path()).unwrap();
        let (meta, contents) = open_blob_per_spec(&multi, &master);
        assert_eq!(meta, expected_meta("Attachments/scan.pdf"));
        assert_eq!(contents, multichunk_contents());
    }
}
