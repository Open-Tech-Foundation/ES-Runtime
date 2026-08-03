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
#         REQUESTS=200000 REPS=5 bench/http2.sh
#         BENCH_JSON=1 bench/http2.sh       (machine-readable, for diffing runs)
set -uo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-../target/release/esrun}"
SERVER="${SERVER:-scripts/helloserver.js}"
CONN="${CONN:-50}"          # connections in the "wide" shape
PARALLEL="${PARALLEL:-50}"  # streams on the single connection in the "narrow" shape
REQUESTS="${REQUESTS:-100000}"
REPS="${REPS:-3}"        # timed repetitions per cell; one warmup rep runs first

pick_free_port() {
  python3 -c 'import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}
PORT="${PORT:-$(pick_free_port)}"

# Split the cores between the server and the load generator, for the reason
# spelled out in rps.sh: both run on this machine, and an unpinned oha spawns a
# worker per core and contends with the very server it is measuring. It matters
# more here than there — the narrow shape multiplexes onto one connection, so a
# client short of CPU throttles the server without ever looking saturated.
# PIN=0 disables it; SERVER_CPUS/LOAD_CPUS choose the split.
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
  $LOAD_PIN "$OHA" "${args[@]}" "$URL" >"$OUT" 2>/dev/null
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
# The server is started fresh per measurement: Node and Bun need a different one
# per version (their h2c is `node:http2`, not their default server), and starting
# one for everybody keeps every cell equally cold at the same point in its run.
measure() {
  local r="$1" version="$2" conns="$3" streams="$4"
  local h2=""; [ "$version" = "h2" ] && h2="1"
  BENCH_PORT="$PORT" BENCH_H2="$h2" $SERVER_PIN ${CMD[$r]} "$SERVER" >/dev/null 2>&1 &
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

# Sampling, following run.sh's methodology rather than inventing a second one:
#
#   * **Interleaved + shuffled.** Each repetition samples every runtime once,
#     back to back, in a random order — so contention hits all of them inside the
#     same window instead of one runtime being measured minutes after another.
#     Without this, a close call is decided by *when* a runtime happened to run.
#   * **A discarded warmup repetition**, so page cache, JIT and the load
#     generator's own state are not charged to the first runtime in the list.
#   * **Best of N, not mean.** Interference only ever *subtracts* throughput, so
#     the maximum over repetitions is the contention-free ceiling — the same
#     argument by which run.sh takes the minimum of a duration.
CELLS=(wide_h1 wide_h2 narrow_h1 narrow_h2)
declare -A BEST

# The client shape each cell is measured with: version, connections, streams.
cell_args() {
  case "$1" in
    wide_h1)   echo "h1 $CONN 1" ;;
    wide_h2)   echo "h2 $CONN 1" ;;
    narrow_h1) echo "h1 1 1" ;;
    narrow_h2) echo "h2 1 $PARALLEL" ;;
  esac
}

sample_all() {
  local order=("${ORDER[@]}")
  if command -v shuf >/dev/null 2>&1; then
    mapfile -t order < <(printf '%s\n' "${ORDER[@]}" | shuf)
  fi
  local rep="$1" r c args v prev
  for r in "${order[@]}"; do
    for c in "${CELLS[@]}"; do
      read -r args <<<"$(cell_args "$c")"
      # shellcheck disable=SC2086
      v="$(measure "$r" $args)"
      [ "$rep" = "warmup" ] && continue
      case "$v" in ERR|'') continue ;; esac
      prev="${BEST[$r|$c]:-}"
      if [ -z "$prev" ] || [ "$v" -gt "$prev" ]; then BEST[$r|$c]="$v"; fi
    done
  done
}

sample_all warmup
for rep in $(seq "$REPS"); do sample_all "$rep"; done

# "1.84x" from two rates, or "n/a" when either side could not be measured. A
# runtime whose two versions come from two different servers gets a dagger: the
# ratio there is not "what HTTP/2 costs", it also carries the gap between two
# implementations, so it does not mean the same thing as the rows above it.
ratio() {
  local lo="$1" hi="$2" mark="$3"
  case "$lo$hi" in *ERR*|'') echo "n/a"; return ;; esac
  [ -z "$lo" ] || [ -z "$hi" ] && { echo "n/a"; return; }
  python3 -c "print(f'{$hi / $lo:.2f}x$mark')" 2>/dev/null || echo "n/a"
}

cell() { case "$1" in ERR|'') echo "n/a" ;; *) echo "$1" ;; esac; }

# Runtimes whose HTTP/1.1 and HTTP/2 numbers come from two different servers.
declare -A SPLIT_SERVER=([node]=1 [bun]=1)

if [ -n "${BENCH_JSON:-}" ]; then
  printf '{\n  "results_http2": {'
  first=1
  for r in "${ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    for c in "${CELLS[@]}"; do
      eval "$c=\${BEST[\$r|\$c]:-null}"
    done
    printf '\n    "%s": { "wide_h1": %s, "wide_h2": %s, "narrow_h1": %s, "narrow_h2": %s, "split_server": %s }' \
      "$r" "$wide_h1" "$wide_h2" "$narrow_h1" "$narrow_h2" \
      "$([ -n "${SPLIT_SERVER[$r]:-}" ] && echo true || echo false)"
  done
  printf '\n  }\n}\n'
else
  echo "HTTP/1.1 vs HTTP/2 (h2c, cleartext) — hello-world plaintext"
  echo "server: $SERVER   load: oha -n $REQUESTS   best of $REPS interleaved reps (+1 discarded warmup)"
  echo
  printf "%-7s | %-27s | %-27s\n" "" "wide: $CONN conns × 1 stream" "narrow: 1 conn × $PARALLEL streams"
  printf "%-7s | %9s %9s %6s | %9s %9s %6s\n" \
    "runtime" "HTTP/1.1" "HTTP/2" "gain" "HTTP/1.1" "HTTP/2" "gain"
  printf -- "--------+----------------------------+----------------------------\n"
  for r in "${ORDER[@]}"; do
    mark=""; [ -n "${SPLIT_SERVER[$r]:-}" ] && mark="†"
    wide_h1="${BEST[$r|wide_h1]:-}"
    wide_h2="${BEST[$r|wide_h2]:-}"
    narrow_h1="${BEST[$r|narrow_h1]:-}"
    narrow_h2="${BEST[$r|narrow_h2]:-}"
    printf "%-7s | %9s %9s %6s | %9s %9s %6s\n" \
      "$r" "$(cell "$wide_h1")" "$(cell "$wide_h2")" "$(ratio "$wide_h1" "$wide_h2" "$mark")" \
      "$(cell "$narrow_h1")" "$(cell "$narrow_h2")" "$(ratio "$narrow_h1" "$narrow_h2" "$mark")"
  done
  echo
  echo "req/sec. Compare *down* a column freely — that is one client shape against"
  echo "each runtime's best available server. The gain column is HTTP/2 ÷ HTTP/1.1;"
  echo "† marks a runtime whose two versions come from two different servers"
  echo "(node:http2 vs node:http / Bun.serve), so its ratio also carries the gap"
  echo "between two implementations and is not comparable with an unmarked row."
  echo "n/a = no cleartext h2 server, or no rep came back ≥99% successful."
fi
