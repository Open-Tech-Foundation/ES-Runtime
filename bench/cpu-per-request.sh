#!/usr/bin/env bash
# CPU time each runtime's server spends per request, split into user and
# kernel time, under a fixed request count.
#
# req/s says how fast a server answered; this says what each answer cost the
# server, independent of how long it waited for a core. The split separates the
# two places a server can be slow: kernel time (syscalls, the TCP stack) is
# much the same for every runtime serving the same bytes, so a gap in user time
# is the runtime's own code — its HTTP stack, its JS boundary and the handler.
#
# Usage:  bench/cpu-per-request.sh
#         RUNTIMES="esrun deno" REQUESTS=500000 bench/cpu-per-request.sh
#         SERVER=scripts/hono.js bench/cpu-per-request.sh
#         REQUEST_HEADERS=15 BENCH_READ_HEADER=x-h7 bench/cpu-per-request.sh
#         BENCH_RESPONSE_HEADERS=10 bench/cpu-per-request.sh
#         RUNTIMES="hyper hyper-h1 provider esrun" bench/cpu-per-request.sh
#
# REQUEST_HEADERS=<n> sends n extra request headers (x-h1 … x-hn).
# helloserver.js reads BENCH_READ_HEADER and BENCH_RESPONSE_HEADERS from the
# environment (see the script); they pass straight through.
#
# The server is pinned to one CPU and oha to the upper half of the machine, so
# the server's time is its own. Times come from /proc/<pid>/stat (utime, stime),
# read before and after the measured run; a warmup run is not counted.
set -euo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-$(cd .. && pwd)/target/release/esrun}"
SERVER="${SERVER:-scripts/helloserver.js}"
REQUESTS="${REQUESTS:-200000}"
CONN="${CONN:-100}"
PORT="${PORT:-3941}"
NCPU="$(nproc)"
SERVER_CPU="${SERVER_CPU:-0}"
LOAD_CPUS="${LOAD_CPUS:-$((NCPU / 2))-$((NCPU - 1))}"

command -v oha >/dev/null || { echo "needs oha: cargo install oha" >&2; exit 1; }

declare -A CMD
CMD[esrun]="$ESRUN --allow-all"
CMD[deno]="deno run -A --quiet"
CMD[bun]="bun"
CMD[node]="node"
# The layers under esrun's server, with no JavaScript (rows only when asked
# for): bare hyper, HTTP/1-only hyper, and esrun's provider answered from Rust.
# Build: cargo build --release -p es-runtime-default-providers --example http_floor
FLOOR="${FLOOR:-$(cd .. && pwd)/target/release/examples/http_floor}"
CMD[hyper]="$FLOOR hyper"
CMD[hyper-h1]="$FLOOR hyper-h1"
CMD[provider]="$FLOOR provider"
RUNTIMES="${RUNTIMES:-esrun deno bun node}"

TICK="$(getconf CLK_TCK)"
HEADERS=(-H "Accept-Encoding: identity")
for i in $(seq 1 "${REQUEST_HEADERS:-0}"); do HEADERS+=(-H "x-h$i: value-number-$i"); done
cpu_ticks() { awk '{print $14, $15}' "/proc/$1/stat"; }

printf '%-8s %10s %12s %12s %12s\n' runtime req/s "user µs/req" "sys µs/req" "total"
for rt in $RUNTIMES; do
  cmd="${CMD[$rt]}"
  bin="${cmd%% *}"
  command -v "$bin" >/dev/null || [ -x "$bin" ] || { echo "$rt: not found" >&2; continue; }
  # esrun runs from the workspace root, as in rps.sh: its sandbox is the
  # working directory, and bench/'s packages are linked from the root.
  if [ "$rt" = esrun ]; then
    (cd .. && BENCH_PORT="$PORT" exec taskset -c "$SERVER_CPU" $cmd "bench/$SERVER") >/dev/null 2>&1 &
  else
    BENCH_PORT="$PORT" taskset -c "$SERVER_CPU" $cmd "$SERVER" >/dev/null 2>&1 &
  fi
  pid=$!
  for _ in $(seq 50); do
    (echo >"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break
    sleep 0.1
  done
  url="http://127.0.0.1:$PORT/"
  taskset -c "$LOAD_CPUS" oha -c "$CONN" -n $((REQUESTS / 4)) --no-tui "${HEADERS[@]}" "$url" >/dev/null
  read -r u1 s1 < <(cpu_ticks "$pid")
  rps="$(taskset -c "$LOAD_CPUS" oha -c "$CONN" -n "$REQUESTS" --no-tui "${HEADERS[@]}" "$url" |
    awk '/Requests\/sec/ {printf "%d", $2}')"
  read -r u2 s2 < <(cpu_ticks "$pid")
  kill "$pid"; wait "$pid" 2>/dev/null || true
  awk -v rt="$rt" -v rps="$rps" -v u=$((u2 - u1)) -v s=$((s2 - s1)) -v t="$TICK" -v n="$REQUESTS" \
    'BEGIN { us = 1e6 / t / n; printf "%-8s %10d %12.2f %12.2f %12.2f\n", rt, rps, u*us, s*us, (u+s)*us }'
  PORT=$((PORT + 1))
done
