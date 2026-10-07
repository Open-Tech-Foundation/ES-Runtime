#!/usr/bin/env bash
# Image pipeline benchmark: esrun's `runtime:images` vs `Bun.Image`.
#
# Separate from `bench/run.sh` because the APIs differ in shape, so each
# runtime has its own script against one workload table (`workload.mjs`). The
# inputs are generated (`gen.py`, needs Pillow) and every workload prints a
# checksum the runner compares.
#
#   bench/images/run.sh
#   REPS=5 bench/images/run.sh
#   WORKLOADS="jpeg_webp" RUNTIMES="esrun" bench/images/run.sh
#
# Each cell is the minimum of REPS runs, with peak RSS of that run.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
data="$here/.data"
esrun_bin="${ESRUN:-$root/target/release/esrun}"

REPS="${REPS:-3}"
WORKLOADS="${WORKLOADS:-jpeg_webp jpeg_jpeg png_jpeg}"
RUNTIMES="${RUNTIMES:-esrun bun}"

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
if [ ! -f "$data/photo.jpg" ] || [ ! -f "$data/screen.png" ]; then
  mkdir -p "$data"
  python3 -I "$here/gen.py" "$data" || { echo "generating inputs needs Pillow (pip install pillow)" >&2; exit 1; }
fi

cmd() {
  case "$1" in
    esrun) echo "$esrun_bin --allow-read --allow-imports" ;;
    bun)   echo "bun" ;;
  esac
}
available() {
  case "$1" in
    esrun) [ -x "$esrun_bin" ] ;;
    *) command -v "$1" >/dev/null 2>&1 ;;
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
      # shellcheck disable=SC2086
      json="$(python3 "$root/bench/db/measure.py" $(cmd "$rt") "./$rt.mjs" "$workload" "$data")"
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
    printf '%-6s %-10s %8s ms  %6s MB  %s\n' "$rt" "$workload" \
      "${WALL[$rt/$workload]}" "${RSS[$rt/$workload]}" "${OUT[$rt/$workload]}"
  done
done

echo
echo "== wall ms (min of $REPS) / peak RSS MB =="
printf '%-12s' "workload"; for rt in $RUNTIMES; do printf '%20s' "$rt"; done; echo
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do
    printf '%20s' "${WALL[$rt/$workload]:-n/a} / ${RSS[$rt/$workload]:-n/a}"
  done
  echo
done

echo
echo "== checksums (must agree) =="
for workload in $WORKLOADS; do
  printf '%-12s' "$workload"
  for rt in $RUNTIMES; do printf ' %s=%s' "$rt" "${OUT[$rt/$workload]:-n/a}"; done
  echo
done
