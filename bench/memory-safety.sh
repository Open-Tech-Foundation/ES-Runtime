#!/usr/bin/env bash
#
# Memory-safety probe: what each runtime does when a script asks for more than
# the machine can give. Not a speed benchmark — the question is only whether the
# runtime refuses gracefully (a catchable exception, a clean non-zero exit) or
# dies on a signal, taking whatever else the process was doing with it.
#
# Three shapes, all of which a real bug can produce:
#   mem_nested_json    200k-deep nested array, then JSON.stringify — recursion
#                      depth in the serializer.
#   mem_large_string   doubling a string 35 times — past the engine's maximum
#                      string length.
#   mem_promise_leak   10M chained .then() — unbounded microtask/promise growth.
#
# Each script wraps its work in try/catch, so "graceful" means the runtime threw
# something JS could catch (or exited cleanly); "crash" means it took a signal
# — SIGSEGV or SIGABRT — and never gave the guest a chance.
#
# Usage:  bench/memory-safety.sh              (human-readable)
#         BENCH_JSON=1 bench/memory-safety.sh (machine-readable, for the site)
set -uo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-../target/release/esrun}"
TIMEOUT="${MEM_TIMEOUT:-20}"
BENCH_JSON="${BENCH_JSON:-}"

# Same detection as run.sh, so this probe covers exactly the runtimes the rest
# of the suite reports on. The previous version invoked `esrun <path-to-esrun>`
# and looked for LLRT at ../llrt, so neither ever actually ran.
declare -A CMD
ORDER=()
command -v node >/dev/null 2>&1 && { CMD[node]="node"; ORDER+=(node); }
command -v bun >/dev/null 2>&1 && { CMD[bun]="bun"; ORDER+=(bun); }
DENO="$(command -v deno 2>/dev/null)"
[ -z "$DENO" ] && for d in "$HOME/.deno/bin/deno" /tmp/deno/bin/deno; do
  [ -x "$d" ] && { DENO="$d"; break; }
done
[ -n "$DENO" ] && { CMD[deno]="$DENO run -A --quiet"; ORDER+=(deno); }
LLRT="$(command -v llrt 2>/dev/null)"
[ -z "$LLRT" ] && for d in "$HOME/.llrt/bin/llrt" "$HOME/.local/bin/llrt" /tmp/llrt/llrt; do
  [ -x "$d" ] && { LLRT="$d"; break; }
done
[ -n "$LLRT" ] && { CMD[llrt]="$LLRT"; ORDER+=(llrt); }
# --allow-all, matching deno -A above.
[ -x "$ESRUN" ] && { CMD[esrun]="$ESRUN --allow-all"; ORDER+=(esrun); }

SCRIPTS=(mem_nested_json mem_large_string mem_promise_leak)

# One run → one verdict. Signals are the interesting outcome: >128 means the
# process was killed rather than allowed to fail.
probe() { # cmd script
  local code
  timeout "${TIMEOUT}s" $1 "scripts/$2.js" >/dev/null 2>&1
  code=$?
  if [ "$code" -eq 0 ]; then
    echo graceful
  elif [ "$code" -eq 124 ]; then
    echo timeout
  elif [ "$code" -gt 128 ]; then
    # 139 = SIGSEGV, 134 = SIGABRT, 137 = SIGKILL (usually the OOM killer).
    echo "crash:$((code - 128))"
  else
    echo "exit:$code"
  fi
}

declare -A RESULT
for s in "${SCRIPTS[@]}"; do
  for r in "${ORDER[@]}"; do
    RESULT[$s,$r]="$(probe "${CMD[$r]}" "$s")"
  done
done

if [ -n "$BENCH_JSON" ]; then
  printf '{\n  "memory_safety": {'
  firstrow=1
  for s in "${SCRIPTS[@]}"; do
    [ -z "$firstrow" ] && printf ','
    firstrow=
    printf '\n    "%s": {' "$s"
    first=1
    for r in "${ORDER[@]}"; do
      [ -z "$first" ] && printf ','
      first=
      printf '\n      "%s": "%s"' "$r" "${RESULT[$s,$r]}"
    done
    printf '\n    }'
  done
  printf '\n  }\n}\n'
  exit 0
fi

echo "Memory-safety probe (graceful = the runtime let JS catch it; crash = signal)"
echo
printf "%-18s" "scenario"
for r in "${ORDER[@]}"; do printf " | %10s" "$r"; done
printf "\n"
printf -- "------------------"
for _ in "${ORDER[@]}"; do printf -- "+-----------"; done
printf "\n"
for s in "${SCRIPTS[@]}"; do
  printf "%-18s" "${s#mem_}"
  for r in "${ORDER[@]}"; do printf " | %10s" "${RESULT[$s,$r]}"; done
  printf "\n"
done
