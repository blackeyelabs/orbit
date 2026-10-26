#!/usr/bin/env bash
# Runs the same checks as CI. Usage: scripts/ci.sh
# CI lists these as separate workflow steps for readable logs; keep the two in sync.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
scripts/check-core-deps.sh
scripts/test-check-core-deps.sh
