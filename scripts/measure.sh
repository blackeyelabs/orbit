#!/usr/bin/env bash
# Measures Orbit running one scenario (docs/research/performance.md), apart
# from any Orbit already running: its own config, state, cache and runtime
# folders, the compositor reached by absolute path.
#
#   scripts/measure.sh <orbit binary> <vault> <scenario> [warm-up s] [measure s] [work dir]
#
# The scenario is an ORBIT_SCENARIO value (the binary needs the
# dev-profiler feature): idle, open:<note> or type:<note>. The search index
# and the session live in the vault's own .vault/: a freshly made vault
# measures a cold start, and one Orbit has opened before a warm start (with
# the tabs it left open). Memory comes from /proc/<pid>/status: Orbit makes
# itself non-dumpable, which keeps smaps out of reach even of its user.
# Prints a Markdown table row:
#   scenario | CPU ms/s | RSS MB | anonymous MB | file-backed MB |
#   peak RSS MB | threads | indexing ms | changed_on_disk | search updates
set -euo pipefail

bin=$1 vault=$2 scenario=$3
warmup=${4:-20} span=${5:-30}
work=${6:-$(mktemp -d)}
mkdir -p "$work"/{config,state,cache,runtime}
chmod 700 "$work/runtime"

display=${WAYLAND_DISPLAY:-wayland-0}
[[ $display == /* ]] || display="${XDG_RUNTIME_DIR:?needed to find the compositor}/$display"

XDG_RUNTIME_DIR="$work/runtime" WAYLAND_DISPLAY="$display" \
  XDG_CONFIG_HOME="$work/config" XDG_STATE_HOME="$work/state" \
  XDG_CACHE_HOME="$work/cache" ORBIT_SCENARIO="$scenario" \
  ORBIT_LOG="warn,orbit=debug" \
  "$bin" "$vault" >"$work/stderr.log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true' EXIT

# CPU time so far, in clock ticks: utime + stime. Orbit gone early is said,
# with where its output is.
ticks() {
  [[ -r /proc/$pid/stat ]] || { echo "Orbit exited early: see $work/stderr.log" >&2; exit 1; }
  awk '{print $14 + $15}' "/proc/$pid/stat"
}
hz=$(getconf CLK_TCK)

sleep "$warmup"
t0=$(ticks)
sleep "$span"
t1=$(ticks)

kb() { awk -v key="$1:" '$1 == key {print $2}' "$2"; }
mb() { awk -v k="$1" 'BEGIN {printf "%.0f", k / 1024}'; }
status=/proc/$pid/status
cpu=$(awk -v d="$((t1 - t0))" -v hz="$hz" -v s="$span" 'BEGIN {printf "%.1f", d * 1000 / hz / s}')
rss=$(mb "$(kb VmRSS "$status")")
anon=$(mb "$(kb RssAnon "$status")")
file=$(mb "$(kb RssFile "$status")")
peak=$(mb "$(kb VmHWM "$status")")
threads=$(kb Threads "$status")

# This run's lines only: a reused work dir's log holds earlier runs too.
run=$({ cat "$work"/state/orbit/logs/*.log 2>/dev/null || true; } |
  awk -v pid="pid=$pid " 'index($0, pid) && /Orbit started/ {found = 1} found')
indexing=$(grep -o 'search index up to date.*ms=[0-9]*' <<<"$run" | grep -o 'ms=[0-9]*' | head -1 | cut -d= -f2 || true)
changed=$(grep -c 'window.changed_on_disk.*close' <<<"$run" || true)
updates=$(grep -c 'search.update.*close' <<<"$run" || true)

echo "| $scenario | $cpu | $rss | $anon | $file | $peak | $threads | ${indexing:--} | $changed | $updates |"
