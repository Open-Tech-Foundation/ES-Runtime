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
#
# Incremental is the normal way to run it: `workloads` carries the esrun
# version the validator checks, so after a version bump run it first, then the
# other sections one at a time — each publishes on its own, and one that fails
# costs only itself.
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
TMP8="$(mktemp)"
TMP9="$(mktemp)"
TMP10="$(mktemp)"
TMP11="$(mktemp)"
TMP12="$(mktemp)"
TMP_COMBINED="$(mktemp)"
trap 'rm -f "$TMP1" "$TMP2" "$TMP3" "$TMP4" "$TMP5" "$TMP6" "$TMP7" "$TMP8" "$TMP9" "$TMP10" "$TMP11" "$TMP12" "$TMP_COMBINED"' EXIT

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
ALL_SECTIONS="workloads rps rps_sustained rps_static rps_elysia devserver buildtime pg_qps mysql_qps websocket http2 memory_safety"
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

selected() { case " $SECTIONS " in *" $1 "*) return 0 ;; esac; return 1; }

# Everything the selected sections need, checked before any of them runs. Each
# section used to check its own prerequisites when it started, so a missing
# fixture surfaced fifty minutes into a run and took every finished section
# down with it.
preflight() {
  local problems=()
  local esrun="${ESRUN:-../target/release/esrun}" esdev="${ESDEV:-../target/release/esdev}"
  [ -x "$esrun" ] || problems+=("no esrun at $esrun — cargo build --release -p es-runtime-cli")
  if selected rps_elysia || selected devserver || selected buildtime; then
    [ -x "$esdev" ] || problems+=("no esdev at $esdev — cargo build --release -p es-runtime-dev-cli")
  fi
  if selected devserver || selected buildtime; then
    [ -d dev-server/apps/app-10000/node_modules ] ||
      problems+=("the dev-server fixture is missing — (cd dev-server && node gen.mjs 10000 && cd apps/app-10000 && npm install)")
  fi
  if selected pg_qps; then
    [ -n "${PG_URL:-}" ] || problems+=("pg_qps needs PG_URL — see bench/README.md")
    [ -f ../packages/postgres/dist/index.js ] || problems+=("pg_qps needs the postgres driver built — tsr build")
  fi
  if selected mysql_qps; then
    [ -n "${MYSQL_URL:-}" ] || problems+=("mysql_qps needs MYSQL_URL — see bench/README.md")
    [ -f ../packages/mysql/dist/index.js ] || problems+=("mysql_qps needs the mysql driver built — tsr build")
  fi
  if [ ${#problems[@]} -gt 0 ]; then
    echo "not starting — fix these first:" >&2
    printf '  - %s\n' "${problems[@]}" >&2
    exit 1
  fi
}
preflight

# Finished sections are kept, so a run that fails late resumes rather than
# starting again. The cache is keyed on exactly what the numbers depend on —
# the esrun and esdev binaries, every runtime's version, and the row scope — so
# a rebuilt esrun or an upgraded Node invalidates it instead of being mixed
# with numbers it did not produce. It is cleared once a module is published.
# RESUME=0 ignores it.
fingerprint() {
  {
    for bin in "${ESRUN:-../target/release/esrun}" "${ESDEV:-../target/release/esdev}"; do
      [ -f "$bin" ] && sha256sum "$bin" | cut -d" " -f1
    done
    for rt in node bun deno llrt; do command -v "$rt" >/dev/null 2>&1 && "$rt" --version 2>&1 | head -1; done
    echo "rows:$ROW_SCOPE"
  } | sha256sum | cut -c1-16
}
CACHE=".cache/sections/$(fingerprint)"
mkdir -p "$CACHE"

FRAGMENTS=()
run_section() { # name  outfile  command...
  local name="$1" out="$2"; shift 2
  selected "$name" || return 0
  if [ "${RESUME:-1}" != 0 ] && [ -s "$CACHE/$name.json" ]; then
    echo "  section: $name (kept from an earlier run with these binaries)" >&2
    cp "$CACHE/$name.json" "$out"
    FRAGMENTS+=("$out")
    return 0
  fi
  echo "  section: $name" >&2
  local started=$SECONDS
  "$@" > "$out"
  # Fail loudly here rather than writing a truncated module later.
  bun -e 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))' "$out"
  cp "$out" "$CACHE/$name.json"
  echo "  section: $name done in $(( (SECONDS - started) / 60 ))m$(( (SECONDS - started) % 60 ))s" >&2
  FRAGMENTS+=("$out")
}

run_workloads() {
  if [ -n "$ROW_SCOPE" ]; then WORKLOADS="$ROW_SCOPE" BENCH_JSON=1 bash run.sh
  else BENCH_JSON=1 bash run.sh; fi
}
run_rps_hono() { SERVER=scripts/hono.js BENCH_JSON=1 bash rps.sh; }
# The same hello-world shape through Elysia instead of Hono — the framework
# comparison the home page charts. Elysia cannot run on esrun from source (a
# transitive dependency is CommonJS and esrun is ESM-only), so the section
# first bundles scripts/elysia.js with esdev — the documented deployment path —
# and every runtime serves that same bundle: one artifact, one comparison.
run_rps_elysia() {
  ESDEV="${ESDEV:-../target/release/esdev}"
  if [ ! -x "$ESDEV" ]; then
    echo "rps_elysia needs esdev at $ESDEV — build it: cargo build --release -p es-runtime-dev-cli" >&2
    exit 1
  fi
  "$ESDEV" build scripts/elysia.js --out=dist/elysia.bundle.js >&2
  SERVER=dist/elysia.bundle.js SERVER_KEY=elysia BENCH_JSON=1 bash rps.sh
}
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
# Dev-server cold/warm/memory on the generated 10k-component React app —
# vite dev vs oj dev --bundle vs esdev start. See bench/dev-server/run.mjs
# for the legs and bench/README.md for what each number means.
run_devserver() { BENCH_JSON=1 node dev-server/run.mjs 10000; }
run_buildtime() { BENCH_JSON=1 node dev-server/build.mjs 10000; }
# Postgres QPS in Bun's shape (100 rows x 100 in flight). Needs a server:
# PG_URL=postgres://postgres:esrun@127.0.0.1:5433/esrun_test (see the
# DB-backed endpoint section in bench/README.md for the one-time setup).
run_pg_qps() {
  [ -n "${PG_URL:-}" ] || { echo "pg_qps needs PG_URL — see bench/README.md" >&2; exit 1; }
  BENCH_JSON=1 bash db/pg/qps-run.sh
}
# MySQL QPS, the same shape. Needs a server with bench/db/mysql/seed.mjs run:
# MYSQL_URL=mysql://root:esrun@127.0.0.1:3307/esrun_test (see bench/README.md).
run_mysql_qps() {
  [ -n "${MYSQL_URL:-}" ] || { echo "mysql_qps needs MYSQL_URL — see bench/README.md" >&2; exit 1; }
  BENCH_JSON=1 bash db/mysql/qps-run.sh
}
run_websocket() { BENCH_JSON=1 bash websocket-chat/run-chat.sh; }
run_http2() { BENCH_JSON=1 bash http2.sh; }
run_memory_safety() { BENCH_JSON=1 bash memory-safety.sh; }

run_section workloads "$TMP1" run_workloads
run_section rps "$TMP2" run_rps_hono
run_section rps_elysia "$TMP8" run_rps_elysia
run_section rps_sustained "$TMP7" run_rps_sustained
run_section websocket "$TMP3" run_websocket
run_section http2 "$TMP4" run_http2
run_section rps_static "$TMP5" run_rps_static
run_section devserver "$TMP9" run_devserver
run_section buildtime "$TMP10" run_buildtime
run_section pg_qps "$TMP11" run_pg_qps
run_section mysql_qps "$TMP12" run_mysql_qps
run_section memory_safety "$TMP6" run_memory_safety

# Merge onto whatever the module already holds, so unselected sections survive.
# Two levels deep: `results_rps` gains a server key without losing its
# siblings, and a row-keyed matrix gains rows without dropping the rest.
#
# When a whole (non-row-scoped) run.sh ran, its fragment replaces the keys it
# owns instead of merging into them — otherwise a row deleted from the suite
# would live on in the data forever. Which keys those are is read off the
# fragment rather than guessed: the previous version deleted every `results_*`
# key, which swept up `results_http2` — owned by http2.sh, not run.sh — and a
# `SECTIONS=workloads` run therefore destroyed a section it had never measured.
# The validator caught it and refused to publish, which is what it is for.
OWNER_FRAGMENT=""
if [ -z "$ROW_SCOPE" ]; then
  case " $SECTIONS " in *" workloads "*) OWNER_FRAGMENT="$TMP1" ;; esac
fi
bun -e '
  const fs = require("fs");
  const [outPath, existingPath, ownerFragment, ...fragments] = process.argv.slice(1);
  let base = {};
  if (fs.existsSync(existingPath)) {
    const raw = fs.readFileSync(existingPath, "utf8")
      .replace(/^\/\/.*\n/gm, "").replace(/^export default /, "");
    try { base = JSON.parse(raw); } catch { base = {}; }
  }
  // A full run.sh owns exactly the top-level keys it emits, and no others.
  if (ownerFragment) {
    const owned = JSON.parse(fs.readFileSync(ownerFragment, "utf8"));
    for (const k of Object.keys(owned)) delete base[k];
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
' "$TMP_COMBINED" "$OUT" "$OWNER_FRAGMENT" "${FRAGMENTS[@]}"

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
# Published: the kept sections have served their purpose, and the next run
# should measure afresh.
rm -rf "$CACHE"

# The README quotes the same numbers, so it is regenerated from the module that
# was just written rather than kept in step by hand. It had rotted badly when it
# was not: still showing base64 at 71.5ms from before that workload had a Rust
# implementation, and rows the suite no longer has.
node sync-readme-table.mjs
