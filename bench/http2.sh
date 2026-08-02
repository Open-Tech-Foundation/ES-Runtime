#!/usr/bin/env bash
#
# HTTP/1.1 vs HTTP/2 throughput, per runtime.
#
# Same hello-world server, same load generator, same request count — only the
# protocol version changes. That is the point: an HTTP/2 number on its own says
# nothing, because it is dominated by how many connections the client opened and
# how many streams it put on each. So every runtime is measured twice, in two
# shapes that answer two different questions:
#
#   wide   $CONN connections, 1 stream each   — throughput at the concurrency a
#                                               load generator would use anyway;
#                                               HTTP/1.1's best case, since it
#                                               already has $CONN sockets.
#   narrow 1 connection, $PARALLEL streams    — multiplexing. On HTTP/1.1 one
#                                               connection is strictly serial
#                                               (one request in flight, the next
#                                               written only after the response
#                                               is read); on HTTP/2 the same
#                                               socket carries $PARALLEL at once.
#
# The narrow shape is where HTTP/2 is supposed to win by a lot, and the wide
# shape is where it is *not* supposed to — a runtime showing a large win in both
# is usually measuring its own HTTP/1.1 connection handling, not h2.
#
# Cleartext throughout (h2c by prior knowledge), so the numbers are the protocol
# and the server, with no TLS handshake mixed in. Each runtime is measured on its
# *best available* cleartext h2 server, which is not always the same server as
# its h1 column: esrun and Deno detect the version per connection on one server,
# while for Node and Bun h2c lives behind the separate `node:http2` API. A
# runtime with no cleartext h2 server at all reads n/a.
#
# Load generator: `oha` (the only one of the two rps.sh accepts that speaks
# HTTP/2 to a cleartext origin). Install: `cargo install oha`.
#
# Usage:  bench/http2.sh                    (auto-detects installed runtimes)
#         CONN=250 PARALLEL=100 bench/http2.sh
#         REQUESTS=200000 bench/http2.sh
#         BENCH_JSON=1 bench/http2.sh       (machine-readable, for diffing runs)
set -uo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-../target/release/esrun}"
SERVER="${SERVER:-scripts/helloserver.js}"
CONN="${CONN:-50}"          # connections in the "wide" shape
PARALLEL="${PARALLEL:-50}"  # streams on the single connection in the "narrow" shape
REQUESTS="${REQUESTS:-100000}"

pick_free_port() {
  python3 -c 'import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}
PORT="${PORT:-$(pick_free_port)}"

OHA="$(command -v oha 2>/dev/null || true)"
[ -z "$OHA" ] && [ -x "$HOME/.cargo/bin/oha" ] && OHA="$HOME/.cargo/bin/oha"
if [ -z "$OHA" ]; then
  echo "http2.sh needs oha (bombardier cannot drive cleartext HTTP/2):" >&2
  echo "  cargo install oha" >&2
  exit 1
fi

# Runtimes, in display order; skipped if not found. LLRT is absent for the same
# reason as in rps.sh: it has no general HTTP server.
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
OUT="$(mktemp)"
cleanup() { [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null; rm -f "$OUT"; }
trap cleanup EXIT

URL="http://127.0.0.1:$PORT/"
HDR="Accept-Encoding: identity"

# Same guard rps.sh has, for the same reason: a squatter on $PORT would answer
# every runtime's load and read as all of them scoring identically.
if (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
  echo "http2.sh: port $PORT is already in use — every runtime would be measured against that process." >&2
  exit 1
fi

# load <version> <conns> <streams> → "<req/s>", or ERR.
#
# A runtime that cannot serve the version still *answers* — with a connection
# error, a GOAWAY, or an HTTP/1.1 response to a preface it did not understand —
# and oha will happily report a fast, meaningless rate for that. So the success
# rate is checked too: anything under 99% is ERR rather than a number, because a
# half-failing run is not a throughput measurement.
load() {
  local version="$1" conns="$2" streams="$3"
  local args=(-n "$REQUESTS" -c "$conns" --no-tui --output-format json -H "$HDR")
  [ "$version" = "h2" ] && args+=(--http2 -p "$streams")
  "$OHA" "${args[@]}" "$URL" >"$OUT" 2>/dev/null
  python3 -c "
import json, sys
d = json.load(open('$OUT'))
s = d['summary']
ok = sum(v for k, v in d.get('statusCodeDistribution', {}).items() if k.startswith('2'))
if s.get('successRate', 0) < 0.99 or ok == 0:
    print('ERR'); sys.exit()
print(f\"{s['requestsPerSec']:.0f}\")" 2>/dev/null || echo "ERR"
}

# measure <runtime> <version> <conns> <streams> → "<req/s>" or ERR.
# The server is restarted per measurement: Node needs a different one per
# version, and restarting the others too keeps every number equally warm.
measure() {
  local r="$1" version="$2" conns="$3" streams="$4"
  local h2=""; [ "$version" = "h2" ] && h2="1"
  BENCH_PORT="$PORT" BENCH_H2="$h2" ${CMD[$r]} "$SERVER" >/dev/null 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 50); do
    (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break
    sleep 0.1
  done
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    SERVER_PID=""
    echo "ERR"
    return
  fi
  load "$version" "$conns" "$streams"
  kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; SERVER_PID=""
}

# "1.84x" from two rates, or "n/a" when either side could not be measured.
ratio() {
  case "$1$2" in *ERR*) echo "n/a"; return ;; esac
  python3 -c "print(f'{$2 / $1:.2f}x')" 2>/dev/null || echo "n/a"
}

cell() { case "$1" in ERR|'') echo "n/a" ;; *) echo "$1" ;; esac; }

if [ -n "${BENCH_JSON:-}" ]; then
  printf '{\n  "results_http2": {'
  first=1
  for r in "${ORDER[@]}"; do
    wide_h1="$(measure "$r" h1 "$CONN" 1)"
    wide_h2="$(measure "$r" h2 "$CONN" 1)"
    narrow_h1="$(measure "$r" h1 1 1)"
    narrow_h2="$(measure "$r" h2 1 "$PARALLEL")"
    [ -z "$first" ] && printf ','
    first=
    for v in wide_h1 wide_h2 narrow_h1 narrow_h2; do
      case "${!v}" in ERR|'') eval "$v=null" ;; esac
    done
    printf '\n    "%s": { "wide_h1": %s, "wide_h2": %s, "narrow_h1": %s, "narrow_h2": %s }' \
      "$r" "$wide_h1" "$wide_h2" "$narrow_h1" "$narrow_h2"
  done
  printf '\n  }\n}\n'
else
  echo "HTTP/1.1 vs HTTP/2 (h2c, cleartext) — hello-world plaintext"
  echo "server: $SERVER   load: oha -n $REQUESTS   requests are identical; only the version differs"
  echo
  printf "%-7s | %-25s | %-25s\n" "" "wide: $CONN conns × 1 stream" "narrow: 1 conn × $PARALLEL streams"
  printf "%-7s | %9s %9s %5s | %9s %9s %5s\n" \
    "runtime" "HTTP/1.1" "HTTP/2" "gain" "HTTP/1.1" "HTTP/2" "gain"
  printf -- "--------+---------------------------+---------------------------\n"
  for r in "${ORDER[@]}"; do
    wide_h1="$(measure "$r" h1 "$CONN" 1)"
    wide_h2="$(measure "$r" h2 "$CONN" 1)"
    narrow_h1="$(measure "$r" h1 1 1)"
    narrow_h2="$(measure "$r" h2 1 "$PARALLEL")"
    printf "%-7s | %9s %9s %5s | %9s %9s %5s\n" \
      "$r" "$(cell "$wide_h1")" "$(cell "$wide_h2")" "$(ratio "$wide_h1" "$wide_h2")" \
      "$(cell "$narrow_h1")" "$(cell "$narrow_h2")" "$(ratio "$narrow_h1" "$narrow_h2")"
  done
  echo
  echo "req/sec; ratio is HTTP/2 ÷ HTTP/1.1. n/a = that runtime has no cleartext"
  echo "h2 server, or the run did not come back ≥99% successful."
fi
