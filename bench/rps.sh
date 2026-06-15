#!/usr/bin/env bash
#
# HTTP requests/sec benchmark: a hello-world server per runtime, driven by an
# external load generator (autocannon) — the classic "req/s" plaintext shape
# (à la the Bun/TechEmpower charts). This is the *right* way to measure server
# throughput: a separate client hammers the server over a real socket, so the
# number reflects the server alone (unlike bench/run.sh's in-process `http`
# workload, where the same single thread runs both the client fetch and the
# server). Each runtime runs scripts/helloserver.js with its own native server.
#
# Needs `autocannon` (used via `bunx autocannon`, or a global install). If
# neither is available the script explains and exits.
#
# Usage:  bench/rps.sh                        (auto-detects installed runtimes)
#         CONN=250 PIPELINE=20 bench/rps.sh   (higher load / HTTP pipelining)
#         DURATION=10 bench/rps.sh
#         SERVER=scripts/hono.js bench/rps.sh (serve through the Hono framework;
#                                              run `bun add hono @hono/node-server` first)
set -uo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-../target/release/esrun}"
SERVER="${SERVER:-scripts/helloserver.js}"  # the hello-world server to run
PORT=3000   # the server scripts bind this fixed port
CONN="${CONN:-100}"
PIPELINE="${PIPELINE:-1}"
DURATION="${DURATION:-10}"

# Resolve autocannon: a global binary, else `bunx autocannon`.
if command -v autocannon >/dev/null 2>&1; then
  AC=(autocannon)
elif command -v bunx >/dev/null 2>&1; then
  AC=(bunx autocannon)
elif command -v npx >/dev/null 2>&1; then
  AC=(npx --yes autocannon)
else
  echo "rps.sh needs autocannon (install it, or have bunx/npx available)." >&2
  exit 1
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
if [ -x "$ESRUN" ]; then CMD[esrun]="$ESRUN"; ORDER+=(esrun); else
  echo "esrun not found at $ESRUN — build it: cargo build --release -p es-runtime-cli" >&2; exit 1
fi

SERVER_PID=""
cleanup() { [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null; }
trap cleanup EXIT

# Pulls req/s + latency out of autocannon's JSON for one runtime.
measure() {
  local cmd="$1" j
  $cmd "$SERVER" >/dev/null 2>&1 &
  SERVER_PID=$!
  # Wait for the port to accept connections (up to ~5s).
  for _ in $(seq 50); do
    (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break
    sleep 0.1
  done
  j=$("${AC[@]}" -c "$CONN" -p "$PIPELINE" -d "$DURATION" -j "http://127.0.0.1:$PORT/" 2>/dev/null)
  kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; SERVER_PID=""
  python3 -c "
import json,sys
d=json.loads(sys.argv[1])
print(f\"{d['requests']['average']:.0f} {d['latency']['average']} {d['latency']['p99']}\")
" "$j" 2>/dev/null || echo "ERR ERR ERR"
}

echo "HTTP requests/sec — hello-world plaintext (\"Hello, World!\")"
echo "server: $SERVER"
echo "load: autocannon -c $CONN -p $PIPELINE -d ${DURATION}s on 127.0.0.1:$PORT"
echo
printf "%-7s | %12s | %11s | %11s\n" "runtime" "req/sec" "avg lat" "p99 lat"
printf -- "--------+--------------+-------------+------------\n"
for r in "${ORDER[@]}"; do
  read -r rps avg p99 <<<"$(measure "${CMD[$r]}")"
  printf "%-7s | %12s | %9s ms | %8s ms\n" "$r" "$rps" "$avg" "$p99"
done
