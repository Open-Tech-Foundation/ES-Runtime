#!/usr/bin/env bash
#
# Soak: keep esrun busy for a long time and check that it stays the same size.
# Not a speed benchmark. Each scenario runs for SOAK_MINUTES while its processes
# are sampled every SOAK_SAMPLE seconds — resident memory, open file
# descriptors and threads — and fails when any of them is still climbing at the
# end, when a process dies, or when a scenario reports a wrong answer.
#
#   http      server.js under oha load, with client.js churning fetch bodies and
#             WebSocket connections against it (both processes are watched)
#   workers   start, message and terminate workers, four at a time
#   db        runtime:db SQLite connections opened and closed beside a pool
#   images    runtime:images decode, resize and encode in every output format
#
# Usage:  bench/soak/run.sh                     every scenario, 10 minutes each
#         SOAK_MINUTES=120 bench/soak/run.sh http
#
# Linux only: it reads /proc. Needs a current release esrun (`cargo build
# --release -p es-runtime-cli`) and `oha` for the http scenario. Run it from the
# solid-state checkout, as the benchmarks are, so the db scenario measures the
# runtime rather than the disk.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ESRUN="${ESRUN:-$root/target/release/esrun}"
MINUTES="${SOAK_MINUTES:-10}"
SAMPLE="${SOAK_SAMPLE:-5}"
PORT="${SOAK_PORT:-24310}"
OUT="${SOAK_OUT:-$(mktemp -d)}"

[ -r /proc/self/status ] || { echo "soak: needs /proc (Linux)" >&2; exit 2; }
[ -x "$ESRUN" ] || { echo "soak: no esrun at $ESRUN — cargo build --release -p es-runtime-cli" >&2; exit 2; }

scenarios=("$@")
[ ${#scenarios[@]} -eq 0 ] && scenarios=(http workers db images)

pids=()
cleanup() { [ ${#pids[@]} -gt 0 ] && kill "${pids[@]}" 2>/dev/null; wait 2>/dev/null; }
trap cleanup EXIT

# One CSV row per sample: seconds, then rss_kib,fds,threads per watched pid.
# A pid that has gone away ends the scenario as a failure.
sample() { # csv name pid...
  local csv="$1" name="$2"; shift 2
  local start=$SECONDS end=$((SECONDS + MINUTES * 60))
  local header="t" pid
  for pid in "$@"; do header="$header,${pid}_rss_kib,${pid}_fds,${pid}_threads"; done
  echo "$header" >"$csv"
  while [ $SECONDS -lt $end ]; do
    local row="$((SECONDS - start))"
    for pid in "$@"; do
      if [ ! -r "/proc/$pid/status" ]; then
        echo "  ✗ $name: process $pid exited before the soak ended"
        return 1
      fi
      local rss threads fds
      rss="$(awk '/^VmRSS:/{print $2}' "/proc/$pid/status")"
      threads="$(awk '/^Threads:/{print $2}' "/proc/$pid/status")"
      fds="$(ls "/proc/$pid/fd" 2>/dev/null | wc -l)"
      row="$row,$rss,$fds,$threads"
    done
    echo "$row" >>"$csv"
    sleep "$SAMPLE"
  done
}

# The verdict: drop the first fifth as warm-up, then compare the median of the
# first and last thirds of what is left. Growth past every bound of a column
# fails it — a long-lived process settles, a leaking one does not.
judge() { # csv name
  python3 -I - "$1" "$2" <<'PY'
import csv, statistics, sys
path, name = sys.argv[1], sys.argv[2]
rows = list(csv.DictReader(open(path)))
rows = rows[len(rows) // 5:]
if len(rows) < 6:
    print(f"  ✗ {name}: only {len(rows)} samples after warm-up; raise SOAK_MINUTES")
    sys.exit(1)
third = len(rows) // 3
# (relative, absolute) growth a column may show before it counts as a leak.
bounds = {"rss_kib": (0.20, 16 * 1024), "fds": (0.0, 16), "threads": (0.0, 4)}
failed = False
for column in rows[0]:
    if column == "t":
        continue
    kind = next(k for k in bounds if column.endswith(k))
    first = statistics.median(float(r[column]) for r in rows[:third])
    last = statistics.median(float(r[column]) for r in rows[-third:])
    rel, absolute = bounds[kind]
    grew = last - first
    bad = grew > absolute and grew > rel * first
    failed |= bad
    unit = " KiB" if kind == "rss_kib" else ""
    print(f"  {'✗' if bad else '✓'} {name} {column}: {first:.0f}{unit} → {last:.0f}{unit}")
sys.exit(1 if failed else 0)
PY
}

wait_port() { # port
  for _ in $(seq 100); do
    (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null && return 0
    sleep 0.1
  done
  return 1
}

run_http() {
  command -v oha >/dev/null || { echo "  ✗ http: needs oha (cargo install oha)"; return 1; }
  (cd "$OUT" && SOAK_PORT=$PORT exec "$ESRUN" --allow-listen --allow-env server.js) >"$OUT/http-server.log" 2>&1 &
  local server=$!; pids+=("$server")
  wait_port "$PORT" && wait_port $((PORT + 1)) || { echo "  ✗ http: server did not start"; return 1; }
  # A steady rate rather than flat out: the question is what a busy server does
  # over time, not how fast it can go.
  oha -z "${MINUTES}m" -q 2000 -c 32 --no-tui "http://127.0.0.1:$PORT/" >"$OUT/http-oha.log" 2>&1 &
  pids+=($!)
  (cd "$OUT" && SOAK_PORT=$PORT exec "$ESRUN" --allow-net --allow-env client.js) >"$OUT/http-client.log" 2>&1 &
  local client=$!; pids+=("$client")
  sample "$OUT/http.csv" http "$server" "$client" || return 1
  judge "$OUT/http.csv" http
}

run_script() { # name flags...
  local name="$1"; shift
  (cd "$OUT" && exec "$ESRUN" "$@" "$name.js") >"$OUT/$name.log" 2>&1 &
  local pid=$!; pids+=("$pid")
  sample "$OUT/$name.csv" "$name" "$pid" || return 1
  judge "$OUT/$name.csv" "$name"
}

# A run reaches only its working directory, so the scripts run from a copy in
# the output directory, beside the database file and the logs.
cp "$here"/*.js "$OUT"/
echo "soak: $("$ESRUN" --version), ${MINUTES} min per scenario, logs in $OUT"
status=0
for scenario in "${scenarios[@]}"; do
  echo "• $scenario"
  case "$scenario" in
    http) run_http ;;
    workers) run_script workers --allow-workers --allow-imports ;;
    db) run_script db --allow-read --allow-write ;;
    images) run_script images ;;
    *) echo "  ✗ unknown scenario: $scenario"; false ;;
  esac || status=1
  cleanup; pids=()
done
exit $status
