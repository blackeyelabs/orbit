# Orbit

Orbit is a local-first, Obsidian-like workspace for markdown notes and text-drawn diagrams, kept as plain files in a folder. Some folders can be protected with real encryption.

## Language

### Workspace

**Orbit**:
The desktop app itself.
_Avoid_: Vault (as the product name), the editor

**Vault**:
A root folder of notes that Orbit opens. A vault is open in at most one window at a time.
_Avoid_: workspace, library, project

**Note**:
A `.md` or `.txt` file inside a vault. A `.txt` note is always shown as Source mode.
_Avoid_: page, document, file (when a note is meant)

**Attachment**:
Any file in a vault that is not a note or a canvas, such as an image or PDF a note links to or embeds.
_Avoid_: asset, media, resource

**Canvas**:
A `.canvas` file (JSON Canvas). It is listed and searchable, but not editable yet.
_Avoid_: board, whiteboard

### Links and tags

**Link**:
A reference from a note to a note, heading or attachment, written as a wikilink (`[[…]]`) or a markdown link.
_Avoid_: reference, relation

**Embed**:
A link written `![[…]]` that shows the target inside the linking note.
_Avoid_: transclusion, include

**Unresolved link**:
A link whose target does not exist. Following it creates the note.
_Avoid_: broken link, dead link

**Backlink**:
A link seen from its target's side: the notes that link to the note you are viewing.
_Avoid_: inbound link, reverse link

**Tag**:
A label from front matter `tags:` or an inline `#tag`. `/` nests tags, and matching ignores case.
_Avoid_: label, category, keyword

### Editing

**Live Preview**:
The default editor mode. Markdown renders in place, and its syntax shows only where the caret or selection touches it.
_Avoid_: WYSIWYG mode, rich mode

**Source mode**:
The editor mode that shows every markdown mark exactly as stored.
_Avoid_: raw mode, plain mode, code view

**Split view**:
A temporary source-plus-preview layout that exists only until Live Preview ships.
_Avoid_: preview pane, reading view

### Navigation

**Switcher**:
The overlay you type into to jump to a note or run a command. It has two modes, Quick switcher and Command palette.
_Avoid_: launcher, omnibox

**Quick switcher**:
The Switcher mode for finding and opening notes by name.
_Avoid_: file finder, go to file

**Command palette**:
The Switcher mode, entered with `>`, for running commands by name.
_Avoid_: action menu

**Vault search**:
Full-text search across the vault's notes, shown as a panel with filters.
_Avoid_: find, global search

### Protection

**Protected folder**:
A folder in a vault whose contents, file names and structure are encrypted at rest. It is always either locked or unlocked, and never contains another protected folder.
_Avoid_: sealed folder, area, private folder, encrypted folder

**Protected vault**:
A vault protected as a whole, with the vault root as its protected folder.
_Avoid_: encrypted vault, sealed vault

**Locked**:
The state of a protected folder whose contents Orbit cannot read. It does not appear in the tree, search, links or backlinks.
_Avoid_: closed, hidden

**Unlocked**:
The state of a protected folder whose key Orbit holds in memory, so its notes behave like any others until it locks.
_Avoid_: open, decrypted, mounted

**Passphrase**:
The secret a person types to unlock a protected folder. Each protected folder has its own.
_Avoid_: password, PIN

**Recovery key**:
A one-time code, shown when a folder is protected, that unlocks it if its passphrase is forgotten.
_Avoid_: backup code, reset key
