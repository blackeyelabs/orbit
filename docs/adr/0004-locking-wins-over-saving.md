# Locking always wins over saving

When a protected folder locks (manually, on idle, on suspend or on screen lock), Orbit saves pending edits first. If that encrypted save fails, the lock still happens: plaintext is dropped, and the unsaved edits are kept in memory only as ciphertext under the folder's key until a save succeeds or the window closes. A toast says this, and closing the window before a save succeeds loses those edits. A reader might expect "never lose edits" to win. Here it deliberately doesn't, because a lock that can be blocked by a full disk isn't a lock.
