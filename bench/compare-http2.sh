#!/usr/bin/env bash
#
# esrun against esrun: two or more builds, the same hello-world server, in
# bench/http2.sh's shapes — the A/B a change to the HTTP connection code is
# judged by. Reports, per build and shape, the best req/s and the server's
# user+system CPU per request (from /proc, so the cost of an idle wait is not
# counted as work).
#
#   narrow_h2  1 connection, $PARALLEL streams   multiplexing: the shape where
#                                                write batching shows
#   wide_h2    $CONN connections, 1 stream each
#   wide_h1    $CONN connections, HTTP/1.1
#
# Methodology is http2.sh's: server and load generator on separate halves of
# the cores, every repetition samples each build once in a shuffled order, a
# discarded warmup repetition, best of $REPS.
#
# Usage (from bench/, builds in ../target/release):
#   BINS="esrun-before esrun-after" bench/compare-http2.sh
#   BINS="a b c" REPS=5 REQUESTS=300000 bench/compare-http2.sh
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
: "${BINS:?name the builds to compare: BINS=\"esrun-a esrun-b\" (in ../target/release)}"
BIN_DIR="${BIN_DIR:-../target/release}"
PORT="${PORT:-18940}"
REQUESTS="${REQUESTS:-200000}"
REPS="${REPS:-4}"
CONN="${CONN:-50}"
PARALLEL="${PARALLEL:-50}"
OHA="$(command -v oha 2>/dev/null || echo "$HOME/.cargo/bin/oha")"
[ -x "$OHA" ] || { echo "compare-http2.sh needs oha: cargo install oha" >&2; exit 1; }
for bin in $BINS; do
  [ -x "$BIN_DIR/$bin" ] || { echo "no build at $BIN_DIR/$bin" >&2; exit 1; }
done
if (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
  echo "compare-http2.sh: port $PORT is already in use" >&2
  exit 1
fi

NCPU="$(nproc)"
half=$((NCPU / 2))
SERVER_PIN=(taskset -c "0-$((half - 1))")
LOAD_PIN=(taskset -c "$half-$((NCPU - 1))")
OUT="$(mktemp)"
SERVER=""
trap '[ -n "$SERVER" ] && kill "$SERVER" 2>/dev/null; rm -f "$OUT"' EXIT

# run <build> <version> <connections> <streams> → "<req/s> <µs/req>" or ERR
run() {
  BENCH_PORT="$PORT" "${SERVER_PIN[@]}" "$BIN_DIR/$1" --allow-all scripts/helloserver.js >/dev/null 2>&1 &
  SERVER=$!
  for _ in $(seq 50); do (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break; sleep 0.1; done
  local args=(-n "$REQUESTS" -c "$3" --no-tui --output-format json -H "Accept-Encoding: identity")
  [ "$2" = h2 ] && args+=(--http2 -p "$4")
  local before after
  before="$(awk '{print $14 + $15}' "/proc/$SERVER/stat")"
  env -u NO_COLOR "${LOAD_PIN[@]}" "$OHA" "${args[@]}" "http://127.0.0.1:$PORT/" >"$OUT" 2>/dev/null
  after="$(awk '{print $14 + $15}' "/proc/$SERVER/stat")"
  kill "$SERVER" 2>/dev/null; wait "$SERVER" 2>/dev/null; SERVER=""
  local tick; tick="$(getconf CLK_TCK)"
  python3 - "$OUT" "$before" "$after" "$tick" "$REQUESTS" <<'PY'
import json, sys
out, before, after, tick, n = sys.argv[1], float(sys.argv[2]), float(sys.argv[3]), float(sys.argv[4]), int(sys.argv[5])
d = json.load(open(out)); s = d["summary"]
ok = sum(v for k, v in d.get("statusCodeDistribution", {}).items() if k.startswith("2"))
if s.get("successRate", 0) < 0.99 or ok == 0:
    print("ERR")
else:
    print(f"{s['requestsPerSec']:.0f} {(after - before) / tick * 1e6 / n:.2f}")
PY
}

declare -A BEST CPU
SHAPES=("narrow_h2 h2 1 $PARALLEL" "wide_h2 h2 $CONN 1" "wide_h1 h1 $CONN 1")
for rep in $(seq 0 "$REPS"); do
  for bin in $(printf "%s\n" $BINS | shuf); do
    for shape in "${SHAPES[@]}"; do
      set -- $shape
      result="$(run "$bin" "$2" "$3" "$4")"
      [ "$rep" -eq 0 ] && continue
      read -r rps us <<<"$result"
      [[ "$rps" =~ ^[0-9]+$ ]] || continue
      if [ "$rps" -gt "${BEST[$bin $1]:-0}" ]; then
        BEST[$bin $1]="$rps"
        CPU[$bin $1]="$us"
      fi
    done
  done
done

printf "%-24s %-10s %10s %10s\n" build shape "req/s" "µs/req"
for bin in $BINS; do
  for shape in "${SHAPES[@]}"; do
    set -- $shape
    printf "%-24s %-10s %10s %10s\n" "$bin" "$1" "${BEST[$bin $1]:-ERR}" "${CPU[$bin $1]:-}"
  done
done
