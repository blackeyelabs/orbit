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

## Logs

Orbit logs to stderr and to a file a day under `~/.local/state/orbit/logs` (`$XDG_STATE_HOME/orbit/logs`), kept a week. **Open log folder**, in Settings › Logs or the Command palette, shows them.

By default Orbit's own crates log at info and everything else only warnings and errors. Turn on **Detailed logging** in Settings to record debug lines for a bug report, or set the filter when starting Orbit:

```sh
ORBIT_LOG=warn,orbit=debug cargo run -p app   # `orbit` covers orbit_core and the rest too
```

`ORBIT_LOG` takes `tracing` filter directives and wins over `RUST_LOG`. Logs never hold note text, secrets or any path in a protected folder ([ADR 0007](docs/adr/0007-logs-carry-no-vault-content.md)).
