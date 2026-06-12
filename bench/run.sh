#!/usr/bin/env bash
#
# Cross-runtime benchmark: esrun vs Node.js vs Bun vs Deno.
#
# All workloads use only Web APIs common to every runtime, so the same script
# runs unmodified on each. Startup is measured as process wall-time on a near-
# empty script; the other workloads time themselves with performance.now() and
# print RESULT_MS, which this harness parses (isolating engine/crypto cost from
# process launch).
#
# Usage:  bench/run.sh            (auto-detects installed runtimes)
#         ESRUN=/path/to/esrun bench/run.sh
#         STARTUP_RUNS=30 bench/run.sh
set -uo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-../target/release/esrun}"
STARTUP_RUNS="${STARTUP_RUNS:-25}"
WORKLOAD_RUNS="${WORKLOAD_RUNS:-3}"

# Resolve each runtime's invocation, in display order. Skipped if not found.
declare -A CMD VER
add() { # name  "invocation"  "version-cmd"
  CMD[$1]="$2"
  VER[$1]="$($3 2>/dev/null | head -1)"
}
ORDER=()
command -v node >/dev/null 2>&1 && { add node "node" "node --version"; ORDER+=(node); }
command -v bun  >/dev/null 2>&1 && { add bun  "bun"  "bun --version";  ORDER+=(bun);  }
DENO="$(command -v deno 2>/dev/null || ([ -x /tmp/deno/bin/deno ] && echo /tmp/deno/bin/deno))"
[ -n "$DENO" ] && { add deno "$DENO run -A --quiet" "$DENO --version"; ORDER+=(deno); }
if [ -x "$ESRUN" ]; then add esrun "$ESRUN" "$ESRUN --version"; ORDER+=(esrun); else
  echo "esrun not found at $ESRUN — build it: cargo build --release -p es-runtime-cli" >&2; exit 1
fi

now() { date +%s.%N; }
to_ms() { awk "BEGIN{printf \"%.1f\", $1*1000}"; }

# Min process wall-time over STARTUP_RUNS runs (plus one discarded warmup).
measure_startup() {
  local cmd="$1" best="" s e d
  $cmd scripts/startup.js >/dev/null 2>&1   # warmup
  for _ in $(seq "$STARTUP_RUNS"); do
    s=$(now); $cmd scripts/startup.js >/dev/null 2>&1; e=$(now)
    d=$(awk "BEGIN{print $e-$s}")
    [ -z "$best" ] && best=$d
    awk "BEGIN{exit !($d < $best)}" && best=$d
  done
  to_ms "$best"
}

# Best (min) self-timed RESULT_MS over WORKLOAD_RUNS runs.
measure_workload() {
  local cmd="$1" script="$2" out best=""
  for _ in $(seq "$WORKLOAD_RUNS"); do
    out=$($cmd "scripts/$script" 2>/dev/null | grep -oE 'RESULT_MS=[0-9.]+' | head -1 | cut -d= -f2)
    [ -z "$out" ] && { echo "ERR"; return; }
    [ -z "$best" ] && best=$out
    awk "BEGIN{exit !($out < $best)}" && best=$out
  done
  printf "%.1f" "$best"
}

echo "ES-Runtime cross-runtime benchmark"
echo "=================================="
for r in "${ORDER[@]}"; do printf "  %-6s %s\n" "$r" "${VER[$r]}"; done
echo
echo "Workloads: startup (process wall-time, min of $STARTUP_RUNS); compute (20M-iter math);"
echo "sha256 (20k x 4KiB SHA-256); webapi (100k URL parse + TextEncoder/Decoder)."
echo "All times in milliseconds, lower is better."
echo

printf "%-8s | %10s | %10s | %10s | %10s\n" "runtime" "startup" "compute" "sha256" "webapi"
printf -- "---------+------------+------------+------------+------------\n"
for r in "${ORDER[@]}"; do
  printf "%-8s | %10s | %10s | %10s | %10s\n" \
    "$r" \
    "$(measure_startup "${CMD[$r]}")" \
    "$(measure_workload "${CMD[$r]}" compute.js)" \
    "$(measure_workload "${CMD[$r]}" sha256.js)" \
    "$(measure_workload "${CMD[$r]}" webapi.js)"
done
