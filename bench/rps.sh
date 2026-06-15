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
# Usage:  bench/rps.sh                         (auto-detects installed runtimes)
#         CONN=250 bench/rps.sh                (higher concurrency)
#         REQUESTS=1000000 bench/rps.sh        (more requests per runtime)
#         SERVER=scripts/hono.js bench/rps.sh  (serve through the Hono framework;
#                                               run `bun install` in bench/ first)
set -uo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-../target/release/esrun}"
SERVER="${SERVER:-scripts/helloserver.js}"  # the hello-world server to run
PORT=3000           # the server scripts bind this fixed port
CONN="${CONN:-100}"
REQUESTS="${REQUESTS:-500000}"

# Resolve the load generator: prefer oha, then bombardier (also check the usual
# cargo/go install dirs even if they aren't on PATH). Sets TOOL + LOADER array.
OHA="$(command -v oha 2>/dev/null || true)"; [ -z "$OHA" ] && [ -x "$HOME/.cargo/bin/oha" ] && OHA="$HOME/.cargo/bin/oha"
BOMB="$(command -v bombardier 2>/dev/null || true)"; [ -z "$BOMB" ] && [ -x "$HOME/.local/bin/bombardier" ] && BOMB="$HOME/.local/bin/bombardier"
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

# Runs the load generator against the already-running server, writes JSON to
# $OUT, then prints "<req/s> <avg-latency-ms>" parsed from it.
load() {
  if [ "$TOOL" = "oha" ]; then
    "$OHA" -n "$REQUESTS" -c "$CONN" --no-tui --output-format json -H "$HDR" "$URL" >"$OUT" 2>/dev/null
    python3 -c "
import json
d=json.load(open('$OUT'))['summary']
print(f\"{d['requestsPerSec']:.0f} {d['average']*1000:.2f}\")" 2>/dev/null || echo "ERR ERR"
  else
    "$BOMB" -c "$CONN" -n "$REQUESTS" -H "$HDR" -o json -p result "$URL" >"$OUT" 2>/dev/null
    python3 -c "
import json
d=json.load(open('$OUT'))['result']
print(f\"{d['rps']['mean']:.0f} {d['latency']['mean']/1000:.2f}\")" 2>/dev/null || echo "ERR ERR"
  fi
}

# Boots one runtime's server, waits for the port, loads it, tears it down.
measure() {
  local cmd="$1"
  $cmd "$SERVER" >/dev/null 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 50); do
    (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break
    sleep 0.1
  done
  load
  kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; SERVER_PID=""
}

echo "HTTP requests/sec — hello-world plaintext (\"Hello, World!\")"
echo "server: $SERVER"
echo "load: $TOOL -c $CONN -n $REQUESTS -H \"$HDR\" $URL"
echo
printf "%-7s | %12s | %11s\n" "runtime" "req/sec" "avg lat"
printf -- "--------+--------------+------------\n"
for r in "${ORDER[@]}"; do
  read -r rps avg <<<"$(measure "${CMD[$r]}")"
  printf "%-7s | %12s | %9s ms\n" "$r" "$rps" "$avg"
done
