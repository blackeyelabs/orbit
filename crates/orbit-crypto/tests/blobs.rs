use orbit_crypto::{Error, KdfParams, Metadata, create, open, open_metadata, seal};
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng};
use secrecy::SecretString;

const FAST: KdfParams = KdfParams::INSECURE_TEST;

fn folder(seed: u64) -> orbit_crypto::MasterKey {
    let mut rng = ChaCha20Rng::seed_from_u64(seed);
    create(&SecretString::from("p"), FAST, &mut rng).unwrap().2
}

fn meta() -> Metadata {
    Metadata::new("Journal/2026/Therapy notes.md", 1_793_000_000_123)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn a_sealed_note_opens_to_its_path_mtime_and_contents() {
    let key = folder(1);
    let blob = seal(
        &key,
        &meta(),
        b"# Session\nFelt better today.",
        &mut ChaCha20Rng::seed_from_u64(2),
    )
    .unwrap();
    let m = open_metadata(&key, &blob).unwrap();
    assert_eq!(m.path(), "Journal/2026/Therapy notes.md");
    assert_eq!(m.mtime_ms(), 1_793_000_000_123);
    let (m2, contents) = open(&key, &blob).unwrap();
    assert_eq!(m2, m);
    assert_eq!(contents.as_slice(), b"# Session\nFelt better today.");
}

#[test]
fn neither_the_contents_nor_the_path_appear_in_the_blob() {
    let key = folder(1);
    let blob = seal(
        &key,
        &meta(),
        b"Felt better today.",
        &mut ChaCha20Rng::seed_from_u64(2),
    )
    .unwrap();
    assert!(!contains(&blob, b"Felt better"));
    assert!(!contains(&blob, b"Therapy"));
    assert!(!contains(&blob, b"Journal"));
}

#[test]
fn empty_and_multi_chunk_contents_round_trip() {
    let key = folder(1);
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    for contents in [Vec::new(), vec![7u8; 65_536], big] {
        let blob = seal(&key, &meta(), &contents, &mut ChaCha20Rng::seed_from_u64(3)).unwrap();
        assert_eq!(
            open(&key, &blob).unwrap().1.as_slice(),
            contents.as_slice(),
            "len {}",
            contents.len()
        );
    }
}

#[test]
fn another_folders_key_cannot_open_a_blob() {
    let blob = seal(
        &folder(1),
        &meta(),
        b"x",
        &mut ChaCha20Rng::seed_from_u64(2),
    )
    .unwrap();
    assert_eq!(
        open_metadata(&folder(9), &blob).err(),
        Some(Error::WrongSecret)
    );
    assert_eq!(open(&folder(9), &blob).err(), Some(Error::WrongSecret));
}

fn sealed(contents: &[u8]) -> (orbit_crypto::MasterKey, Vec<u8>) {
    let key = folder(1);
    let blob = seal(&key, &meta(), contents, &mut ChaCha20Rng::seed_from_u64(4)).unwrap();
    (key, blob)
}

#[test]
fn any_flipped_bit_is_rejected() {
    let (key, blob) = sealed(b"short note");
    for bit in 0..blob.len() * 8 {
        let mut bad = blob.clone();
        bad[bit / 8] ^= 1 << (bit % 8);
        assert!(open(&key, &bad).is_err(), "accepted flipped bit {bit}");
    }
}

#[test]
fn every_truncation_is_rejected() {
    let (key, blob) = sealed(b"short note");
    for len in 0..blob.len() {
        assert!(
            open(&key, &blob[..len]).is_err(),
            "accepted truncation to {len}"
        );
    }
}

/// Offsets of the content chunks in a blob whose metadata is `meta()`.
fn content_start(blob: &[u8]) -> usize {
    let meta_len = u32::from_le_bytes(blob[44..48].try_into().unwrap()) as usize;
    48 + meta_len
}

const SEALED_CHUNK: usize = 64 * 1024 + 16;

#[test]
fn reordered_dropped_or_extra_chunks_are_rejected() {
    // Three full chunks plus a short last one.
    let contents: Vec<u8> = (0..(3 * 65_536 + 100) as u32)
        .map(|i| (i % 253) as u8)
        .collect();
    let (key, blob) = sealed(&contents);
    let start = content_start(&blob);
    let chunk = |i: usize| &blob[start + i * SEALED_CHUNK..start + (i + 1) * SEALED_CHUNK];

    let mut swapped = blob[..start].to_vec();
    swapped.extend_from_slice(chunk(1));
    swapped.extend_from_slice(chunk(0));
    swapped.extend_from_slice(&blob[start + 2 * SEALED_CHUNK..]);
    assert!(open(&key, &swapped).is_err(), "accepted swapped chunks");

    // Drop the short last chunk: the third full chunk now ends the blob.
    let dropped = &blob[..start + 3 * SEALED_CHUNK];
    assert!(
        open(&key, dropped).is_err(),
        "accepted a dropped last chunk"
    );

    let mut extra = blob.clone();
    extra.extend_from_slice(&[0u8; 17]);
    assert!(open(&key, &extra).is_err(), "accepted trailing bytes");
}

#[test]
fn a_blob_from_a_newer_format_version_is_reported_as_such() {
    let (key, mut blob) = sealed(b"x");
    blob[8] = 2;
    assert_eq!(open(&key, &blob).err(), Some(Error::UnsupportedVersion(2)));
}

#[test]
fn metadata_debug_output_hides_the_path() {
    let shown = format!("{:?}", meta());
    assert!(!shown.contains("Therapy"), "{shown}");
}

#[test]
fn any_flipped_bit_in_the_prefix_or_metadata_chunk_fails_open_metadata() {
    let (key, blob) = sealed(b"short note");
    for bit in 0..content_start(&blob) * 8 {
        let mut bad = blob.clone();
        bad[bit / 8] ^= 1 << (bit % 8);
        assert!(
            open_metadata(&key, &bad).is_err(),
            "accepted flipped bit {bit}"
        );
    }
}

#[test]
fn a_duplicated_chunk_appended_is_rejected() {
    let contents: Vec<u8> = (0..(2 * 65_536 + 10) as u32)
        .map(|i| (i % 249) as u8)
        .collect();
    let (key, blob) = sealed(&contents);
    let start = content_start(&blob);
    let mut extra = blob.clone();
    extra.extend_from_slice(&blob[start..start + SEALED_CHUNK]);
    assert!(open(&key, &extra).is_err());
}
