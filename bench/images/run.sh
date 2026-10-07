#!/usr/bin/env bash
# Image pipeline benchmark, each runtime's own way of making a thumbnail:
# esrun's `runtime:images`, Bun's `Bun.Image`, Deno's `createImageBitmap` +
# `OffscreenCanvas`, and, since Node.js has nothing built in, sharp (libvips),
# the npm package Node users reach for.
#
# Separate from `bench/run.sh` because the APIs differ in shape, so each has
# its own script against one workload table (`workload.mjs`). The inputs are
# generated (`gen.py`, needs Pillow) and every workload prints a checksum the
# runner compares, and the size of one output. sharp comes from
# bench/package.json: `pnpm install`.
#
#   bench/images/run.sh
#   REPS=5 bench/images/run.sh
#   WORKLOADS="jpeg_webp" RUNTIMES="esrun bun" bench/images/run.sh
#   BENCH_JSON=1 bench/images/run.sh      # the site's data (gen-bench-data.sh)
#
# Each cell is the minimum of REPS runs, with peak RSS of that run.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
data="$here/.data"
esrun_bin="${ESRUN:-$root/target/release/esrun}"

REPS="${REPS:-5}"
WORKLOADS="${WORKLOADS:-jpeg_webp jpeg_jpeg png_jpeg jpeg_avif}"
RUNTIMES="${RUNTIMES:-esrun node bun deno}"

# Human-readable progress goes to stderr when the JSON is wanted on stdout.
if [ "${BENCH_JSON:-0}" = 1 ]; then exec 3>&1 1>&2; fi

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
if [ ! -f "$data/photo.jpg" ] || [ ! -f "$data/screen.png" ]; then
  mkdir -p "$data"
  python3 -I "$here/gen.py" "$data" || { echo "generating inputs needs Pillow (pip install pillow)" >&2; exit 1; }
fi

cmd() {
  case "$1" in
    esrun) echo "$esrun_bin --allow-read --allow-imports ./esrun.mjs" ;;
    node)  echo "node ./sharp.mjs" ;;
    bun)   echo "bun ./bun.mjs" ;;
    deno)  echo "deno run --quiet --allow-read ./deno.mjs" ;;
  esac
}
available() {
  case "$1" in
    esrun) [ -x "$esrun_bin" ] ;;
    node) command -v node >/dev/null 2>&1 && [ -d "$root/bench/node_modules/sharp" ] ;;
    *) command -v "$1" >/dev/null 2>&1 ;;
  esac
}
version() {
  case "$1" in
    esrun) "$esrun_bin" --version | head -1 ;;
    node) echo "node $(node --version) + sharp $(node -p 'require(process.argv[1]).version' "$root/bench/node_modules/sharp/package.json")" ;;
    deno) deno --version | head -1 ;;
    bun) echo "bun $(bun --version)" ;;
  esac
}
field() { python3 -c "import json,sys; print(json.load(sys.stdin).get('$1',''))"; }

declare -A WALL RSS OUT SIZE
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
      # A runtime that cannot do a workload says so rather than scoring it.
      if [ "$(printf '%s' "$json" | field out)" = "n/a" ]; then best_json=""; break; fi
      wall="$(printf '%s' "$json" | field wall_ms)"
      if [ -z "$best_wall" ] || awk "BEGIN{exit !($wall < $best_wall)}"; then
        best_wall="$wall"; best_json="$json"
      fi
    done
    [ -n "$best_json" ] || continue
    WALL[$rt/$workload]="$(printf '%s' "$best_json" | field wall_ms)"
    RSS[$rt/$workload]="$(printf '%s' "$best_json" | field rss_mb)"
    out="$(printf '%s' "$best_json" | field out)"
    OUT[$rt/$workload]="${out%% *}"
    SIZE[$rt/$workload]="${out##* }"
    printf '%-6s %-10s %8s ms  %6s MB  %8s B  %s\n' "$rt" "$workload" \
      "${WALL[$rt/$workload]}" "${RSS[$rt/$workload]}" "${SIZE[$rt/$workload]}" "${OUT[$rt/$workload]}"
  done
done

# Checksums must agree: a runtime that produced a different size did different work.
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
printf '%-12s' "workload"; for rt in $RUNTIMES; do printf '%18s' "$rt"; done; echo
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do printf '%18s' "${WALL[$rt/$workload]:-n/a} / ${RSS[$rt/$workload]:-n/a}"; done
  echo
done

if [ "${BENCH_JSON:-0}" = 1 ]; then
  {
    printf '{"results_images":{'
    sep=""
    for workload in $WORKLOADS; do
      printf '%s"%s":{' "$sep" "$workload"; sep=","
      inner=""
      for rt in $RUNTIMES; do
        [ -n "${WALL[$rt/$workload]:-}" ] || continue
        printf '%s"%s":{"ms":%s,"rss_mb":%s,"bytes":%s}' "$inner" "$rt" \
          "${WALL[$rt/$workload]}" "${RSS[$rt/$workload]}" "${SIZE[$rt/$workload]}"
        inner=","
      done
      printf '}'
    done
    printf '},"results_images_versions":{'
    sep=""
    for rt in $RUNTIMES; do
      available "$rt" || continue
      printf '%s"%s":"%s"' "$sep" "$rt" "$(version "$rt")"; sep=","
    done
    printf '},"results_images_method":{"jobs":%s,"reps":%s}}\n' \
      "$(sed -n 's/^export const JOBS = \([0-9]*\);/\1/p' workload.mjs)" "$REPS"
  } >&3
fi
