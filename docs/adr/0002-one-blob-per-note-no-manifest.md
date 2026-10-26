# Protected folders store one encrypted blob per note, with no manifest

Each note or attachment in a protected folder is its own encrypted blob under `.s/<random>/blobs/`. The blob's first encrypted chunk holds its logical path and modification time. Unlocking rebuilds the folder tree by decrypting only those small chunks, so there is no central manifest that can be corrupted or produce conflicts when git or Dropbox syncs the folder.

## Consequences

Someone with access to the disk can see that a protected folder exists, how many blobs it holds and roughly how large they are. Orbit says so plainly in the protect flow and in Settings › Security.

## Considered Options

- **One container file per protected folder**: hides counts and sizes, but every edit rewrites one large file, and a sync conflict in it can lose the whole folder.
