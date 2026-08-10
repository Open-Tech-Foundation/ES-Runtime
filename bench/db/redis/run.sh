#!/usr/bin/env bash
# Redis client comparison: esrun + @opentf/esrun-redis against each runtime's
# own answer — Bun's built-in client, and ioredis on Node and Deno, which have
# none.
#
# What this measures is the *client*, not Redis. A command's cost is a round
# trip and a decode, and the server's share of that is small — so the four
# workloads separate the two: `serial_*` is round-trip bound, `pipeline` is the
# same work batched, and `list`/`hash` are big enough replies for decoding to
# dominate.
#
#   bench/db/redis/run.sh
#   REPS=5 bench/db/redis/run.sh
#   WORKLOADS="pipeline list" RUNTIMES="esrun bun" bench/db/redis/run.sh
#   BUN_REDIS=ioredis bench/db/redis/run.sh    # Bun on the same library as Node
#
# Reported per cell: the **minimum** of REPS runs (the repo's convention — a
# minimum is the contention-free sample), plus peak RSS. Every workload prints a
# checksum the runner compares across runtimes, so a client cannot look fast by
# doing less.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
export REDIS_BENCH_URL="${REDIS_BENCH_URL:-redis://127.0.0.1:6379}"
esrun_bin="${ESRUN:-$root/target/release/esrun}"

REPS="${REPS:-3}"
WORKLOADS="${WORKLOADS:-serial_set serial_get pipeline list hash}"
RUNTIMES="${RUNTIMES:-esrun node bun deno}"

command -v python3 >/dev/null || { echo "python3 is required for the measurements" >&2; exit 1; }

cmd() {
  case "$1" in
    esrun) echo "$esrun_bin" ;;
    node)  echo "node" ;;
    bun)   echo "bun" ;;
    deno)  echo "deno run --quiet --allow-all" ;;
  esac
}

available() {
  case "$1" in
    esrun) [ -x "$esrun_bin" ] ;;
    *) command -v "$1" >/dev/null 2>&1 ;;
  esac
}

[ -f "$root/packages/redis/dist/index.js" ] || {
  echo "the driver is not built — (cd packages/redis && bun run build)" >&2; exit 1; }

# Staged rather than imported in place, for the same reason the Postgres bench
# stages its driver: esrun jails the module loader to the project root it
# detects from the entry file, which here is `bench/`, so a reach up into
# `packages/` is refused. Copying is what crosses that line honestly.
rm -rf "$here/.driver"
cp -r "$root/packages/redis/dist" "$here/.driver"

# One setup, shared by everyone: the keyspace is the workload, not part of what
# is being measured.
"$(cmd esrun)" "$here/esrun.mjs" setup >/dev/null

field() { python3 -c "import json,sys; print(json.load(sys.stdin).get('$1',''))"; }

declare -A WALL RSS OUT
for rt in $RUNTIMES; do
  available "$rt" || { echo "skipping $rt (not installed)" >&2; continue; }
  for workload in $WORKLOADS; do
    best=""; best_json=""
    for _ in $(seq 1 "$REPS"); do
      # shellcheck disable=SC2086
      json="$(python3 "$here/../measure.py" $(cmd "$rt") "$here/$rt.mjs" "$workload")"
      ok="$(printf '%s' "$json" | field ok)"
      if [ "$ok" != "True" ]; then
        echo "FAILED $rt/$workload: $(printf '%s' "$json" | field err)" >&2
        best_json=""; break
      fi
      wall="$(printf '%s' "$json" | field wall_ms)"
      if [ -z "$best" ] || awk "BEGIN{exit !($wall < $best)}"; then
        best="$wall"; best_json="$json"
      fi
    done
    if [ -n "$best_json" ]; then
      WALL[$rt/$workload]="$(printf '%s' "$best_json" | field wall_ms)"
      RSS[$rt/$workload]="$(printf '%s' "$best_json" | field rss_mb)"
      OUT[$rt/$workload]="$(printf '%s' "$best_json" | field out)"
      printf '%-6s %-11s %9s ms  %6s MB  %s\n' \
        "$rt" "$workload" "${WALL[$rt/$workload]}" "${RSS[$rt/$workload]}" "${OUT[$rt/$workload]}"
    else
      WALL[$rt/$workload]="n/a"
    fi
  done
done

echo
echo "== wall ms (min of $REPS) =="
printf '%-12s' "workload"; for rt in $RUNTIMES; do printf '%12s' "$rt"; done; echo
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do printf '%12s' "${WALL[$rt/$workload]:-n/a}"; done
  echo
done

echo
echo "== peak RSS MB =="
printf '%-12s' "workload"; for rt in $RUNTIMES; do printf '%12s' "$rt"; done; echo
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do printf '%12s' "${RSS[$rt/$workload]:-n/a}"; done
  echo
done

echo
echo "== checksums (must agree across runtimes) =="
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do printf ' %s=%s' "$rt" "${OUT[$rt/$workload]:-n/a}"; done
  echo
done

rm -rf "$here/.driver"
