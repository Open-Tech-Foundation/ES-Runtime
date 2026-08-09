#!/usr/bin/env bash
# Cross-runtime SQLite benchmark: esrun vs Node.js vs Bun vs Deno.
#
# Unlike `bench/run.sh`, this cannot run one script on every runtime — the
# SQLite APIs are different shapes (esrun's `runtime:db` is async over an op
# boundary; `node:sqlite` and `bun:sqlite` are synchronous and in-process), so
# each runtime gets its own script against a shared workload definition
# (`workload.js`). Every workload prints a checksum the runner verifies, so a
# runtime cannot look fast by doing less.
#
#   bench/db/run.sh                 # everything
#   REPS=5 bench/db/run.sh          # more repetitions
#   WORKLOADS="insert point" bench/db/run.sh
#   RUNTIMES="esrun node" bench/db/run.sh
#
# Reported per cell: the **minimum** of REPS runs (the repo's convention — a
# minimum is the contention-free sample), plus peak RSS and the user/sys CPU
# split of that run.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
scratch="$here/.scratch"
esrun_bin="${ESRUN:-$root/target/release/esrun}"

REPS="${REPS:-3}"
WORKLOADS="${WORKLOADS:-open insert scan_num scan_text point stream}"
RUNTIMES="${RUNTIMES:-esrun node bun deno}"

command -v python3 >/dev/null || { echo "python3 is required for the measurements" >&2; exit 1; }

# `cmd <runtime>` — how to launch a runtime, minus the script and its arguments.
cmd() {
  case "$1" in
    esrun) echo "$esrun_bin" ;;
    node)  echo "node" ;;
    bun)   echo "bun" ;;
    # Deno is sandboxed by default; the grants are the equivalent of the others
    # running unrestricted, so the comparison is of the database and not of the
    # permission model.
    deno)  echo "deno run --quiet --allow-read --allow-write --allow-ffi --allow-env" ;;
  esac
}

available() {
  case "$1" in
    esrun) [ -x "$esrun_bin" ] ;;
    *) command -v "$1" >/dev/null 2>&1 ;;
  esac
}

# One measured run: prints the JSON from measure.py.
measure() {
  local rt="$1" workload="$2" db="$3"
  # shellcheck disable=SC2086
  python3 "$here/measure.py" $(cmd "$rt") "$here/$rt.mjs" "$workload" "$db"
}

field() { python3 -c "import json,sys; print(json.load(sys.stdin).get('$1',''))"; }

rm -rf "$scratch"; mkdir -p "$scratch"
declare -A WALL RSS USER SYS OUT

for rt in $RUNTIMES; do
  if ! available "$rt"; then
    echo "skipping $rt (not installed)" >&2
    continue
  fi
  for workload in $WORKLOADS; do
    best_wall=""; best_json=""
    for rep in $(seq 1 "$REPS"); do
      db="$scratch/$rt-$workload-$rep.db"
      rm -f "$db" "$db"-* 2>/dev/null || true
      # Reading workloads need a populated database; building it is not timed.
      case "$workload" in
        scan_num|scan_text|point)
          python3 "$here/measure.py" $(cmd "$rt") "$here/$rt.mjs" insert "$db" >/dev/null ;;
        stream)
          python3 "$here/measure.py" $(cmd "$rt") "$here/$rt.mjs" seed_stream "$db" >/dev/null ;;
      esac
      json="$(measure "$rt" "$workload" "$db")"
      ok="$(printf '%s' "$json" | field ok)"
      if [ "$ok" != "True" ]; then
        echo "FAILED $rt/$workload: $(printf '%s' "$json" | field err)" >&2
        best_json=""; break
      fi
      wall="$(printf '%s' "$json" | field wall_ms)"
      if [ -z "$best_wall" ] || awk "BEGIN{exit !($wall < $best_wall)}"; then
        best_wall="$wall"; best_json="$json"
      fi
      rm -f "$db" "$db"-* 2>/dev/null || true
    done
    if [ -n "$best_json" ]; then
      WALL[$rt/$workload]="$(printf '%s' "$best_json" | field wall_ms)"
      RSS[$rt/$workload]="$(printf '%s' "$best_json" | field rss_mb)"
      USER[$rt/$workload]="$(printf '%s' "$best_json" | field user_ms)"
      SYS[$rt/$workload]="$(printf '%s' "$best_json" | field sys_ms)"
      OUT[$rt/$workload]="$(printf '%s' "$best_json" | field out)"
      printf '%-6s %-10s %8s ms  %6s MB  u%-8s s%-8s %s\n' \
        "$rt" "$workload" "${WALL[$rt/$workload]}" "${RSS[$rt/$workload]}" \
        "${USER[$rt/$workload]}" "${SYS[$rt/$workload]}" "${OUT[$rt/$workload]}"
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
echo "== CPU ms of that run (user + sys) =="
printf '%-12s' "workload"; for rt in $RUNTIMES; do printf '%16s' "$rt"; done; echo
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do
    u="${USER[$rt/$workload]:-}"; y="${SYS[$rt/$workload]:-}"
    if [ -n "$u" ]; then printf '%16s' "$(awk "BEGIN{printf \"%.0f+%.0f\", $u, $y}")"; else printf '%16s' "n/a"; fi
  done
  echo
done

echo
echo "== checksums (must agree across runtimes) =="
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do printf ' %s=%s' "$rt" "${OUT[$rt/$workload]:-n/a}"; done
  echo
done

rm -rf "$scratch"
