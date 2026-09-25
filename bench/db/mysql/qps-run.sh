#!/usr/bin/env bash
#
# MySQL QPS benchmark — the Postgres one's shape (100 rows x 100 queries in flight).
# Each runtime loops 100 concurrent workers over the same 100-row scan and
# reports queries/sec; every response is row-counted and the first is
# checksummed, so a runtime cannot win by doing less. Best of REPS wins,
# with spread and the best rep's peak RSS.
#
# Usage:  MYSQL_URL=mysql://root:esrun@127.0.0.1:3307/esrun_test bench/db/mysql/qps-run.sh
#         QPS_WARMUP=1 REPS=2 bench/db/mysql/qps-run.sh   (iterate)
set -uo pipefail
cd "$(dirname "$0")"

[ -n "${MYSQL_URL:-}" ] || { echo "qps-run.sh needs MYSQL_URL" >&2; exit 1; }
ESRUN="${ESRUN:-../../../target/release/esrun}"

# The built driver, staged beside the script: esrun jails the module loader to
# the project root it detects from the entry file, which here is `bench/`, so a
# reach up into `packages/` is refused. Copying is what crosses that line, and it
# keeps the benchmark measuring the built artifact rather than a stale copy.
[ -f ../../../packages/mysql/dist/index.js ] || { echo "the driver is not built — run tsr build" >&2; exit 1; }
rm -rf .driver && cp -r ../../../packages/mysql/dist .driver
QPS_WARMUP="${QPS_WARMUP:-3}"
QPS_TOTAL=100000
REPS="${REPS:-3}"

declare -A CMD
ORDER=()
command -v node >/dev/null 2>&1 && { CMD[node]="node qps-node.mjs"; ORDER+=(node); }
command -v bun >/dev/null 2>&1 && { CMD[bun]="bun qps-bun.mjs"; ORDER+=(bun); }
DENO="$(command -v deno 2>/dev/null)"
[ -z "$DENO" ] && for d in "$HOME/.deno/bin/deno" /tmp/deno/bin/deno; do
  [ -x "$d" ] && { DENO="$d"; break; }
done
[ -n "$DENO" ] && { CMD[deno]="$DENO run -A --quiet qps-deno.mjs"; ORDER+=(deno); }
if [ -x "$ESRUN" ]; then CMD[esrun]="$ESRUN --allow-all qps-esrun.mjs"; ORDER+=(esrun); else
  echo "esrun not found at $ESRUN" >&2; exit 1
fi

# Runs one rep: stdout JSON has {qps}; peak RSS comes from getrusage, which
# needs no GNU time. Prints "<qps> <peak_mb>" or "ERR ERR".
run_once() {
  QPS_WARMUP="$QPS_WARMUP" MYSQL_URL="$MYSQL_URL" python3 - "$@" <<'EOF'
import json, resource, subprocess, sys
p = subprocess.run(sys.argv[1:], capture_output=True, text=True)
try:
    qps = json.loads(p.stdout.strip().splitlines()[-1])["qps"]
except Exception:
    print("ERR ERR")
    sys.stderr.write(p.stdout[-2000:] + p.stderr[-2000:])
    sys.exit(0)
peak = round(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024)
print(f"{qps} {peak}")
EOF
}

measure() {
  local cmd="$1" best_qps=0 best_peak=0 worst_qps=0 qps peak spread
  for _ in $(seq "$REPS"); do
    # shellcheck disable=SC2086
    read -r qps peak <<<"$(run_once $cmd)"
    case "$qps" in '' | ERR) continue ;; esac
    if awk "BEGIN{exit !($qps > $best_qps)}"; then best_qps="$qps"; best_peak="$peak"; fi
    if [ "$worst_qps" = 0 ] || awk "BEGIN{exit !($qps < $worst_qps)}"; then worst_qps="$qps"; fi
  done
  [ "$best_qps" = 0 ] && { echo "ERR ERR ERR"; return; }
  spread=$(awk "BEGIN{printf \"%.1f\", 100*($best_qps-$worst_qps)/$best_qps}")
  echo "$best_qps $best_peak $spread"
}

if [ -n "${BENCH_JSON:-}" ]; then
  declare -A QPS PEAK SPREAD
  for r in "${ORDER[@]}"; do
    read -r qps peak spread <<<"$(measure "${CMD[$r]}")"
    case "$qps" in '' | ERR) qps=null; spread=null ;; esac
    case "$peak" in '' | ERR) peak=null ;; esac
    QPS[$r]="$qps"
    PEAK[$r]="$peak"
    SPREAD[$r]="$spread"
  done
  printf '{\n  "results_mysql_qps": {\n    "mysql_qps": {'
  first=1
  for r in "${ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    printf '\n      "%s": %s' "$r" "${QPS[$r]}"
  done
  printf '\n    }\n  },'
  printf '\n  "results_mysql_qps_rss": {\n    "mysql_qps": {'
  first=1
  for r in "${ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    printf '\n      "%s": %s' "$r" "${PEAK[$r]}"
  done
  printf '\n    }\n  },'
  printf '\n  "mysql_qps_method": {'
  printf '\n    "mysql_qps": {'
  printf '\n      "query": "%s",' "SELECT a, b, c FROM bench_num WHERE id <= 100 (100 rows x 100 in flight x 100,000)"
  printf '\n      "warmup_s": %s,' "$QPS_WARMUP"
  printf '\n      "queries": %s,' "$QPS_TOTAL"
  printf '\n      "reps": %s,' "$REPS"
  printf '\n      "aggregate": "max",'
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
  echo "MySQL QPS — 100 rows x 100 queries in flight, 100,000 queries (higher is better)"
  echo "warmup ${QPS_WARMUP}s, then 100,000 queries timed, best of $REPS"
  echo
  printf "%-7s | %12s | %8s | %8s\n" "runtime" "queries/s" "spread" "peak rss"
  printf -- "--------+--------------+----------+----------\n"
  for r in "${ORDER[@]}"; do
    read -r qps peak spread <<<"$(measure "${CMD[$r]}")"
    printf "%-7s | %12s | %6s%% | %6s MB\n" "$r" "$qps" "$spread" "$peak"
  done
fi
