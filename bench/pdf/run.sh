#!/usr/bin/env bash
# PDF benchmark: text extraction, merge and split. esrun runs `runtime:pdf`;
# Node.js, Bun and Deno have no PDF support built in, so each runs what a
# JavaScript application uses — pdf.js (pdfjs-dist) for text and pdf-lib to
# edit — from bench/package.json (`pnpm install`).
#
# The input is generated (`gen.py`, standard library only) and every workload
# prints a checksum the runner compares: the "invoice" count for text, the
# output's page count for merge and split.
#
#   bench/pdf/run.sh
#   REPS=5 bench/pdf/run.sh
#   WORKLOADS="text" RUNTIMES="esrun node" bench/pdf/run.sh
#   BENCH_JSON=1 bench/pdf/run.sh      # the site's data (gen-bench-data.sh)
#
# Each cell is the minimum of REPS runs, with peak RSS of that run.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
data="$here/.data"
esrun_bin="${ESRUN:-$root/target/release/esrun}"

REPS="${REPS:-5}"
WORKLOADS="${WORKLOADS:-text merge split}"
RUNTIMES="${RUNTIMES:-esrun node bun deno}"

# Human-readable progress goes to stderr when the JSON is wanted on stdout.
if [ "${BENCH_JSON:-0}" = 1 ]; then exec 3>&1 1>&2; fi

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
if [ ! -f "$data/report.pdf" ]; then
  mkdir -p "$data"
  python3 -I "$here/gen.py" "$data"
fi

cmd() {
  case "$1" in
    esrun) echo "$esrun_bin --allow-read --allow-imports ./esrun.mjs" ;;
    node)  echo "node ./node.mjs" ;;
    bun)   echo "bun ./node.mjs" ;;
    deno)  echo "deno run --quiet --allow-read --allow-env --node-modules-dir=manual ./node.mjs" ;;
  esac
}
libraries() { [ -d "$root/bench/node_modules/pdf-lib" ] && [ -d "$root/bench/node_modules/pdfjs-dist" ]; }
available() {
  case "$1" in
    esrun) [ -x "$esrun_bin" ] ;;
    *) command -v "$1" >/dev/null 2>&1 && libraries ;;
  esac
}
package() { node -p 'require(process.argv[1]).version' "$root/bench/node_modules/$1/package.json"; }
version() {
  local libs="pdf-lib $(package pdf-lib) + pdfjs-dist $(package pdfjs-dist)"
  case "$1" in
    esrun) "$esrun_bin" --version | head -1 ;;
    node) echo "node $(node --version) + $libs" ;;
    deno) echo "$(deno --version | head -1) + $libs" ;;
    bun) echo "bun $(bun --version) + $libs" ;;
  esac
}
field() { python3 -c "import json,sys; print(json.load(sys.stdin).get('$1',''))"; }

declare -A WALL RSS OUT
cd "$here"
for rt in $RUNTIMES; do
  available "$rt" || { echo "skipping $rt (not installed)" >&2; continue; }
  for workload in $WORKLOADS; do
    best_wall=""; best_json=""
    for _ in $(seq 1 "$REPS"); do
      # shellcheck disable=SC2046
      json="$(python3 "$root/bench/db/measure.py" $(cmd "$rt") "$workload" "$data")"
      if [ "$(printf '%s' "$json" | field ok)" != "True" ]; then
        echo "FAILED $rt/$workload: $(printf '%s' "$json" | field err)" >&2
        best_json=""; break
      fi
      wall="$(printf '%s' "$json" | field wall_ms)"
      if [ -z "$best_wall" ] || awk "BEGIN{exit !($wall < $best_wall)}"; then
        best_wall="$wall"; best_json="$json"
      fi
    done
    [ -n "$best_json" ] || continue
    WALL[$rt/$workload]="$(printf '%s' "$best_json" | field wall_ms)"
    RSS[$rt/$workload]="$(printf '%s' "$best_json" | field rss_mb)"
    OUT[$rt/$workload]="$(printf '%s' "$best_json" | field out)"
    printf '%-6s %-6s %8s ms  %6s MB  %s\n' "$rt" "$workload" \
      "${WALL[$rt/$workload]}" "${RSS[$rt/$workload]}" "${OUT[$rt/$workload]}"
  done
done

# Checksums must agree: a runtime that counted differently did different work.
status=0
for workload in $WORKLOADS; do
  first=""
  for rt in $RUNTIMES; do
    out="${OUT[$rt/$workload]:-}"
    [ -n "$out" ] || continue
    if [ -z "$first" ]; then first="$out"
    elif [ "$out" != "$first" ]; then
      echo "checksum mismatch on $workload: $rt=$out, expected $first" >&2; status=1
    fi
  done
done
[ "$status" = 0 ] || exit 1

echo
echo "== wall ms (min of $REPS) / peak RSS MB =="
printf '%-8s' "workload"; for rt in $RUNTIMES; do printf '%18s' "$rt"; done; echo
for workload in $WORKLOADS; do
  printf '%-8s' "$workload"
  for rt in $RUNTIMES; do printf '%18s' "${WALL[$rt/$workload]:-n/a} / ${RSS[$rt/$workload]:-n/a}"; done
  echo
done

if [ "${BENCH_JSON:-0}" = 1 ]; then
  {
    printf '{"results_pdf":{'
    sep=""
    for workload in $WORKLOADS; do
      printf '%s"%s":{' "$sep" "$workload"; sep=","
      inner=""
      for rt in $RUNTIMES; do
        [ -n "${WALL[$rt/$workload]:-}" ] || continue
        printf '%s"%s":{"ms":%s,"rss_mb":%s}' "$inner" "$rt" \
          "${WALL[$rt/$workload]}" "${RSS[$rt/$workload]}"
        inner=","
      done
      printf '}'
    done
    printf '},"results_pdf_versions":{'
    sep=""
    for rt in $RUNTIMES; do
      available "$rt" || continue
      printf '%s"%s":"%s"' "$sep" "$rt" "$(version "$rt")"; sep=","
    done
    printf '},"results_pdf_method":{"jobs":%s,"reps":%s}}\n' \
      "$(sed -n 's/^export const JOBS = \([0-9]*\);/\1/p' workload.mjs)" "$REPS"
  } >&3
fi
