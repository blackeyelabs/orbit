# Protected folder format, version 1

This is the on-disk format of a **protected folder** (see `CONTEXT.md`), as implemented in `crates/orbit-crypto`. ADR 0002 records why it's one blob per note with no manifest, and ADR 0003 records why there's one passphrase per folder with blind unlock. Version 1 is frozen by the known-answer vectors in `crates/orbit-crypto/tests/vectors/`: `v1.txt` (a header and a single-chunk blob) and `v1-multichunk.blob` (a full 64 KiB chunk plus a short last one). Every later build must still open them. A test also decrypts both by following this document with raw primitives, independent of the crate's own decryption code.

All integers are little-endian unless stated otherwise. Byte ranges are half-open.

## Layout on disk

```text
<vault>/.s/<folder-id>/       one directory per protected folder; <folder-id> is 32 random hex chars
    header                    the header below (229 bytes)
    blobs/<id>                one blob per note or attachment; <id> is the blob's 16-byte id as 32 lowercase hex chars
```

Someone with the disk can still see that a protected folder exists, how many blobs it holds and their approximate sizes. File names, folder structure and contents are encrypted.

## Primitives

| Use | Primitive | Crate |
|---|---|---|
| Passphrase → key-encryption key | Argon2id, version 0x13, 32-byte output | `argon2` 0.6 |
| Recovery key → key-encryption key; master key → subkeys | HKDF-SHA256 | `hkdf` 0.13, `sha2` 0.11 |
| Key wrapping | XChaCha20-Poly1305 (24-byte nonce, 16-byte tag) | `chacha20poly1305` 0.11 |
| Header integrity | HMAC-SHA256 | `hmac` 0.13 |
| Blob chunking | STREAM, big-endian 32-bit counter (`StreamBE32`) over XChaCha20-Poly1305 | `aead-stream` 0.6 |

Every random value (master key, recovery key, salts, nonces, blob ids) comes from a cryptographically secure RNG. The OS RNG is used in the app; tests inject a seeded one.

## Header (229 bytes)

| Offset | Length | Field |
|---|---|---|
| 0 | 8 | magic `ORBITPF\0` |
| 8 | 1 | format version, `0x01` |
| | | **passphrase slot** (100 bytes) |
| 9 | 4 | Argon2 memory, KiB (u32) |
| 13 | 4 | Argon2 iterations (u32) |
| 17 | 4 | Argon2 parallelism (u32) |
| 21 | 16 | Argon2 salt |
| 37 | 24 | XChaCha20 nonce |
| 61 | 48 | wrapped master key: 32-byte ciphertext and 16-byte tag |
| | | **recovery slot** (88 bytes) |
| 109 | 16 | HKDF salt |
| 125 | 24 | XChaCha20 nonce |
| 149 | 48 | wrapped master key: 32-byte ciphertext and 16-byte tag |
| 197 | 32 | header MAC |

### Keys

- **Master key:** 32 random bytes, generated once when the folder is protected. It never changes, so changing the passphrase never re-encrypts notes.
- **Passphrase key-encryption key:** `Argon2id(password = UTF-8 of the passphrase in Unicode NFC, salt = bytes 21..37, m, t, p, length 32)`. NFC makes the same passphrase typed through different keyboards or IMEs (composed or combining accents) derive the same key.
- **Recovery key-encryption key:** `HKDF-SHA256(salt = bytes 109..125, ikm = the 20-byte recovery key, info = "orbit/pf/v1/recovery", L = 32)`. The recovery key carries 160 bits of entropy, so it needs no slow KDF.
- **Header MAC key:** `HKDF-SHA256(salt = none, ikm = master key, info = "orbit/pf/v1/header-mac", L = 32)`.

### Slots

Each slot wraps the master key with XChaCha20-Poly1305 under its key-encryption key. The associated data binds the slot to the format, its own name and its own parameters:

- passphrase slot AAD: `"ORBITPF\0" ‖ 0x01 ‖ "passphrase" ‖ bytes 9..37` (the parameters and salt)
- recovery slot AAD: `"ORBITPF\0" ‖ 0x01 ‖ "recovery" ‖ bytes 109..125` (the salt)

Because each slot carries its own salt and parameters, either one can be replaced without the other's secret. **Change passphrase** writes a new passphrase slot, with a fresh salt and possibly new parameters. **New recovery key** writes a new recovery slot. Both require the unlocked master key, and both recompute the MAC.

### Header MAC

`HMAC-SHA256(header MAC key, bytes 0..197)`. Unlocking through either slot verifies it after unwrapping the master key. This detects tampering with the slot that wasn't used to unlock, which the slot's own tag can't see.

### Unlocking

1. Check the magic, then the version. An unknown version is reported as *unsupported version*, not as tampering. Then check the length is exactly 229.
2. Passphrase path only: check the Argon2 parameters against Orbit's ceiling before running Argon2. Memory must be 1 KiB to 512 MiB, iterations 1–10, parallelism 1–8. A header outside these limits is refused. Blind unlock tries every locked folder's header, so a planted header mustn't be able to make unlocking allocate a lot or spin for long.
3. Derive the key-encryption key, unwrap the master key, then verify the header MAC.

A wrong secret, a failed unwrap and a failed MAC are all reported as the same error, so a tampered header looks like a wrong passphrase. The unlock prompt names no folder (ADR 0003): it tries the entered passphrase against every locked folder's header.

**Creating a header or changing the passphrase** also enforces a floor: at least 64 MiB and 3 iterations (`KdfParams::RECOMMENDED`; the app should tune upwards towards about 1 s on target hardware). Unlocking still accepts weaker headers, since nobody can produce a valid header for someone else's folder without its master key. The only exception to the floor is `KdfParams::INSECURE_TEST` (8 KiB, 1 pass), which exists only with the `insecure-test-kdf` feature that the crate's own tests enable.

### Recovery key text

The 20-byte key is shown as 32 Crockford base32 characters (`0123456789ABCDEFGHJKMNPQRSTVWXYZ`), taking bits most-significant first, in 8 groups of 4 joined by `-`. For example: `X6AM-W6MT-34A7-94C6-5ZH3-NE3C-ZNVR-N0HK`. Parsing ignores case, dashes and whitespace, and reads `O` as `0` and `I`/`L` as `1`. It must yield exactly 32 symbols.

## Blob

| Offset | Length | Field |
|---|---|---|
| 0 | 8 | magic `ORBITBL\0` |
| 8 | 1 | format version, `0x01` |
| 9 | 16 | blob id (random; also the file name) |
| 25 | 19 | STREAM nonce prefix (random) |
| 44 | 4 | length of the metadata chunk ciphertext (u32) |
| 48 | … | chunks |

Bytes 0..48 are the **blob prefix**. They are the associated data of every chunk, so the version, id, nonce prefix and framing are all authenticated.

- **Blob key:** `HKDF-SHA256(salt = none, ikm = master key, info = "orbit/pf/v1/blob" ‖ id, L = 32)`.
- **STREAM nonce** for chunk *i*: `nonce prefix (19) ‖ i as u32 big-endian ‖ last-chunk flag (0x00 or 0x01)`. That's `aead-stream`'s `StreamBE32`.

Chunks, in order:

1. **Metadata chunk** (never last): `u16 path length ‖ UTF-8 vault-relative path ‖ i64 mtime in milliseconds since the Unix epoch`. Paths longer than 65,535 bytes are refused with *too large*. Its ciphertext length is stored at offset 44. Unlocking decrypts only this chunk to rebuild the folder's tree, without a manifest.
2. **Content chunks:** 64 KiB (65,536 bytes) of plaintext each, so 65,552 bytes of ciphertext. The final content chunk carries the last-chunk flag and may be shorter. It may be empty, which makes it exactly 16 bytes; every blob has at least one content chunk.

To read the content, start after the metadata chunk. While more than 65,552 bytes remain, decrypt the next 65,552 bytes as a non-last chunk; then decrypt whatever remains as the last chunk. STREAM's counter and last-chunk flag reject reordered, duplicated, dropped or extra chunks, and truncation.

## Test coverage

`crates/orbit-crypto/tests/` covers:
- round trips through both unlock paths;
- wrong passphrases and recovery keys;
- every single-bit flip of a header (parameter bytes through the recovery path, which binds them without running Argon2) and of a blob;
- every truncation of a blob;
- swapped, dropped and appended chunks;
- out-of-range Argon2 parameters;
- newer format versions;
- passphrase changes and recovery-key rotation;
- the v1 known-answer vector.

Argon2 runs at 8 KiB and 1 pass in tests only.

## Not covered by v1

- **Rollback and replay.** Changing the passphrase re-wraps the master key and never re-encrypts notes, so anyone who kept an old header can still unlock with the old passphrase. Old blobs can also be put back undetected, because there's no manifest to check them against (ADR 0002's trade-off). After a suspected passphrase compromise, the remedy is to remove protection and protect the folder again, which creates a new master key.
- **File name vs id.** The blob's file name should equal its embedded id (`blob_id`), but this crate doesn't check that; the filesystem layer should.
- Padding blob sizes to hide approximate lengths. It can be added in a later version.
- Memory locking and clearing plaintext from swap. Those belong to the app layer (Phase 1 leak checklist).
- An independent review of this design. That's planned before release (design doc, Phase 3).
