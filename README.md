<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/OrbitDarkNobg.png">
    <source media="(prefers-color-scheme: light)" srcset="assets/logo/OrbitLightNobg.png">
    <img alt="Orbit Logo" src="assets/logo/OrbitLightNobg.png" width="180">
  </picture>
</p>

<h1 align="center">Orbit</h1>

<p align="center">
  <strong>A local-first markdown knowledge workspace, written in Rust.</strong>
</p>

<p align="center">
  <a href="https://github.com/blackeyelabs/orbit/actions/workflows/ci.yml"><img src="https://github.com/blackeyelabs/orbit/actions/workflows/ci.yml/badge.svg" alt="CI Status"></a>
  <a href="https://github.com/blackeyelabs/orbit/releases"><img src="https://img.shields.io/github/v/release/blackeyelabs/orbit?include_prereleases&label=version&color=blue" alt="Version"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-2024-orange.svg?logo=rust" alt="Rust 2024"></a>
  <a href="https://github.com/blackeyelabs/orbit"><img src="https://img.shields.io/badge/platform-Linux-lightgrey.svg?logo=linux" alt="Platform: Linux"></a>
  <a href="docs/adr/"><img src="https://img.shields.io/badge/ADRs-documented-blue" alt="ADRs"></a>
</p>

<p align="center">
  Orbit keeps your notes as plain markdown files in a folder you own. You edit them in a live-preview editor that renders as you write and shows the raw syntax wherever your cursor is. Folders can be encrypted, and Mermaid diagrams render natively.
</p>

> [!NOTE]
> **Status: early design and prototyping.** There is no stable release yet. The feature list below describes the plan and active architectural components.

---

## Features

- **Live preview editor.** Markdown renders in place, and the syntax reappears where the caret or selection touches it. Your files stay plain text, and front matter is never rewritten.
- **Full GitHub-flavored markdown.** Tables, task lists, footnotes, alerts, math, syntax-highlighted code, emoji shortcodes, images, and a safe subset of inline HTML.
- **Linked notes.** `[[wikilinks]]`, embeds, backlinks, tags, and front matter properties.
- **Fast search.** Full-text search, a quick switcher, and a command palette, built for vaults of up to about 10,000 notes.
- **Protected folders.** Encrypt a folder, or a whole vault, with a passphrase. Filenames and folder structure are encrypted too. While locked, protected notes are hidden from the file tree, search, and backlinks.
- **Mermaid diagrams.** Flowcharts, sequence, class, ER, state, Gantt, mindmap, and more, rendered in Rust with no JavaScript runtime or web view.
- **Local and portable.** No account, no cloud requirement, and no lock-in. Your vault is a folder of `.md` files.

---

## Installation (Linux)

### Universal One-Line Installer

The installer auto-detects your distribution and installs the optimal native package (`.deb`, `.rpm`, `.pkg.tar.zst`, Nix, or AppImage):

```sh
curl -fsSL https://raw.githubusercontent.com/blackeyelabs/orbit/main/install.sh | bash
```

### Package Repositories

#### Debian / Ubuntu / Mint / Pop!_OS (APT)
```sh
curl -fsSL https://blackeyelabs.github.io/orbit/keys/orbit.gpg.key | sudo gpg --dearmor -o /etc/apt/keyrings/orbit.gpg
echo "deb [signed-by=/etc/apt/keyrings/orbit.gpg] https://blackeyelabs.github.io/orbit/apt stable main" | sudo tee /etc/apt/sources.list.d/orbit.list
sudo apt update && sudo apt install orbit
```

#### Fedora / RHEL / CentOS (DNF)
```sh
sudo dnf config-manager --add-repo https://blackeyelabs.github.io/orbit/rpm/orbit.repo
sudo dnf install orbit
```

#### Arch Linux / Manjaro / CachyOS (Pacman / AUR)
Install directly from the AUR:
```sh
yay -S orbit-bin
# or paru -S orbit-bin
```
Or add the repository to `/etc/pacman.conf`:
```ini
[orbit]
SigLevel = Required DatabaseOptional
Server = https://blackeyelabs.github.io/orbit/pacman/$arch
```

#### Nix / NixOS (Flakes)
```sh
nix run github:blackeyelabs/orbit
# or install into user profile:
nix profile install github:blackeyelabs/orbit
```

#### Standalone AppImage & Release Tarballs
Pre-compiled `.AppImage`, `.deb`, `.rpm`, and `.tar.gz` release artifacts are available directly on [GitHub Releases](https://github.com/blackeyelabs/orbit/releases/latest).

---

## How It Is Built

- **Pure Rust UI on [GPUI Kit](https://gpui-kit.com):** Linux is our first target; macOS and Windows will follow.
- **Headless domain core:** The domain crates (`orbit-core`, `orbit-fs`, `orbit-crypto`, `orbit-index`, `md-engine`, `diagrams`) never depend on the UI, allowing full headless testing and reuse across other front ends. Architectural separation is verified in CI via `scripts/check-core-deps.sh`.
- **Audited cryptography:** Encryption uses audited RustCrypto primitives: Argon2id key derivation and XChaCha20-Poly1305. Protected content never touches the on-disk index or cache.
- **Search & Storage:** Search is powered by `tantivy`, markdown parsing uses `comrak`, and all disk operations pass through a unified `VaultFs` layer.

---

## Security Notes

Protected areas hide file names, directory structures, and contents. Someone with access to your disk can still see that encrypted areas exist, roughly how many files they hold, and approximate sizes. If you lose both your passphrase and your recovery key, the data cannot be recovered. Orbit has not had an independent third-party security review.

---

## Developer Workflow

Orbit uses [just](https://github.com/casey/just) to automate development, testing, and packaging workflows:

```sh
just                  # display all available recipes
just run              # run Orbit in debug mode
just run-vault <path> # open Orbit against a specific vault
just test             # run workspace unit and integration tests
just guard            # verify core crates remain free of UI dependencies
just ci               # run full CI suite (fmt, clippy, tests, guard)
just package-all      # build .deb, .rpm, .pkg.tar.zst, .AppImage, and tarball
```

---

## Logs & Hardware Acceleration

Orbit logs to stderr and daily rotating files under `~/.local/state/orbit/logs` (`$XDG_STATE_HOME/orbit/logs`), retained for 7 days.

Control logging level via `ORBIT_LOG`:
```sh
ORBIT_LOG=warn,orbit=debug just run
```

With `ORBIT_LOG=info`, Orbit reports the GPU selected for rendering. To force a specific GPU on multi-GPU systems, specify `ZED_DEVICE_ID` with its PCI device ID (e.g., `ZED_DEVICE_ID=0x9a60`). Logs never contain note text, secrets, or paths in protected folders ([ADR 0007](docs/adr/0007-logs-carry-no-vault-content.md)).

---

## Documentation

- Vocabulary: [`CONTEXT.md`](CONTEXT.md)
- Architectural Decisions: [`docs/adr/`](docs/adr/)
- Design & Roadmap: [`docs/Orbit implementation plan for Claude Code.md`](docs/Orbit%20implementation%20plan%20for%20Claude%20Code.md)
- Phase 0 Decisions & Spikes: [`docs/phase0-gate.md`](docs/phase0-gate.md) and [`spikes/SPIKES.md`](spikes/SPIKES.md)
- Protected Folder Format: [`docs/protected-folder-format.md`](docs/protected-folder-format.md)

---

## Roadmap

1. **Foundation:** Workspace setup, editor and Mermaid prototypes, encrypted storage format.
2. **MVP:** Split source and preview editing, vault browsing, search, backlinks, tags, protected folders, Mermaid blocks.
3. **Core:** Full live preview editor with block widgets, whole-vault protection, link-aware rename, attachments.
4. **Hardening:** Performance at scale, fuzzing, security review, packaging, followed by macOS and Windows.

*Graph view, plugins, and sync will follow these initial phases.*

---

## Contributing

Orbit is currently in an early architectural stage where design feedback is the most useful contribution. Please open an issue to discuss ideas before sending a pull request.

---

## License

Orbit is released under the [MIT License](LICENSE).
