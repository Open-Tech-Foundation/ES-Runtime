#!/usr/bin/env bash
# Measures what context propagation and diagnostics cost, then rewrites the
# tables in the internals pages between their markers.
#
# Two questions, because they are the two claims those pages make:
#
#   1. What does carrying a context across an `await` cost, here and in the
#      runtimes that ship `AsyncLocalStorage`? Ours is a V8 promise hook, so the
#      honest comparison is against the others' own implementations of the same
#      idea, on the same workload.
#   2. What does *observing* cost — nothing when unsubscribed, and how much when
#      subscribed, when subscribed with `detail`, and when exporting OTLP? The
#      module's central claim is that the first of those is free, and a claim
#      like that is worth a number rather than an assertion.
#
#   bash bench/probe-tracing.sh            # measure and update the docs
#   bash bench/probe-tracing.sh --json     # print the measurements
#   bash bench/probe-tracing.sh --check    # fail if the docs are out of date
#
# Needs node, bun and deno on PATH and a release esrun (cargo build --release).
# A runtime that is missing is reported as "n/a".
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTEXT_DOC="$ROOT/website/app/docs/internals/context/page.mdx"
DIAG_DOC="$ROOT/website/app/docs/internals/diagnostics/page.mdx"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"; kill $(jobs -p) 2>/dev/null' EXIT

ESRUN="$ROOT/target/release/esrun"
have() { command -v "$1" >/dev/null 2>&1; }

# Iterations. Large enough that the loop dominates process startup, small enough
# that the whole probe stays under a minute.
ITERS=200000

# ---------------------------------------------------------------------------
# 1. Context propagation across an await
# ---------------------------------------------------------------------------
#
# The same shape everywhere: a value made current once, then read from inside a
# continuation, N times. `baseline` is the identical loop with no context at all,
# so what is reported is the propagation and not the loop.

cat > "$WORK/ctx-esrun.mjs" <<'JS'
import { createContext } from "runtime:context";
const N = Number(globalThis.__N ?? 200000);
const ctx = createContext({ defaultValue: 0 });
const bench = async (label, body) => {
  for (let i = 0; i < 2000; i++) await body(i);
  const t0 = performance.now();
  for (let i = 0; i < N; i++) await body(i);
  console.log(`${label} ${((performance.now() - t0) * 1e6 / N).toFixed(0)}`);
};
let sink = 0;
await bench("baseline", async () => { await null; sink += 1; });
await bench("context", () => ctx.run(1, async () => { await null; sink += ctx.get(); }));
if (sink < 0) console.log("unreachable");
JS

# The bare loop in a program that never loads the module, which is what a
# program not using contexts actually pays: no promise hook is installed.
cat > "$WORK/ctx-esrun-bare.mjs" <<'JS'
const N = 200000;
const bench = async (label, body) => {
  for (let i = 0; i < 2000; i++) await body(i);
  const t0 = performance.now();
  for (let i = 0; i < N; i++) await body(i);
  console.log(`${label} ${((performance.now() - t0) * 1e6 / N).toFixed(0)}`);
};
let sink = 0;
await bench("unloaded", async () => { await null; sink += 1; });
if (sink < 0) console.log("unreachable");
JS

cat > "$WORK/ctx-node.mjs" <<'JS'
import { AsyncLocalStorage } from "node:async_hooks";
const N = Number(process.env.BENCH_N ?? 200000);
const als = new AsyncLocalStorage();
const bench = async (label, body) => {
  for (let i = 0; i < 2000; i++) await body(i);
  const t0 = performance.now();
  for (let i = 0; i < N; i++) await body(i);
  console.log(`${label} ${((performance.now() - t0) * 1e6 / N).toFixed(0)}`);
};
let sink = 0;
await bench("baseline", async () => { await null; sink += 1; });
await bench("context", () => als.run(1, async () => { await null; sink += als.getStore(); }));
if (sink < 0) console.log("unreachable");
JS

# esrun reads N from a global the CLI cannot set, so it is inlined instead.
sed -i "s/globalThis.__N ?? 200000/$ITERS/" "$WORK/ctx-esrun.mjs"
sed -i "s/const N = 200000;/const N = $ITERS;/" "$WORK/ctx-esrun-bare.mjs"

ns_of() { echo "$1" | awk -v k="$2" '$1 == k { print $2 }'; }

measure_context() {
  local rt="$1" out=""
  case "$rt" in
    esrun) [ -x "$ESRUN" ] && out="$(cd "$WORK" && "$ESRUN" "$WORK/ctx-esrun.mjs" 2>/dev/null)" ;;
    node)  have node && out="$(BENCH_N=$ITERS node "$WORK/ctx-node.mjs" 2>/dev/null)" ;;
    bun)   have bun  && out="$(BENCH_N=$ITERS bun  "$WORK/ctx-node.mjs" 2>/dev/null)" ;;
    deno)  have deno && out="$(BENCH_N=$ITERS deno run -A "$WORK/ctx-node.mjs" 2>/dev/null)" ;;
  esac
  local base ctx
  base="$(ns_of "$out" baseline)"; ctx="$(ns_of "$out" context)"
  if [ -z "$base" ] || [ -z "$ctx" ]; then echo "n/a n/a n/a"; return; fi
  echo "$base $ctx $((ctx - base))"
}

declare -A CTX
for rt in esrun node bun deno; do CTX[$rt]="$(measure_context "$rt")"; done

# esrun's fourth number: the same loop in a program that never imported the
# module, so no promise hook exists. The others have no equivalent — their
# propagation machinery is in the binary either way.
UNLOADED=""
[ -x "$ESRUN" ] && UNLOADED="$(cd "$WORK" && "$ESRUN" "$WORK/ctx-esrun-bare.mjs" 2>/dev/null | awk '$1=="unloaded"{print $2}')"

# ---------------------------------------------------------------------------
# 2. What observing costs
# ---------------------------------------------------------------------------
#
# One op in a loop, under four conditions. The op is `performance.now()`: it does
# no I/O, so what is measured is the recording path and not a syscall.

cat > "$WORK/diag.mjs" <<'JS'
const N = 200000;
const sub = globalThis.__BENCH_SUBSCRIBE === "1"
  ? (await import("runtime:diagnostics")).subscribe({ bufferSize: 64 }, () => {})
  : null;
for (let i = 0; i < 5000; i++) performance.now();
const t0 = performance.now();
for (let i = 0; i < N; i++) performance.now();
const t1 = performance.now();
console.log(`op ${((t1 - t0) * 1e6 / N).toFixed(0)}`);
if (sub !== null) sub.close();
JS
sed -i "s/const N = 200000;/const N = $ITERS;/" "$WORK/diag.mjs"

# The subscribing variants need the flag *and* the global; the global is set by
# a wrapper module so the benchmarked file stays identical across conditions.
cat > "$WORK/diag-on.mjs" <<'JS'
globalThis.__BENCH_SUBSCRIBE = "1";
await import("./diag.mjs");
JS

measure_diag() {
  local out=""
  [ -x "$ESRUN" ] || { echo "n/a"; return; }
  out="$(cd "$WORK" && "$ESRUN" "$@" 2>/dev/null)"
  ns_of "$out" op
}

OFF="$(measure_diag "$WORK/diag.mjs")"
OBSERVE="$(measure_diag --allow-diagnostics --allow-imports "$WORK/diag-on.mjs")"
DETAIL="$(measure_diag --allow-diagnostics-detail --allow-imports "$WORK/diag-on.mjs")"
# Exporting with no collector listening: the encode and the hand-off are on the
# loop, the delivery is not, which is exactly what the cost of `--otel` is.
OTEL="$(measure_diag --otel=http://127.0.0.1:1 "$WORK/diag.mjs")"

version() {
  case "$1" in
    esrun) [ -x "$ESRUN" ] && "$ESRUN" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo n/a ;;
    bun) have bun && bun --revision 2>/dev/null || echo n/a ;;
    *) have "$1" && "$1" --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo n/a ;;
  esac
}

col() { local v; v="$(echo "${CTX[$1]}" | cut -d' ' -f"$2")"; [ "$v" = "n/a" ] && echo "n/a" || echo "${v}ns"; }

STAMP="<sub>esrun $(version esrun) · Node $(version node) · Bun $(version bun) · Deno $(version deno) · $(uname -s) · $(date -u +%Y-%m-%d) · ${ITERS} iterations</sub>"

CONTEXT_TABLE="$(
  echo "| | esrun | Node.js | Bun | Deno |"
  echo "| --- | --- | --- | --- | --- |"
  echo "| \`await\`, module never imported | ${UNLOADED:-n/a}ns | — | — | — |"
  echo "| Bare \`await\` | $(col esrun 1) | $(col node 1) | $(col bun 1) | $(col deno 1) |"
  echo "| \`await\` inside a context scope | $(col esrun 2) | $(col node 2) | $(col bun 2) | $(col deno 2) |"
  echo "| **Cost of propagation** | $(col esrun 3) | $(col node 3) | $(col bun 3) | $(col deno 3) |"
  echo
  echo "$STAMP"
)"

fmt() { [ -z "$1" ] && echo "n/a" || echo "${1}ns"; }
DIAG_TABLE="$(
  echo "| One op, under | Cost |"
  echo "| --- | --- |"
  echo "| nothing subscribed | $(fmt "$OFF") |"
  echo "| a subscription (\`diagnostics\`) | $(fmt "$OBSERVE") |"
  echo "| a subscription (\`diagnostics-detail\`) | $(fmt "$DETAIL") |"
  echo "| \`--otel\` exporting | $(fmt "$OTEL") |"
  echo
  echo "$STAMP"
)"

if [ "${1:-}" = "--json" ]; then
  for rt in esrun node bun deno; do echo "context $rt baseline/scope/delta=${CTX[$rt]}"; done
  echo "diagnostics off=$OFF observe=$OBSERVE detail=$DETAIL otel=$OTEL"
  exit 0
fi

splice() {
  awk -v table="$2" -v mark="$3" '
    $0 ~ ("BEGIN " mark) { print; print table; skip = 1; next }
    $0 ~ ("END " mark)   { skip = 0 }
    !skip { print }
  ' "$1"
}

NEW_CONTEXT="$(splice "$CONTEXT_DOC" "$CONTEXT_TABLE" "probe:context")"
NEW_DIAG="$(splice "$DIAG_DOC" "$DIAG_TABLE" "probe:diagnostics")"

# `--check` compares the tables with every digit masked. A timing benchmark never
# reproduces its own numbers exactly, so demanding equality would fail on noise
# and teach everyone to ignore it. What it can honestly catch is a table that was
# never generated, one that was hand-edited into a different shape, or a row that
# appeared or disappeared — which is what the rule about committed scripts is
# actually for.
shape() { tr '0-9' '#' ; }
if [ "${1:-}" = "--check" ]; then
  status=0
  [ "$(printf '%s' "$NEW_CONTEXT" | shape)" = "$(shape < "$CONTEXT_DOC")" ] ||
    { echo "$CONTEXT_DOC does not match what this script generates" >&2; status=1; }
  [ "$(printf '%s' "$NEW_DIAG" | shape)" = "$(shape < "$DIAG_DOC")" ] ||
    { echo "$DIAG_DOC does not match what this script generates" >&2; status=1; }
  if [ "$status" = 0 ]; then
    echo "the internals pages have the tables this script writes" >&2
  else
    echo "run: bash bench/probe-tracing.sh" >&2
  fi
  exit "$status"
fi

printf '%s\n' "$NEW_CONTEXT" > "$CONTEXT_DOC"
printf '%s\n' "$NEW_DIAG" > "$DIAG_DOC"
echo "updated $CONTEXT_DOC and $DIAG_DOC" >&2
