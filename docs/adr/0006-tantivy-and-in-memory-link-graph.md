# Search uses tantivy plus an in-memory link graph, not SQLite

Full-text search uses `tantivy`. Links, backlinks, tags and headings live in an in-memory graph, and the graph is snapshotted to a cache under `.vault/` for unprotected notes only. This keeps Orbit free of C libraries we choose ourselves, and it handles the target of about 10,000 notes comfortably. The cost is no SQL, and we write and maintain the link graph ourselves. Revisit if vaults approach 100,000 notes.

The search index is cached in a folder that `VaultFs::cache_dir` hands out, `.vault/search` for a plain vault, and tantivy reads and writes it there directly. It's the one place vault-side files bypass `VaultFs`: tantivy needs a real directory, and the cache holds nothing that isn't in the notes already. Storage that can't keep plaintext on disk returns no cache folder, and its notes are indexed in memory.

## Considered Options

- **SQLite with FTS5**: one store for search and links, with SQL queries, but it adds a C dependency and is harder to keep protected notes out of an on-disk database.
