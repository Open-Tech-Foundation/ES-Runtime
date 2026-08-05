#!/usr/bin/env bash
#
# Regenerates the site's benchmark data module from a real benchmark run.
#
# Runs bench/run.sh in machine mode (BENCH_JSON=1) and wraps its JSON output as
# an ES module the site imports directly. The numbers are therefore never typed
# by hand — this script is the only way the site data changes.
#
# Usage:  bench/gen-bench-data.sh            (uses auto-detected runtimes)
#         ESRUN=/path/to/esrun bench/gen-bench-data.sh
#         bench/gen-bench-data.sh regex strings   (re-measure rows, merge the rest)
#         SECTIONS=rps_static bench/gen-bench-data.sh   (one section only)
set -euo pipefail
cd "$(dirname "$0")"

OUT="../website/src/benchmarks.js"
TMP1="$(mktemp)"
TMP2="$(mktemp)"
TMP3="$(mktemp)"
TMP4="$(mktemp)"
TMP5="$(mktemp)"
TMP6="$(mktemp)"
TMP7="$(mktemp)"
TMP_COMBINED="$(mktemp)"
trap 'rm -f "$TMP1" "$TMP2" "$TMP3" "$TMP4" "$TMP5" "$TMP6" "$TMP7" "$TMP_COMBINED"' EXIT

# Scoped or full, one code path.
#
# The data module is fed by five independent scripts, and re-running all of
# them to change one is most of an hour. SECTIONS picks which actually run;
# every section left out keeps the values already in the module, so a targeted
# regeneration is a normal thing to do rather than an all-or-nothing event.
#
#   SECTIONS=rps_static bench/gen-bench-data.sh
#   SECTIONS="workloads memory_safety" bench/gen-bench-data.sh
#
# `workloads` is bench/run.sh and owns every charted row; the others own one
# section each. Note the row-level workload update is the argument form above
# (`gen-bench-data.sh regex strings`), which is cheaper still.
ALL_SECTIONS="workloads rps rps_sustained rps_static websocket http2 memory_safety"
# Row names as arguments scope the `workloads` section to those rows. They used
# to be a separate mode that could not be combined with anything, so adding a
# row and a section in one pass was impossible: each failed validation waiting
# on the other. Now they are the same mechanism.
ROW_SCOPE="$*"
if [ -n "$ROW_SCOPE" ]; then
  SECTIONS="${SECTIONS:-workloads}"
  case " $SECTIONS " in
    *" workloads "*) ;;
    *) SECTIONS="workloads $SECTIONS" ;;
  esac
  echo "scoped to rows: $ROW_SCOPE" >&2
fi
SECTIONS="${SECTIONS:-$ALL_SECTIONS}"
for s in $SECTIONS; do
  case " $ALL_SECTIONS " in
    *" $s "*) ;;
    *) echo "unknown section '$s' — try: $ALL_SECTIONS" >&2; exit 2 ;;
  esac
done

FRAGMENTS=()
run_section() { # name  outfile  command...
  local name="$1" out="$2"; shift 2
  case " $SECTIONS " in *" $name "*) ;; *) return 0 ;; esac
  echo "  section: $name" >&2
  "$@" > "$out"
  # Fail loudly here rather than writing a truncated module later.
  bun -e 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))' "$out"
  FRAGMENTS+=("$out")
}

run_workloads() {
  if [ -n "$ROW_SCOPE" ]; then WORKLOADS="$ROW_SCOPE" BENCH_JSON=1 bash run.sh
  else BENCH_JSON=1 bash run.sh; fi
}
run_rps_hono() { SERVER=scripts/hono.js BENCH_JSON=1 bash rps.sh; }
# The same Hono server held under load for a fixed window instead of a fixed
# burst. The burst above answers "how fast when fresh"; this answers whether it
# is still that fast once the heap has filled and the collector has been running
# for a while — the question a long-lived server actually poses. Published under
# its own key so the site can put the two side by side.
run_rps_sustained() {
  SERVER=scripts/hono.js SERVER_KEY=hono_sustained \
    DURATION="${SUSTAIN_DURATION:-60s}" REPS="${SUSTAIN_REPS:-2}" \
    BENCH_JSON=1 bash rps.sh
}
# Static-file serving, driven by the same external load generator. Not a row in
# run.sh on purpose: its in-process `http` workload measures the server and the
# client together, which is the thing rps.sh exists to avoid.
run_rps_static() { SERVER=scripts/staticserver.js BENCH_JSON=1 bash rps.sh; }
run_websocket() { BENCH_JSON=1 bash websocket-chat/run-chat.sh; }
run_http2() { BENCH_JSON=1 bash http2.sh; }
run_memory_safety() { BENCH_JSON=1 bash memory-safety.sh; }

run_section workloads "$TMP1" run_workloads
run_section rps "$TMP2" run_rps_hono
run_section rps_sustained "$TMP7" run_rps_sustained
run_section websocket "$TMP3" run_websocket
run_section http2 "$TMP4" run_http2
run_section rps_static "$TMP5" run_rps_static
run_section memory_safety "$TMP6" run_memory_safety

# Merge onto whatever the module already holds, so unselected sections survive.
# Two levels deep: `results_rps` gains a server key without losing its
# siblings, and a row-keyed matrix gains rows without dropping the rest.
# `workloads` is the exception — it owns every row, so its matrices replace
# rather than merge, or a row deleted from the suite would live on forever.
# Only a *whole* workloads run owns the matrices; a row-scoped one merges.
RAN_WORKLOADS=0
if [ -z "$ROW_SCOPE" ]; then
  case " $SECTIONS " in *" workloads "*) RAN_WORKLOADS=1 ;; esac
fi
bun -e '
  const fs = require("fs");
  const [outPath, existingPath, ranWorkloads, ...fragments] = process.argv.slice(1);
  let base = {};
  if (fs.existsSync(existingPath)) {
    const raw = fs.readFileSync(existingPath, "utf8")
      .replace(/^\/\/.*\n/gm, "").replace(/^export default /, "");
    try { base = JSON.parse(raw); } catch { base = {}; }
  }
    // A full run.sh owns the row matrices outright.
  if (ranWorkloads === "1") {
    for (const k of Object.keys(base)) if (k.startsWith("results_") && k !== "results_rps") delete base[k];
    delete base.status;
  }
  const isPlain = (v) => v && typeof v === "object" && !Array.isArray(v);
  // The row catalogue is emitted whole by every run.sh invocation, scoped or
  // not, so it replaces rather than merges — merged, a row deleted from the
  // suite would keep its label and keep the site asking for it forever.
  const REPLACE = new Set(["rows", "groups"]);
  for (const f of fragments) {
    const frag = JSON.parse(fs.readFileSync(f, "utf8"));
    for (const [k, v] of Object.entries(frag)) {
      if (REPLACE.has(k)) base[k] = v;
      else if (isPlain(v) && isPlain(base[k])) {
        for (const [k2, v2] of Object.entries(v)) {
          if (isPlain(v2) && isPlain(base[k][k2])) Object.assign(base[k][k2], v2);
          else base[k][k2] = v2;
        }
      } else base[k] = v;
    }
  }
  fs.writeFileSync(outPath, JSON.stringify(base, null, 2));
' "$TMP_COMBINED" "$OUT" "$RAN_WORKLOADS" "${FRAGMENTS[@]}"

# Check the merged data against what the site actually reads *before* replacing
# the module. A run that half-failed used to be written out regardless: the
# charts then rendered "n/a" everywhere and the only way back was a human
# editing the generated file, which is the exact thing this pipeline exists to
# prevent. On rejection the previous, known-good module stays in place.
node validate-bench-data.mjs "$TMP_COMBINED" ../website

{
  echo "// AUTO-GENERATED by bench/gen-bench-data.sh from a real bench/run.sh run."
  echo "// Do not edit by hand — regenerate with: bench/gen-bench-data.sh"
  echo "// Validated by bench/validate-bench-data.mjs against the rows the site charts."
  printf 'export default '
  cat "$TMP_COMBINED"
} > "$OUT"

echo "wrote $OUT" >&2

# The README quotes the same numbers, so it is regenerated from the module that
# was just written rather than kept in step by hand. It had rotted badly when it
# was not: still showing base64 at 71.5ms from before that workload had a Rust
# implementation, and rows the suite no longer has.
node sync-readme-table.mjs
