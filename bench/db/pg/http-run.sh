#!/usr/bin/env bash
#
# DB-backed endpoint benchmark: GET /item?id=100000 -> one Postgres point
# lookup answered as JSON, per runtime with its own HTTP server and driver
# (node:http/Bun.serve/Deno.serve/runtime:http; postgres.js everywhere except
# esrun's @opentf/esrun-postgres). Driven by oha like bench/rps.sh: best of
# REPS, spread, peak RSS. The query, the row and the bytes are identical
# everywhere (see http-shared.mjs), so a runtime cannot win by doing less.
#
# Usage:  PG_URL=postgres://postgres:esrun@127.0.0.1:5433/esrun_test bench/db/pg/http-run.sh
#         CONN=100 REQUESTS=200000 REPS=3 bench/db/pg/http-run.sh
#         PG_URL=... node bench/db/pg/http-seed.mjs   (seed once, before)
set -uo pipefail
cd "$(dirname "$0")"

[ -n "${PG_URL:-}" ] || { echo "http-run.sh needs PG_URL" >&2; exit 1; }
ESRUN="${ESRUN:-../../../target/release/esrun}"
CONN="${CONN:-100}"
REQUESTS="${REQUESTS:-200000}"
REPS="${REPS:-3}"

pick_free_port() {
  python3 -c 'import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}
PORT="${PORT:-$(pick_free_port)}"

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

OHA="$(command -v oha 2>/dev/null || true)"; [ -z "$OHA" ] && [ -x "$HOME/.cargo/bin/oha" ] && OHA="$HOME/.cargo/bin/oha"
[ -n "$OHA" ] || { echo "http-run.sh needs oha: cargo install oha" >&2; exit 1; }

declare -A CMD
ORDER=()
command -v node >/dev/null 2>&1 && { CMD[node]="node http-node.mjs"; ORDER+=(node); }
command -v bun >/dev/null 2>&1 && { CMD[bun]="bun http-bun.mjs"; ORDER+=(bun); }
DENO="$(command -v deno 2>/dev/null)"
[ -z "$DENO" ] && for d in "$HOME/.deno/bin/deno" /tmp/deno/bin/deno; do
  [ -x "$d" ] && { DENO="$d"; break; }
done
[ -n "$DENO" ] && { CMD[deno]="$DENO run -A --quiet http-deno.mjs"; ORDER+=(deno); }
if [ -x "$ESRUN" ]; then CMD[esrun]="$ESRUN --allow-all http-esrun.mjs"; ORDER+=(esrun); else
  echo "esrun not found at $ESRUN" >&2; exit 1
fi

SERVER_PID=""
cleanup() { [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null; }
trap cleanup EXIT

URL="http://127.0.0.1:$PORT/item?id=100000"
HDR="Accept-Encoding: identity"
OUT="$(mktemp)"
trap 'cleanup; rm -f "$OUT"' EXIT

if (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
  echo "http-run.sh: port $PORT already in use." >&2; exit 1
fi

load() {
  $LOAD_PIN "$OHA" -n "$REQUESTS" -c "$CONN" --no-tui --output-format json -H "$HDR" "$URL" >"$OUT" 2>/dev/null
  python3 -c "
import json
d=json.load(open('$OUT'))['summary']
print(f\"{d['requestsPerSec']:.0f} {d['average']*1000:.2f}\")" 2>/dev/null || echo "ERR ERR"
}

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

measure() {
  local cmd="$1"
  BENCH_PORT="$PORT" PG_URL="$PG_URL" $SERVER_PIN $cmd >/dev/null 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 50); do
    (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break
    sleep 0.1
  done
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    SERVER_PID=""
    echo "ERR ERR ERR null"
    return
  fi
  local result
  result="$(load_best)"
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
  printf '{\n  "results_rps": {\n    "db_http": {'
  first=1
  for r in "${ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    printf '\n      "%s": %s' "$r" "${RPS[$r]}"
  done
  printf '\n    }\n  },'
  printf '\n  "results_rps_rss": {'
  printf '\n    "db_http": {'
  first=1
  for r in "${ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    printf '\n      "%s": %s' "$r" "${PEAK[$r]}"
  done
  printf '\n    }\n  },'
  printf '\n  "rps_method": {'
  printf '\n    "db_http": {'
  printf '\n      "server": "%s",' "http-<runtime>.mjs (PG point lookup -> JSON)"
  printf '\n      "tool": "oha",'
  printf '\n      "connections": %s,' "$CONN"
  printf '\n      "requests": %s,' "$REQUESTS"
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
  echo "DB-backed endpoint — GET /item -> Postgres point lookup -> JSON"
  echo "load: oha -c $CONN -n $REQUESTS -H \"$HDR\" $URL"
  echo "cpu: $PIN_DESC"
  echo "best of $REPS runs per runtime (the first doubles as a warmup)"
  echo
  printf "%-7s | %12s | %11s | %8s | %8s\n" "runtime" "req/sec" "avg lat" "spread" "peak rss"
  printf -- "--------+--------------+-------------+----------+----------\n"
  for r in "${ORDER[@]}"; do
    read -r rps avg spread peak <<<"$(measure "${CMD[$r]}")"
    printf "%-7s | %12s | %9s ms | %6s%% | %6s MB\n" "$r" "$rps" "$avg" "$spread" "$peak"
  done
fi
