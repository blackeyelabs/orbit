use orbit_crypto::{
    Error, KdfParams, RecoveryKey, change_passphrase, create, new_recovery_key, unlock,
    unlock_with_recovery_key,
};
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng};
use secrecy::SecretString;

/// Argon2 settings far below production, so tests run fast.
const FAST: KdfParams = KdfParams::INSECURE_TEST;

fn rng() -> ChaCha20Rng {
    ChaCha20Rng::seed_from_u64(7)
}

fn pass(s: &str) -> SecretString {
    SecretString::from(s)
}

#[test]
fn the_passphrase_unlocks_the_folder_it_created() {
    let (header, _recovery, created) = create(&pass("correct horse"), FAST, &mut rng()).unwrap();
    let unlocked = unlock(&header, &pass("correct horse")).unwrap();
    assert!(unlocked == created);
}

#[test]
fn a_wrong_passphrase_is_rejected() {
    let (header, _, _) = create(&pass("correct horse"), FAST, &mut rng()).unwrap();
    assert!(matches!(
        unlock(&header, &pass("battery staple")),
        Err(Error::WrongSecret)
    ));
}

#[test]
fn the_recovery_key_reads_as_eight_groups_of_four() {
    let (_, recovery, _) = create(&pass("p"), FAST, &mut rng()).unwrap();
    let shown = recovery.reveal();
    let groups: Vec<&str> = shown.split('-').collect();
    assert_eq!(groups.len(), 8, "{}", *shown);
    for g in groups {
        assert_eq!(g.len(), 4);
        assert!(
            g.chars()
                .all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)),
            "{g}"
        );
    }
}

#[test]
fn the_recovery_key_unlocks_even_when_typed_loosely() {
    let (header, recovery, created) = create(&pass("p"), FAST, &mut rng()).unwrap();
    let shown = recovery.reveal();
    // Lower case, spaces instead of dashes, and O/I/L for 0/1/1.
    let typed = shown
        .replace('-', " ")
        .replace('0', "O")
        .replace('1', "l")
        .to_lowercase();
    let parsed: RecoveryKey = typed.parse().unwrap();
    assert!(unlock_with_recovery_key(&header, &parsed).unwrap() == created);
}

#[test]
fn a_different_recovery_key_is_rejected() {
    let (header, _, _) = create(&pass("p"), FAST, &mut rng()).unwrap();
    let (_, other, _) = create(&pass("p"), FAST, &mut ChaCha20Rng::seed_from_u64(99)).unwrap();
    assert!(matches!(
        unlock_with_recovery_key(&header, &other),
        Err(Error::WrongSecret)
    ));
}

#[test]
fn a_recovery_key_of_the_wrong_length_does_not_parse() {
    assert!("7QF2-KM9X-D4RT-8HWB".parse::<RecoveryKey>().is_err());
    assert!(
        "7QF2-KM9X-D4RT-8HWB-3NPA-VC6E-J5YZ-QG2U"
            .parse::<RecoveryKey>()
            .is_err(),
        "U is not Crockford"
    );
}

use orbit_crypto::Header;

/// Byte range of the Argon2 parameters in the header (after magic + version).
const PARAMS: std::ops::Range<usize> = 9..21;

fn flipped(header: &Header, bit: usize) -> Header {
    let mut bytes = header.as_bytes().to_vec();
    bytes[bit / 8] ^= 1 << (bit % 8);
    Header::from_bytes(bytes)
}

#[test]
fn any_flipped_header_bit_is_rejected() {
    let (header, recovery, _) = create(&pass("p"), FAST, &mut rng()).unwrap();
    for bit in 0..header.as_bytes().len() * 8 {
        let bad = flipped(&header, bit);
        assert!(
            unlock_with_recovery_key(&bad, &recovery).is_err(),
            "recovery path accepted bit {bit}"
        );
        // A flipped memory bit could ask Argon2 for gigabytes; the recovery
        // path above already proves the parameters are bound.
        if !PARAMS.contains(&(bit / 8)) {
            assert!(
                unlock(&bad, &pass("p")).is_err(),
                "passphrase path accepted bit {bit}"
            );
        }
    }
}

#[test]
fn absurd_kdf_parameters_are_refused_before_running_argon2() {
    let (header, _, _) = create(&pass("p"), FAST, &mut rng()).unwrap();
    let mut bytes = header.as_bytes().to_vec();
    bytes[PARAMS.start..PARAMS.start + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let started = std::time::Instant::now();
    assert_eq!(
        unlock(&Header::from_bytes(bytes), &pass("p")).err(),
        Some(Error::BadParams)
    );
    assert!(started.elapsed().as_secs() < 1);
}

#[test]
fn a_header_from_a_newer_format_version_is_reported_as_such() {
    let (header, _, _) = create(&pass("p"), FAST, &mut rng()).unwrap();
    let mut bytes = header.as_bytes().to_vec();
    bytes[8] = 2;
    assert_eq!(
        unlock(&Header::from_bytes(bytes), &pass("p")).err(),
        Some(Error::UnsupportedVersion(2))
    );
}

#[test]
fn kdf_parameters_beyond_orbits_limits_are_refused() {
    let (header, _, _) = create(&pass("p"), FAST, &mut rng()).unwrap();
    // (memory KiB, iterations, parallelism): 16 GiB, 1000 passes, 0 lanes.
    for (offset, value) in [(0, 16u32 << 20), (4, 1000), (8, 0)] {
        let mut bytes = header.as_bytes().to_vec();
        let at = PARAMS.start + offset;
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        assert_eq!(
            unlock(&Header::from_bytes(bytes), &pass("p")).err(),
            Some(Error::BadParams),
            "offset {offset}"
        );
    }
    assert!(matches!(
        create(
            &pass("p"),
            KdfParams {
                memory_kib: 16 << 20,
                iterations: 1,
                parallelism: 1
            },
            &mut rng()
        ),
        Err(Error::BadParams)
    ));
}

#[test]
fn changing_the_passphrase_keeps_the_master_key_and_the_recovery_key() {
    let (header, recovery, master) = create(&pass("old"), FAST, &mut rng()).unwrap();
    let header = change_passphrase(&header, &master, &pass("new"), FAST, &mut rng()).unwrap();
    assert!(unlock(&header, &pass("new")).unwrap() == master);
    assert_eq!(
        unlock(&header, &pass("old")).err(),
        Some(Error::WrongSecret)
    );
    assert!(unlock_with_recovery_key(&header, &recovery).unwrap() == master);
}

#[test]
fn a_new_recovery_key_replaces_the_old_one() {
    let (header, old_recovery, master) = create(&pass("p"), FAST, &mut rng()).unwrap();
    let (header, new_recovery) = new_recovery_key(&header, &master, &mut rng()).unwrap();
    assert!(unlock_with_recovery_key(&header, &new_recovery).unwrap() == master);
    assert_eq!(
        unlock_with_recovery_key(&header, &old_recovery).err(),
        Some(Error::WrongSecret)
    );
    assert!(unlock(&header, &pass("p")).unwrap() == master);
}

#[test]
fn changing_secrets_requires_the_folders_own_master_key() {
    let (header, _, _) = create(&pass("p"), FAST, &mut rng()).unwrap();
    let (_, _, stranger) = create(&pass("p"), FAST, &mut ChaCha20Rng::seed_from_u64(99)).unwrap();
    assert_eq!(
        change_passphrase(&header, &stranger, &pass("x"), FAST, &mut rng()).err(),
        Some(Error::WrongSecret)
    );
    assert_eq!(
        new_recovery_key(&header, &stranger, &mut rng()).err(),
        Some(Error::WrongSecret)
    );
}

#[test]
fn a_blob_sealed_before_a_passphrase_change_still_opens() {
    let (header, _, master) = create(&pass("old"), FAST, &mut rng()).unwrap();
    let meta = orbit_crypto::Metadata::new("Ideas.md", 1);
    let blob = orbit_crypto::seal(&master, &meta, b"unchanged", &mut rng()).unwrap();
    let header = change_passphrase(&header, &master, &pass("new"), FAST, &mut rng()).unwrap();
    let reopened = unlock(&header, &pass("new")).unwrap();
    assert_eq!(
        orbit_crypto::open(&reopened, &blob).unwrap().1.as_slice(),
        b"unchanged"
    );
}

#[test]
fn a_passphrase_unlocks_however_its_accents_were_typed() {
    // NFC-composed "café" vs "cafe" + combining acute accent.
    let (header, _, master) = create(&pass("caf\u{e9}"), FAST, &mut rng()).unwrap();
    assert!(unlock(&header, &pass("cafe\u{301}")).unwrap() == master);
}

#[test]
fn weak_kdf_parameters_are_refused_when_creating_or_changing() {
    let weak = KdfParams {
        memory_kib: 1024,
        iterations: 1,
        parallelism: 1,
    };
    assert_eq!(
        create(&pass("p"), weak, &mut rng()).err(),
        Some(Error::BadParams)
    );
    let (header, _, master) = create(&pass("p"), FAST, &mut rng()).unwrap();
    assert_eq!(
        change_passphrase(&header, &master, &pass("q"), weak, &mut rng()).err(),
        Some(Error::BadParams)
    );
}
