#!/usr/bin/env bash
# PostgreSQL driver comparison: esrun + @opentf/esrun-postgres against
# postgres.js on Node, Bun and Deno.
#
# This is the acceptance test DECISIONS D56 set for the Postgres path: numeric
# rows should win by a wide margin, text rows should at least tie (both sides
# pay TextDecoder, and only the JS engine can make a JS string), a small query
# must not regress meaningfully, and streaming must not grow memory with the
# result.
#
#   PG_URL=postgres://… bench/db/pg/run.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
export PG_URL="${PG_URL:-postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable}"
esrun_bin="${ESRUN:-$root/target/release/esrun}"

REPS="${REPS:-3}"
WORKLOADS="${WORKLOADS:-scan_num scan_text small stream}"
RUNTIMES="${RUNTIMES:-esrun node bun deno}"

cmd() {
  case "$1" in
    esrun) echo "$esrun_bin --allow-all" ;;
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

[ -f "$root/packages/postgres/dist/index.js" ] || {
  echo "the driver is not built — (cd packages/postgres && bun run build)" >&2; exit 1; }

# Staged rather than imported in place: esrun jails the module loader to the
# project root it detects from the entry file, which here is `bench/`, so a
# reach up into `packages/` is refused. Copying is what crosses that line
# honestly, and it keeps the benchmark measuring the built artifact.
rm -rf "$here/.driver"
cp -r "$root/packages/postgres/dist" "$here/.driver"

# One setup, shared by everyone: the table contents are the workload, not part
# of what is being measured.
"$(cmd esrun)" "$here/esrun.mjs" setup >/dev/null

declare -A WALL RSS OUT
for rt in $RUNTIMES; do
  available "$rt" || { echo "skipping $rt" >&2; continue; }
  for workload in $WORKLOADS; do
    best=""; best_json=""
    for _ in $(seq 1 "$REPS"); do
      # shellcheck disable=SC2086
      json="$(python3 "$here/../measure.py" $(cmd "$rt") "$here/$rt.mjs" "$workload")"
      ok="$(printf '%s' "$json" | python3 -c "import json,sys; print(json.load(sys.stdin)['ok'])")"
      [ "$ok" = "True" ] || { echo "FAILED $rt/$workload: $(printf '%s' "$json" | python3 -c "import json,sys; print(json.load(sys.stdin)['err'])")" >&2; best_json=""; break; }
      wall="$(printf '%s' "$json" | python3 -c "import json,sys; print(json.load(sys.stdin)['wall_ms'])")"
      if [ -z "$best" ] || awk "BEGIN{exit !($wall < $best)}"; then best="$wall"; best_json="$json"; fi
    done
    if [ -n "$best_json" ]; then
      WALL[$rt/$workload]="$best"
      RSS[$rt/$workload]="$(printf '%s' "$best_json" | python3 -c "import json,sys; print(json.load(sys.stdin)['rss_mb'])")"
      OUT[$rt/$workload]="$(printf '%s' "$best_json" | python3 -c "import json,sys; print(json.load(sys.stdin)['out'])")"
    else
      WALL[$rt/$workload]="n/a"
    fi
  done
done

echo
echo "== wall ms (min of $REPS) =="
printf '%-12s' "workload"; for rt in $RUNTIMES; do printf '%10s' "$rt"; done; echo
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do printf '%10s' "${WALL[$rt/$workload]:-n/a}"; done
  echo
done

echo
echo "== peak RSS MB =="
printf '%-12s' "workload"; for rt in $RUNTIMES; do printf '%10s' "$rt"; done; echo
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do printf '%10s' "${RSS[$rt/$workload]:-n/a}"; done
  echo
done

echo
echo "== checksums (must agree) =="
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do printf ' %s=%s' "$rt" "${OUT[$rt/$workload]:-n/a}"; done
  echo
done
