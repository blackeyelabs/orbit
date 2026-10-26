# Orbit

A local-first, Obsidian-like workspace for markdown notes and Mermaid diagrams, written in Rust on GPUI Kit. Linux first.

- Vocabulary: [`CONTEXT.md`](CONTEXT.md)
- Decisions: [`docs/adr/`](docs/adr/)
- Design and roadmap: [`docs/Orbit implementation plan for Claude Code.md`](docs/Orbit%20implementation%20plan%20for%20Claude%20Code.md)
- Phase 0 decisions: [`docs/phase0-gate.md`](docs/phase0-gate.md) (evidence in [`spikes/SPIKES.md`](spikes/SPIKES.md))
- Protected-folder format: [`docs/protected-folder-format.md`](docs/protected-folder-format.md)

## Development

```sh
cargo run -p app          # run Orbit
scripts/ci.sh             # everything CI runs: fmt, clippy, tests, core-deps guard
```

The core crates (`md-engine`, `orbit-core`, `orbit-index`, `orbit-crypto`, `orbit-fs`, `diagrams`) must never depend on GPUI. `scripts/check-core-deps.sh` enforces this in CI.
