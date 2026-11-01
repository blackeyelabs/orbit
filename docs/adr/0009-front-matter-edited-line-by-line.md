# Front matter is edited line by line, without a YAML library

The properties panel changes a note's front matter by rewriting only the lines of the key being edited, in `md-engine`. Comments, key order, quoting and keys it doesn't understand stay byte for byte as they were. A key it can't edit safely (nested maps, anchors, more than one document) shows "Edit in Source" instead of an editor.

A YAML library would be simpler, but parsing and re-serialising the block drops comments and can reorder or requote keys. That would change bytes the user never touched, against ADR 0001: the file, not Orbit's model of it, is the source of truth. A library may still be added later for reading only.
