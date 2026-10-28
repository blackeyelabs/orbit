# Logs carry no vault content

Orbit logs through `tracing` to stderr and to a daily file under `$XDG_STATE_HOME/orbit/logs`, kept a week, so a bug or crash can be diagnosed after the fact. Those files sit outside every protected folder's encryption, and they're the kind of file people attach to bug reports. So logs never hold note or attachment text, passphrases, recovery keys, clipboard text, search queries or link targets. They also never hold a path inside, or naming, a protected folder, unlocked or locked.

How it's held to:
- Paths reach logs only through `Vault::log_path`, which shows `<protected>` for a protected folder's paths. Plain-note paths are logged only at debug level.
- Errors that might name a path are logged by `Error::kind_name`, not by their message.
- Protection events carry counts and outcomes, never folder names.
- orbit-crypto doesn't log at all.
- A test captures every event from every crate at trace level, through a full protect, search, edit, lock and unlock cycle, and fails if anything only the folder knows appears.

A reader might expect a log to name the note it's about. Here it deliberately doesn't for protected notes, even at debug level, because a log that names them would undo what protection hides: file names and folder structure.

The file is written as events happen, not from a background queue. A crash then leaves its cause in the file, at the cost of a small write on whichever thread logs.
