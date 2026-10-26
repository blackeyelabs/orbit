#!/usr/bin/env bash
# Tests check-core-deps.sh against throwaway copies of the workspace: the
# guard must pass on the tree as committed and fail once a core crate pulls
# in GPUI, directly or through another core crate.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
guard="$root/scripts/check-core-deps.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

copy_workspace() {
  local dest="$1"
  mkdir -p "$dest"
  cp "$root/Cargo.toml" "$root/Cargo.lock" "$dest/"
  cp -r "$root/crates" "$dest/"
}

# Adds gpui-kit as a normal dependency of the given crate in a copy.
add_gpui() {
  local dest="$1" crate="$2"
  sed -i 's/^\[dependencies\]$/[dependencies]\ngpui-kit.workspace = true/' "$dest/crates/$crate/Cargo.toml"
  # A real change would commit the updated lockfile, and the guard runs --locked.
  (cd "$dest" && cargo tree --offline -p "$crate" >/dev/null)
}

expect() {
  local want="$1" name="$2" dir="$3" log="$3.log"
  local got=0
  (cd "$dir" && CARGO_NET_OFFLINE=true "$guard" >"$log" 2>&1) || got=$?
  if { [ "$want" = pass ] && [ "$got" -ne 0 ]; } || { [ "$want" = fail ] && [ "$got" -eq 0 ]; }; then
    echo "FAIL: $name: expected guard to $want (exit $got)"
    sed 's/^/    /' "$log"
    exit 1
  fi
  echo "ok: $name"
}

copy_workspace "$tmp/clean"
expect pass "committed tree passes" "$tmp/clean"

copy_workspace "$tmp/direct"
add_gpui "$tmp/direct" orbit-core
expect fail "core crate depending on gpui-kit fails" "$tmp/direct"
grep -q "orbit-core" "$tmp/direct.log" || { echo "FAIL: guard did not name orbit-core"; exit 1; }

copy_workspace "$tmp/transitive"
add_gpui "$tmp/transitive" orbit-crypto
expect fail "gpui reached through another core crate fails" "$tmp/transitive"
grep -q "orbit-fs" "$tmp/transitive.log" || { echo "FAIL: guard did not name orbit-fs (depends on orbit-crypto)"; exit 1; }

# A core crate the guard can't inspect (renamed, removed, unresolvable) must
# fail loudly, never count as GPUI-free.
copy_workspace "$tmp/missing"
sed -i 's/^name = "diagrams"$/name = "diagrams-renamed"/' "$tmp/missing/crates/diagrams/Cargo.toml"
expect fail "core crate cargo can't resolve fails" "$tmp/missing"
