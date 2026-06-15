#!/usr/bin/env bash
#
# Cross-runtime benchmark: esrun vs Node.js vs Bun vs Deno.
#
# All workloads use only Web APIs common to every runtime, so the same script
# runs unmodified on each. Startup is measured as process wall-time (min of N —
# the floor is the launch cost) on a near-empty script and on a generated
# ~100 KB script (parse cost; the startup snapshot only pre-bakes the prelude,
# not user code). The other workloads run an untimed JIT warmup, time
# themselves with performance.now(), and print RESULT_MS, which this harness
# parses (isolating engine/runtime cost from process launch); the harness
# reports the *median* of WORKLOAD_RUNS runs so a single noisy run can't set
# the number. Peak RSS is sampled per runtime (GNU time, skipped if absent).
#
# The fetch workload runs against a local HTTP server on 127.0.0.1:18923
# (started here with Node; the workload is skipped if Node is missing or the
# port is taken).
#
# Usage:  bench/run.sh                      (auto-detects installed runtimes)
#         ESRUN=/path/to/esrun bench/run.sh
#         WORKLOADS="url encoding" bench/run.sh   (subset of workloads)
#         BENCH_JSON=1 bench/run.sh > results.json (machine-readable output)
set -uo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-../target/release/esrun}"
STARTUP_RUNS="${STARTUP_RUNS:-25}"
WORKLOAD_RUNS="${WORKLOAD_RUNS:-5}"
ALL_WORKLOADS="compute json jsonbig sha256 crypto url encoding base64 structured async timers streams fetch http fsread fswrite fsappend glob"
WORKLOADS="${WORKLOADS:-$ALL_WORKLOADS}"
BENCH_JSON="${BENCH_JSON:-}"
FETCH_PORT=18923
# Per-workload wall-clock cap so a runtime that can't run a workload (or stalls
# trying — e.g. a partial server API that never responds) yields a clean n/a
# instead of hanging the whole run. Applied via `timeout` if available.
TIMEOUT_BIN=""
command -v timeout >/dev/null 2>&1 && TIMEOUT_BIN="timeout ${WORKLOAD_TIMEOUT:-60}"

# Resolve each runtime's invocation, in display order. Skipped if not found.
declare -A CMD VER
add() { # name  "invocation"  "version-cmd"
  CMD[$1]="$2"
  VER[$1]="$($3 2>/dev/null | head -1)"
}
ORDER=()
command -v node >/dev/null 2>&1 && { add node "node" "node --version"; ORDER+=(node); }
command -v bun  >/dev/null 2>&1 && { add bun  "bun"  "bun --version";  ORDER+=(bun);  }
DENO="$(command -v deno 2>/dev/null)"
[ -z "$DENO" ] && for d in "$HOME/.deno/bin/deno" /tmp/deno/bin/deno; do
  [ -x "$d" ] && { DENO="$d"; break; }
done
[ -n "$DENO" ] && { add deno "$DENO run -A --quiet" "$DENO --version"; ORDER+=(deno); }
# LLRT (AWS Low Latency Runtime): QuickJS-based, cold-start/memory focused. Runs
# the engine + Web-API workloads it supports; the fs/streams/glob/http workloads
# fall through to n/a (it has no general HTTP server and only partial fs). Looked
# for on PATH, then the usual install spots.
LLRT="$(command -v llrt 2>/dev/null)"
[ -z "$LLRT" ] && for d in "$HOME/.llrt/bin/llrt" "$HOME/.local/bin/llrt" /tmp/llrt/llrt; do
  [ -x "$d" ] && { LLRT="$d"; break; }
done
[ -n "$LLRT" ] && { add llrt "$LLRT" "$LLRT --version"; ORDER+=(llrt); }
if [ -x "$ESRUN" ]; then add esrun "$ESRUN" "$ESRUN --version"; ORDER+=(esrun); else
  echo "esrun not found at $ESRUN — build it: cargo build --release -p es-runtime-cli" >&2; exit 1
fi

# Scratch dir (generated bigscript, RSS samples), cleaned on exit.
SCRATCH="$(mktemp -d)"
SERVER_PID=""
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  rm -rf "$SCRATCH"
}
trap cleanup EXIT

# --- fetch server -----------------------------------------------------------

# Starts the local HTTP server for the fetch workload. Drops the workload (with
# a notice) if Node is missing or the port doesn't come up.
start_fetch_server() {
  case " $WORKLOADS " in *" fetch "*) ;; *) return ;; esac
  if ! command -v node >/dev/null 2>&1; then
    note "fetch workload skipped (needs node for the local server)"
    WORKLOADS="${WORKLOADS//fetch/}"
    return
  fi
  node -e '
    const http = require("http");
    http.createServer((req, res) => {
      res.setHeader("content-type", "text/plain");
      res.end("x".repeat(64));
    }).listen('"$FETCH_PORT"', "127.0.0.1");
  ' &
  SERVER_PID=$!
  for _ in $(seq 50); do
    (echo > "/dev/tcp/127.0.0.1/$FETCH_PORT") 2>/dev/null && return
    sleep 0.1
  done
  note "fetch workload skipped (server did not come up on :$FETCH_PORT)"
  kill "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
  WORKLOADS="${WORKLOADS//fetch/}"
}

# --- generated big script (startup/parse cost) ------------------------------

gen_bigscript() {
  local f="$SCRATCH/bigscript.js" i
  {
    for i in $(seq 700); do
      printf 'function fn%d(a, b) { const o = { id: %d, tag: "abcdefghij-%d" }; let t = 0; for (let j = 0; j < 3; j++) t += j * a + b + o.id; return t + o.tag.length; }\n' "$i" "$i" "$i"
    done
    echo 'let total = 0;'
    for i in $(seq 700); do printf 'total += fn%d(%d, 1);\n' "$i" "$i"; done
    echo 'void total;'
  } > "$f"
  echo "$f"
}

# --- measurement ------------------------------------------------------------

now() { date +%s.%N; }
to_ms() { awk "BEGIN{printf \"%.1f\", $1*1000}"; }

# Min process wall-time over STARTUP_RUNS runs (plus one discarded warmup).
measure_startup() {
  local cmd="$1" script="$2" best="" s e d
  $cmd "$script" >/dev/null 2>&1   # warmup
  for _ in $(seq "$STARTUP_RUNS"); do
    s=$(now); $cmd "$script" >/dev/null 2>&1; e=$(now)
    d=$(awk "BEGIN{print $e-$s}")
    [ -z "$best" ] && best=$d
    awk "BEGIN{exit !($d < $best)}" && best=$d
  done
  to_ms "$best"
}

# Median self-timed RESULT_MS over WORKLOAD_RUNS runs (lower median index for
# even counts).
measure_workload() {
  local cmd="$1" script="$2" out
  local results=()
  for _ in $(seq "$WORKLOAD_RUNS"); do
    out=$($TIMEOUT_BIN $cmd "scripts/$script" 2>/dev/null | grep -oE 'RESULT_MS=[0-9.]+' | head -1 | cut -d= -f2)
    [ -z "$out" ] && { echo "ERR"; return; }
    results+=("$out")
  done
  printf "%s\n" "${results[@]}" | sort -g |
    awk -v n="${#results[@]}" 'NR == int((n + 1) / 2) { printf "%.1f", $1 }'
}

# Peak RSS (MB) of one run, via GNU time or a python3 getrusage fallback.
# Empty (row omitted) if neither is available.
measure_rss() {
  local cmd="$1" script="$2" kb
  if [ -x /usr/bin/time ]; then
    kb=$(/usr/bin/time -v $cmd "$script" 2>&1 >/dev/null |
      grep -oE 'Maximum resident set size \(kbytes\): [0-9]+' | grep -oE '[0-9]+$')
    [ -n "$kb" ] && awk "BEGIN{printf \"%.0f\", $kb/1024}"
  elif command -v python3 >/dev/null 2>&1; then
    python3 - "$cmd" "$script" <<'EOF'
import resource, shlex, subprocess, sys
cmd = shlex.split(sys.argv[1]) + [sys.argv[2]]
subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
print(round(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024))
EOF
  fi
}

note() { [ -z "$BENCH_JSON" ] && echo "$*" >&2; }

# --- run --------------------------------------------------------------------

start_fetch_server
BIGSCRIPT="$(gen_bigscript)"

declare -A RES
ROWS=(startup bigscript)
for w in $WORKLOADS; do ROWS+=("$w"); done

for r in "${ORDER[@]}"; do
  RES["startup,$r"]=$(measure_startup "${CMD[$r]}" scripts/startup.js)
  RES["bigscript,$r"]=$(measure_startup "${CMD[$r]}" "$BIGSCRIPT")
  for w in $WORKLOADS; do
    RES["$w,$r"]=$(measure_workload "${CMD[$r]}" "$w.js")
  done
  RES["rss,$r"]=$(measure_rss "${CMD[$r]}" scripts/startup.js)
done

# Include the rss row only when a sampler was available.
[ -n "${RES[rss,${ORDER[0]}]:-}" ] && ROWS+=(rss)

# --- output -----------------------------------------------------------------

if [ -n "$BENCH_JSON" ]; then
  printf '{\n  "runtimes": {'
  first=1
  for r in "${ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    printf '\n    "%s": "%s"' "$r" "${VER[$r]}"
  done
  printf '\n  },\n  "results_ms": {'
  firstrow=1
  for row in "${ROWS[@]}"; do
    [ -z "$firstrow" ] && printf ','
    firstrow=
    printf '\n    "%s": {' "$row"
    first=1
    for r in "${ORDER[@]}"; do
      [ -z "$first" ] && printf ','
      first=
      v="${RES[$row,$r]:-null}"
      case "$v" in ''|ERR) v=null ;; esac
      printf '\n      "%s": %s' "$r" "$v"
    done
    printf '\n    }'
  done
  printf '\n  }\n}\n'
  exit 0
fi

echo "ES-Runtime cross-runtime benchmark"
echo "=================================="
for r in "${ORDER[@]}"; do printf "  %-6s %s\n" "$r" "${VER[$r]}"; done
echo
echo "startup/bigscript: process wall-time (near-empty / ~100 KB script), min of $STARTUP_RUNS."
echo "Other workloads: self-timed after an untimed warmup, median of $WORKLOAD_RUNS."
echo "rss: peak resident set (MB) on the near-empty script."
echo "All times in milliseconds, lower is better. See scripts/*.js for shapes."
echo

printf "%-11s" "workload"
for r in "${ORDER[@]}"; do printf " | %9s" "$r"; done
printf "\n"
printf -- "-----------"
for _ in "${ORDER[@]}"; do printf -- "+-----------"; done
printf "\n"
for row in "${ROWS[@]}"; do
  printf "%-11s" "$row"
  for r in "${ORDER[@]}"; do
    v="${RES[$row,$r]:--}"
    [ "$v" = ERR ] && v="n/a" # workload the runtime doesn't support
    printf " | %9s" "$v"
  done
  printf "\n"
done
