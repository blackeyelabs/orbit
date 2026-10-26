# Blind unlock with a passphrase per protected folder

Each protected folder has its own passphrase and recovery key. The unlock prompt names no folder: Orbit tries the entered passphrase against every locked folder's header and unlocks each one it opens. That way the prompt never reveals whether, or how many, protected folders exist. Each attempt runs one Argon2id derivation per distinct salt, so unlock time grows with the number of locked folders.

## Considered Options

- **One vault-wide protection passphrase**: a single fast unlock, but you can't keep separate secrets for separate folders.
- **Pick the folder, then type its passphrase**: fast, but the list of folders reveals they exist, which breaks the design's promise.
