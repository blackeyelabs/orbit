# The plain-text buffer is the source of truth

Live Preview is a decoration layer (style runs, conceal ranges, line styles, block widgets) over the note's plain-text buffer. It is never a separate document model. That makes markdown and front matter round-trip byte for byte, and Live Preview and Source mode are two views of the same text. The cost is a custom editor element that has to map caret positions across hidden ranges, which Spike 1 tests.

## Considered Options

- **Block-tree editor** (Notion-style document model serialized to markdown): easier widgets, but it rewrites formatting, loses comments and key order in front matter, and drifts from what other tools see in the file.
