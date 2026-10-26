# Search uses tantivy plus an in-memory link graph, not SQLite

Full-text search uses `tantivy`. Links, backlinks, tags and headings live in an in-memory graph, and the graph is snapshotted to a cache under `.vault/` for unprotected notes only. This keeps Orbit free of C libraries we choose ourselves, and it handles the target of about 10,000 notes comfortably. The cost is no SQL, and we write and maintain the link graph ourselves. Revisit if vaults approach 100,000 notes.

## Considered Options

- **SQLite with FTS5**: one store for search and links, with SQL queries, but it adds a C dependency and is harder to keep protected notes out of an on-disk database.
