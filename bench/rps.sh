#!/usr/bin/env bash
#
# HTTP requests/sec benchmark: a hello-world server per runtime, driven by an
# external load generator — the classic "req/s" plaintext shape (à la the
# Bun/TechEmpower charts). A separate client hammers the server over a real
# socket, so the number reflects the server alone (unlike bench/run.sh's
# in-process `http` workload, where one thread runs both client and server).
# Each runtime runs $SERVER (scripts/helloserver.js by default) with its own
# native server.
#
# Load generator: `oha` (preferred) or `bombardier` — NOT autocannon. Bun's own
# bench/express README warns autocannon's node:http client can't push a fast
# server hard enough to measure it, so we follow their setup: oha/bombardier
# plus `-H "Accept-Encoding: identity"` (stops Deno gzipping the response) and a
# fixed request count. Install: `cargo install oha`, or
# `go install github.com/codesenberg/bombardier@latest`.
#
# The client and the server share one machine, so the cores are split between
# them (see the pinning block below) — otherwise the load generator competes with
# the server it is measuring and the result describes whichever won.
#
# Results are published under a key derived from $SERVER, so a hello-world run
# cannot land in the site's Hono row.
#
# Usage:  bench/rps.sh                         (auto-detects installed runtimes)
#         CONN=250 bench/rps.sh                (higher concurrency)
#         REQUESTS=1000000 bench/rps.sh        (more requests per runtime)
#         DURATION=60s bench/rps.sh            (hold load for a window instead of a
#                                               fixed count — compare against the
#                                               burst number to see degradation)
#         REPS=5 bench/rps.sh                  (more samples per runtime; best wins)
#         SERVER=scripts/hono.js bench/rps.sh  (serve through the Hono framework;
#                                               run `bun install` in bench/ first)
#         PIN=0 bench/rps.sh                   (no CPU pinning)
#         SERVER_CPUS=0-3 LOAD_CPUS=4-11 bench/rps.sh   (choose the split)
set -uo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-../target/release/esrun}"
SERVER="${SERVER:-scripts/helloserver.js}"  # the hello-world server to run
# The port is chosen per run, not fixed: the OS hands out a free one and the
# server scripts read it from BENCH_PORT. A fixed :3000 collided with an
# unrelated dev server once and every runtime was silently load-tested against
# *it* instead — plausible-looking numbers, all four within 4% of each other.
# Override with PORT=... if you need a known port.
pick_free_port() {
  python3 -c 'import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}
PORT="${PORT:-$(pick_free_port)}"
CONN="${CONN:-100}"
REQUESTS="${REQUESTS:-500000}"
REPS="${REPS:-3}"

# The key this run publishes under. Derived from $SERVER, never assumed: this
# was hardcoded to "hono", so running the default helloserver.js wrote
# hello-world numbers into the site's Hono row with nothing to show they were
# not Hono's.
SERVER_KEY="${SERVER_KEY:-$(basename "$SERVER" .js)}"

# CPU isolation. The load generator and the server it is measuring run on the
# same machine; unpinned, oha spawns a worker per core and competes with the
# server for all of them, so past some rate the number describes the client
# rather than the server — the failure mode where every runtime lands within a
# few percent of every other. Splitting the cores gives the server a fixed,
# uncontended budget and keeps the client out of it.
#
# SERVER_CPUS/LOAD_CPUS override the split; PIN=0 disables pinning entirely.
NCPU="$(nproc 2>/dev/null || echo 0)"
SERVER_PIN=""
LOAD_PIN=""
PIN_DESC="none (unpinned — client and server share all cores)"
if [ "${PIN:-1}" != 0 ] && command -v taskset >/dev/null 2>&1 && [ "$NCPU" -ge 4 ]; then
  half=$((NCPU / 2))
  SERVER_CPUS="${SERVER_CPUS:-0-$((half - 1))}"
  LOAD_CPUS="${LOAD_CPUS:-$half-$((NCPU - 1))}"
  SERVER_PIN="taskset -c $SERVER_CPUS"
  LOAD_PIN="taskset -c $LOAD_CPUS"
  PIN_DESC="server on CPUs $SERVER_CPUS, load generator on CPUs $LOAD_CPUS"
fi

# Resolve the load generator: prefer oha, then bombardier (also check the usual
# cargo/go install dirs even if they aren't on PATH). Sets TOOL + LOADER array.
OHA="$(command -v oha 2>/dev/null || true)"; [ -z "$OHA" ] && [ -x "$HOME/.cargo/bin/oha" ] && OHA="$HOME/.cargo/bin/oha"
BOMB="$(command -v bombardier 2>/dev/null || true)"; [ -z "$BOMB" ] && [ -x "$HOME/.local/bin/bombardier" ] && BOMB="$HOME/.local/bin/bombardier"
if [ -z "${TOOL:-}" ]; then
  if [ -n "$OHA" ]; then
    TOOL="oha"
  elif [ -n "$BOMB" ]; then
    TOOL="bombardier"
  else
    echo "rps.sh needs a load generator. Install one:" >&2
    echo "  cargo install oha     # preferred" >&2
    echo "  go install github.com/codesenberg/bombardier@latest" >&2
    exit 1
  fi
fi

# Runtimes, in display order; skipped if not found.
declare -A CMD
ORDER=()
command -v node >/dev/null 2>&1 && { CMD[node]="node"; ORDER+=(node); }
command -v bun  >/dev/null 2>&1 && { CMD[bun]="bun";   ORDER+=(bun);  }
DENO="$(command -v deno 2>/dev/null)"
[ -z "$DENO" ] && for d in "$HOME/.deno/bin/deno" /tmp/deno/bin/deno; do
  [ -x "$d" ] && { DENO="$d"; break; }
done
[ -n "$DENO" ] && { CMD[deno]="$DENO run -A --quiet"; ORDER+=(deno); }
# LLRT (in run.sh's workload bench) is intentionally absent here: it has no
# general HTTP server (it targets Lambda handlers), so there is no hello-world
# server to drive.
if [ -x "$ESRUN" ]; then CMD[esrun]="$ESRUN"; ORDER+=(esrun); else
  echo "esrun not found at $ESRUN — build it: cargo build --release -p es-runtime-cli" >&2; exit 1
fi

SERVER_PID=""
cleanup() { [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null; }
trap cleanup EXIT

URL="http://127.0.0.1:$PORT/"
HDR="Accept-Encoding: identity"
OUT="$(mktemp)"
trap 'cleanup; rm -f "$OUT"' EXIT

# Belt and braces on top of the per-run port: if $PORT is somehow occupied (a
# PORT=... override, or the free port being claimed between pick and bind), stop
# rather than measure a stranger. The silent version of this failure produces
# plausible-looking numbers — our server dies of EADDRINUSE, the port-wait below
# succeeds instantly against the squatter, and every runtime is load-tested
# against that same process, which reads as all runtimes scoring identically.
if (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
  echo "rps.sh: port $PORT is already in use — every runtime would be measured against that process, not its own server." >&2
  if command -v ss >/dev/null 2>&1; then
    echo "  holder: $(ss -tlnp 2>/dev/null | grep ":$PORT" | head -1)" >&2
  fi
  echo "  re-run to pick another port, or free this one if you passed PORT=$PORT." >&2
  exit 1
fi

# Runs the load generator against the already-running server, writes JSON to
# $OUT, then prints "<req/s> <avg-latency-ms>" parsed from it.
# DURATION (e.g. DURATION=60s) switches both tools from a fixed request count to
# a fixed wall-clock window. A burst of $REQUESTS answers "how fast is it when
# fresh"; holding load for a minute answers whether it stays that way once the
# heap has filled and the allocator has been churning — a different question,
# and the one a long-lived server actually poses. The published sections use the
# request-count form; this is for checking degradation by hand against it.
load() {
  if [ "$TOOL" = "oha" ]; then
    if [ -n "${DURATION:-}" ]; then
      $LOAD_PIN "$OHA" -z "$DURATION" -c "$CONN" --no-tui --output-format json -H "$HDR" "$URL" >"$OUT" 2>/dev/null
    else
      $LOAD_PIN "$OHA" -n "$REQUESTS" -c "$CONN" --no-tui --output-format json -H "$HDR" "$URL" >"$OUT" 2>/dev/null
    fi
    python3 -c "
import json
d=json.load(open('$OUT'))['summary']
print(f\"{d['requestsPerSec']:.0f} {d['average']*1000:.2f}\")" 2>/dev/null || echo "ERR ERR"
  else
    if [ -n "${DURATION:-}" ]; then
      $LOAD_PIN "$BOMB" -c "$CONN" -d "$DURATION" -H "$HDR" -o json -p result "$URL" >"$OUT" 2>/dev/null
    else
      $LOAD_PIN "$BOMB" -c "$CONN" -n "$REQUESTS" -H "$HDR" -o json -p result "$URL" >"$OUT" 2>/dev/null
    fi
    python3 -c "
import json
d=json.load(open('$OUT'))['result']
print(f\"{d['rps']['mean']:.0f} {d['latency']['mean']/1000:.2f}\")" 2>/dev/null || echo "ERR ERR"
  fi
}

# Loads the running server $REPS times and prints the best "<req/s> <avg-lat>".
#
# Best, not mean or last: a slower rep is this machine contending with something
# else, not a property of the server, so the maximum is the closest thing to a
# contention-free ceiling. The same convention websocket-chat/run-chat.sh uses,
# and for the same reason.
#
# It also gives the runtime a warmup. A single sample was measuring whichever
# JIT tier the run happened to land in, which is why this number moved by tens
# of percent between runs while run.sh's (min of N, after a warmup) did not.
# Also reports the spread (worst-to-best, %) rather than discarding it. A best-of
# with a wide spread is not a ceiling, it is a lucky draw, and the caller cannot
# tell which from the winning number alone. Prints "<best-rps> <avg-lat> <spread%>".
load_best() {
  local best_rps=0 best_avg=0 worst_rps=0 rps avg spread
  for _ in $(seq "$REPS"); do
    read -r rps avg <<<"$(load)"
    case "$rps" in '' | ERR) continue ;; esac
    if awk "BEGIN{exit !($rps > $best_rps)}"; then best_rps="$rps"; best_avg="$avg"; fi
    if [ "$worst_rps" = 0 ] || awk "BEGIN{exit !($rps < $worst_rps)}"; then worst_rps="$rps"; fi
  done
  [ "$best_rps" = 0 ] && { echo "ERR ERR ERR"; return; }
  spread=$(awk "BEGIN{printf \"%.1f\", 100*($best_rps-$worst_rps)/$best_rps}")
  echo "$best_rps $best_avg $spread"
}

# Boots one runtime's server, waits for the port, loads it, tears it down.
measure() {
  local cmd="$1"
  BENCH_PORT="$PORT" $SERVER_PIN $cmd "$SERVER" >/dev/null 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 50); do
    (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break
    sleep 0.1
  done
  # The port answering is not proof *our* server answered it: a runtime that
  # failed to bind has already exited, and loading now would measure whatever
  # else is listening. Check the process we started is still alive.
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    SERVER_PID=""
    echo "ERR ERR ERR null"
    return
  fi
  local result
  result="$(load_best)"
  # Peak RSS of the *server* process, read before it is killed. `VmHWM` is the
  # kernel's high-water mark, so one read after the load run covers the whole of
  # it. Measured here rather than reused from run.sh's `http` row: that row runs
  # a client and a server in one process, so its memory is not this server's.
  local peak
  peak="$(awk '/^VmHWM:/{printf "%d", $2/1024}' "/proc/$SERVER_PID/status" 2>/dev/null)"
  [ -z "$peak" ] && peak=null
  kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; SERVER_PID=""
  echo "$result $peak"
}

if [ -n "${BENCH_JSON:-}" ]; then
  declare -A RPS SPREAD PEAK
  for r in "${ORDER[@]}"; do
    read -r rps avg spread peak <<<"$(measure "${CMD[$r]}")"
    case "$rps" in '' | ERR) rps=null; spread=null ;; esac
    case "$peak" in '' | ERR) peak=null ;; esac
    RPS[$r]="$rps"
    SPREAD[$r]="$spread"
    PEAK[$r]="$peak"
  done
  printf '{\n  "results_rps": {\n    "%s": {' "$SERVER_KEY"
  first=1
  for r in "${ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    # A runtime that could not be measured emits null, not a bare ERR token —
    # the latter is invalid JSON and would fail the consumer's parse.
    printf '\n      "%s": %s' "$r" "${RPS[$r]}"
  done
  printf '\n    }\n  },'
  # Peak RSS of each server while it served, so the site can show what a runtime
  # costs to run this workload next to how fast it ran it.
  printf '\n  "results_rps_rss": {'
  printf '\n    "%s": {' "$SERVER_KEY"
  first=1
  for r in "${ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    printf '\n      "%s": %s' "$r" "${PEAK[$r]}"
  done
  printf '\n    }\n  },'
  # How this was measured, and how far the reps spread — published so the site
  # can show the conditions instead of a human retyping them.
  printf '\n  "rps_method": {'
  printf '\n    "%s": {' "$SERVER_KEY"
  printf '\n      "server": "%s",' "$SERVER"
  printf '\n      "tool": "%s",' "$TOOL"
  printf '\n      "connections": %s,' "$CONN"
  printf '\n      "requests": %s,' "$([ -n "${DURATION:-}" ] && echo null || echo "$REQUESTS")"
  printf '\n      "duration": %s,' "$([ -n "${DURATION:-}" ] && printf '"%s"' "$DURATION" || echo null)"
  printf '\n      "reps": %s,' "$REPS"
  printf '\n      "aggregate": "max",'
  printf '\n      "cpu_pinning": "%s",' "$PIN_DESC"
  printf '\n      "spread_pct": {'
  first=1
  for r in "${ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    printf '\n        "%s": %s' "$r" "${SPREAD[$r]}"
  done
  printf '\n      }'
  printf '\n    }'
  printf '\n  }\n}\n'
else
  echo "HTTP requests/sec — hello-world plaintext (\"Hello, World!\")"
  echo "server: $SERVER   (published under the key \"$SERVER_KEY\")"
  echo "load: $TOOL -c $CONN -n $REQUESTS -H \"$HDR\" $URL"
  echo "cpu: $PIN_DESC"
  echo "best of $REPS runs per runtime (the first doubles as a warmup)"
  echo "spread = worst-to-best across the reps; a wide one means the best is a lucky draw"
  echo
  printf "%-7s | %12s | %11s | %8s | %8s\n" "runtime" "req/sec" "avg lat" "spread" "peak rss"
  printf -- "--------+--------------+-------------+----------+----------\n"
  for r in "${ORDER[@]}"; do
    read -r rps avg spread peak <<<"$(measure "${CMD[$r]}")"
    printf "%-7s | %12s | %9s ms | %6s%% | %6s MB\n" "$r" "$rps" "$avg" "$spread" "$peak"
  done
fi
