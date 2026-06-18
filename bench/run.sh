#!/usr/bin/env bash
#
# Cross-runtime benchmark: esrun vs Node.js vs Bun vs Deno vs LLRT.
#
# Methodology (see bench/README.md for the rationale and sources):
#  * All workloads use only Web APIs common to every runtime, so the same script
#    runs unmodified on each.
#  * Each script does an untimed in-process warmup (JIT steady state) and times
#    itself with performance.now(), printing RESULT_MS — isolating engine cost
#    from process launch. Startup/bigscript instead measure process wall-time
#    (the launch + parse cost is the metric).
#  * INTERLEAVED + RANDOMIZED: every repetition samples each runtime once per
#    row back-to-back, with the runtime order shuffled, so all candidates share
#    the same contention window. Relative ranking then reflects real differences
#    — not which minute a runtime happened to be measured in.
#  * PROCESS WARMUP: the first repetition is discarded (fills caches, lets the
#    JIT/OS settle) on top of each script's in-process warmup.
#  * AGGREGATION = MIN over repetitions: interference only ever *adds* time, so
#    the minimum is the contention-free floor — the stable, fair comparator.
#  * NOISE is disclosed, not hidden: the coefficient of variation per cell is
#    computed; cells above NOISE_THRESHOLD% are flagged (`~`) and listed.
#  * Peak RSS is sampled per runtime (GNU time, skipped if absent).
#
# The fetch workload runs against a local HTTP server on 127.0.0.1:18923
# (started here with Node; the workload is skipped if Node is missing or the
# port is taken).
#
# Usage:  bench/run.sh                      (auto-detects installed runtimes)
#         ESRUN=/path/to/esrun bench/run.sh
#         WORKLOADS="url encoding" bench/run.sh   (subset of workloads)
#         WORKLOAD_RUNS=15 bench/run.sh           (more samples per workload)
#         QUIET=1 bench/run.sh                    (pin CPU + disable ASLR, etc.)
#         BENCH_JSON=1 bench/run.sh > results.json (machine-readable output)
set -uo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-../target/release/esrun}"
STARTUP_RUNS="${STARTUP_RUNS:-25}"
WORKLOAD_RUNS="${WORKLOAD_RUNS:-9}"
# Coefficient-of-variation (%) above which a measured cell is flagged as noisy.
NOISE_THRESHOLD="${NOISE_THRESHOLD:-5}"
ALL_WORKLOADS="compute json jsonbig sha256 crypto url url_setter urlpattern encoding base64 structured async timers streams fetch http websocket fsread_small fsread_large fswrite_small fswrite_large fsappend_small fsappend_large fsstat_small fsstat_large fsexists_small fsexists_large glob xml_small xml_large"
WORKLOADS="${WORKLOADS:-$ALL_WORKLOADS}"
BENCH_JSON="${BENCH_JSON:-}"
FETCH_PORT=18923
WS_PORT=18924
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
WS_SERVER_PID=""
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$WS_SERVER_PID" ] && kill "$WS_SERVER_PID" 2>/dev/null
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

# --- websocket echo server --------------------------------------------------

# Starts the local WebSocket echo server for the websocket workload. The clients
# are each runtime's standard `WebSocket` global; the server is whichever
# built-in WS server is available — Bun (`Bun.serve`), Deno (`Deno.upgradeWebSocket`),
# or Node with the `ws` package — so it needs no bundled dependency. Drops the
# workload (with a notice) if none is available or the port doesn't come up.
start_ws_server() {
  case " $WORKLOADS " in *" websocket "*) ;; *) return ;; esac
  local cmd="" script="$SCRATCH/ws-echo.js"
  if command -v bun >/dev/null 2>&1; then
    cmd="bun"
    cat > "$script" <<EOF
Bun.serve({ port: $WS_PORT, hostname: "127.0.0.1",
  fetch(req, server) { if (server.upgrade(req)) return; return new Response("", { status: 400 }); },
  websocket: { message(ws, m) { ws.send(m); } } });
EOF
  elif [ -n "$DENO" ]; then
    cmd="$DENO run -A --quiet"
    cat > "$script" <<EOF
Deno.serve({ port: $WS_PORT, hostname: "127.0.0.1" }, (req) => {
  const { socket, response } = Deno.upgradeWebSocket(req);
  socket.onmessage = (e) => socket.send(e.data);
  return response;
});
EOF
  elif command -v node >/dev/null 2>&1 && node -e 'require("ws")' >/dev/null 2>&1; then
    cmd="node"
    cat > "$script" <<EOF
const { WebSocketServer } = require("ws");
new WebSocketServer({ host: "127.0.0.1", port: $WS_PORT })
  .on("connection", (ws) => ws.on("message", (m) => ws.send(m, { binary: false })));
EOF
  else
    note "websocket workload skipped (needs bun, deno, or node+ws for the echo server)"
    WORKLOADS="${WORKLOADS//websocket/}"
    return
  fi
  $cmd "$script" >/dev/null 2>&1 &
  WS_SERVER_PID=$!
  for _ in $(seq 50); do
    (echo > "/dev/tcp/127.0.0.1/$WS_PORT") 2>/dev/null && return
    sleep 0.1
  done
  note "websocket workload skipped (echo server did not come up on :$WS_PORT)"
  kill "$WS_SERVER_PID" 2>/dev/null
  WS_SERVER_PID=""
  WORKLOADS="${WORKLOADS//websocket/}"
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

# Optional environment hardening (opt-in: QUIET=1). Pins every runtime to the
# same CPU and disables ASLR so all candidates face identical conditions; `nice`
# is applied only as root. Governor/turbo need sudo and are printed as a hint.
# The wrapper prefixes every timed launch (but not RSS sampling, where it would
# confuse the resident-set reading of the immediate child).
WRAP=""
if [ -n "${QUIET:-}" ]; then
  command -v taskset >/dev/null 2>&1 && WRAP="taskset -c ${BENCH_CPU:-0} "
  [ "$(id -u)" = 0 ] && WRAP="${WRAP}nice -n -20 "
  setarch -R true >/dev/null 2>&1 && WRAP="${WRAP}setarch -R "
  note "QUIET: launches wrapped with: ${WRAP:-<none available>}"
  note "QUIET: for lowest variance also run (sudo): cpupower frequency-set -g performance; echo 0 | sudo tee /sys/devices/system/cpu/cpufreq/boost — and close background apps"
fi

# A single timed launch → one ms sample (or "ERR" if the runtime can't run it).
#   startup: process wall-time.   workload: the script's self-reported RESULT_MS.
sample_once() {  # kind cmd script
  local kind="$1" cmd="$2" script="$3" s e out
  if [ "$kind" = startup ]; then
    s=$(now); $WRAP $cmd "$script" >/dev/null 2>&1; e=$(now)
    to_ms "$(awk "BEGIN{print $e-$s}")"
  else
    out=$($TIMEOUT_BIN $WRAP $cmd "$script" 2>/dev/null | grep -oE 'RESULT_MS=[0-9.]+' | head -1 | cut -d= -f2)
    [ -z "$out" ] && { echo ERR; return; }
    awk "BEGIN{printf \"%.1f\", $out}"
  fi
}

# Reduces a list of samples to "min cov%": min is the contention-free floor
# (interference only ever adds time); cov is the coefficient of variation, used
# to flag noisy cells.
aggregate() {  # "s1 s2 ..."
  awk '{ for (i=1;i<=NF;i++){ x=$i; sum+=x; sq+=x*x; n++; if (n==1 || x<min) min=x } }
       END{ if (n==0){ print "ERR 0"; exit }
            mean=sum/n; var=(n>1)?(sq-n*mean*mean)/(n-1):0; if (var<0) var=0;
            cov=(mean>0)?100*sqrt(var)/mean:0;
            printf "%.1f %.1f", min, cov }' <<<"$1"
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

shuffle() {  # randomize runtime order each repetition (falls back to fixed)
  if command -v shuf >/dev/null 2>&1; then shuf -e "$@"; else printf '%s\n' "$@"; fi
}

# --- run --------------------------------------------------------------------

start_fetch_server
start_ws_server
BIGSCRIPT="$(gen_bigscript)"

# Rows and their (kind, script path).
declare -A KIND PATHS
KIND[startup]=startup;   PATHS[startup]="scripts/startup.js"
KIND[bigscript]=startup; PATHS[bigscript]="$BIGSCRIPT"
ROWS=(startup bigscript)
for w in $WORKLOADS; do KIND[$w]=workload; PATHS[$w]="scripts/$w.js"; ROWS+=("$w"); done

declare -A SAMPLES RES COV DEAD

# Interleaved + randomized collection (see header). Repetition 0 is the
# discarded process-level warmup; a row a runtime can't run errors on that warmup
# and is then skipped entirely (marked DEAD), so unsupported workloads cost one
# launch instead of N.
collect() {  # row reps
  local row="$1" reps="$2" rep r s
  for rep in $(seq 0 "$reps"); do
    while read -r r; do
      [ -n "${DEAD[$row,$r]:-}" ] && continue
      s=$(sample_once "${KIND[$row]}" "${CMD[$r]}" "${PATHS[$row]}")
      if [ "$s" = ERR ]; then
        [ "$rep" -eq 0 ] && DEAD[$row,$r]=1
        continue
      fi
      [ "$rep" -eq 0 ] && continue   # discard warmup repetition
      SAMPLES[$row,$r]="${SAMPLES[$row,$r]:-} $s"
    done < <(shuffle "${ORDER[@]}")
  done
}

for row in "${ROWS[@]}"; do
  if [ "${KIND[$row]}" = startup ]; then collect "$row" "$STARTUP_RUNS"; else collect "$row" "$WORKLOAD_RUNS"; fi
done

# Aggregate each cell to its min + CoV; collect the noisy ones to disclose.
NOISY=()
for row in "${ROWS[@]}"; do
  for r in "${ORDER[@]}"; do
    if [ -z "${SAMPLES[$row,$r]:-}" ]; then RES[$row,$r]=ERR; continue; fi
    read -r m c < <(aggregate "${SAMPLES[$row,$r]}")
    RES[$row,$r]=$m; COV[$row,$r]=$c
    awk "BEGIN{exit !($c > $NOISE_THRESHOLD)}" && NOISY+=("$row/$r ${c}%")
  done
done

# RSS is a memory floor (contention doesn't inflate peak RSS): one sample each.
declare -A RSS
for row in "${ROWS[@]}"; do
  for r in "${ORDER[@]}"; do
    if [ "${RES[$row,$r]:-}" != ERR ] && [ -n "${RES[$row,$r]:-}" ]; then
      RSS[$row,$r]=$(measure_rss "${CMD[$r]}" "${PATHS[$row]}")
    fi
  done
done

# Restoring 'rss' row for site compatibility (represents startup memory floor)
for r in "${ORDER[@]}"; do RES[rss,$r]="${RSS[startup,$r]:-}"; done
[ -n "${RES[rss,${ORDER[0]}]:-}" ] && ROWS+=(rss)

[ "${#NOISY[@]}" -gt 0 ] &&
  note "noisy cells (CoV > ${NOISE_THRESHOLD}%; min floor still shown, marked ~): ${NOISY[*]}"

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
  printf '\n  },\n  "results_rss": {'
  firstrow=1
  for row in "${ROWS[@]}"; do
    [ -z "$firstrow" ] && printf ','
    firstrow=
    printf '\n    "%s": {' "$row"
    first=1
    for r in "${ORDER[@]}"; do
      [ -z "$first" ] && printf ','
      first=
      v="${RSS[$row,$r]:-null}"
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
echo "Interleaved + randomized runs; min of N (the contention-free floor), after a"
echo "discarded warmup. startup/bigscript: process wall-time, min of $STARTUP_RUNS."
echo "Other workloads: self-timed after an in-process warmup, min of $WORKLOAD_RUNS."
echo "Memory: peak resident set (MB) sampled once per workload."
echo "All times in milliseconds, lower is better. Format: time / memory."
echo "~ marks a noisy cell (CoV > ${NOISE_THRESHOLD}%)."
echo

printf "%-15s" "workload"
for r in "${ORDER[@]}"; do printf " | %13s" "$r"; done
printf "\n"
printf -- "---------------"
for _ in "${ORDER[@]}"; do printf -- "+--------------"; done
printf "\n"
for row in "${ROWS[@]}"; do
  printf "%-15s" "$row"
  for r in "${ORDER[@]}"; do
    v="${RES[$row,$r]:--}"
    if [ "$v" = ERR ]; then
      v="n/a" # workload the runtime doesn't support
    else
      # Flag a noisy cell so a wobbly number isn't read as precise.
      c="${COV[$row,$r]:-0}"
      awk "BEGIN{exit !($c > $NOISE_THRESHOLD)}" && v="${v}~"
      if [ -n "${RSS[$row,$r]:-}" ]; then
        v="${v} / ${RSS[$row,$r]}M"
      fi
    fi
    printf " | %13s" "$v"
  done
  printf "\n"
done
