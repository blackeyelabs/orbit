#!/usr/bin/env bash
# Fails if any core crate depends on GPUI, directly or transitively.
# The core must stay headless so it can be tested without a window and
# reused by orbit-api; only `editor` and `app` may link GPUI.
set -euo pipefail

core_crates=(md-engine orbit-core orbit-index orbit-crypto orbit-fs diagrams)

status=0
for crate in "${core_crates[@]}"; do
  # Normal edges only: a GPUI dev-dependency for tests would be acceptable.
  # cargo tree runs on its own so a crate it can't resolve fails the guard
  # instead of looking GPUI-free.
  tree="$(cargo tree --locked -p "$crate" -e normal --prefix none --format '{p}')"
  hits="$(grep -E '^gpui' <<<"$tree" | sort -u || true)"
  if [ -n "$hits" ]; then
    echo "error: core crate '$crate' depends on GPUI:"
    sed 's/^/  /' <<<"$hits"
    status=1
  fi
done

[ "$status" -eq 0 ] && echo "core crates are GPUI-free"
exit "$status"
