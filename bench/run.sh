#!/usr/bin/env bash
#
# Cross-runtime benchmark: esrun vs Node.js vs Bun vs Deno vs LLRT.
#
# Methodology (see bench/README.md for the rationale and sources):
#  * All workloads use only Web APIs common to every runtime, so the same script
#    runs unmodified on each.
#  * Each script does an untimed in-process warmup (JIT steady state) and times
#    itself with performance.now(), printing RESULT_MS — isolating engine cost
#    from process launch. Startup/bigscript instead measure process wall-time
#    (the launch + parse cost is the metric).
#  * INTERLEAVED + RANDOMIZED: every repetition samples each runtime once per
#    row back-to-back, with the runtime order shuffled, so all candidates share
#    the same contention window. Relative ranking then reflects real differences
#    — not which minute a runtime happened to be measured in.
#  * PROCESS WARMUP: the first repetition is discarded (fills caches, lets the
#    JIT/OS settle) on top of each script's in-process warmup.
#  * AGGREGATION = MIN over repetitions: interference only ever *adds* time, so
#    the minimum is the contention-free floor — the stable, fair comparator.
#  * ADAPTIVE STOP: WORKLOAD_RUNS is a ceiling, not a quota. A row stops being
#    sampled once every live cell in it is within NOISE_THRESHOLD% (never before
#    MIN_REPS), because further repetitions cannot move a minimum that has
#    already settled. Checked per row, so all runtimes keep an equal sample count.
#  * NOISE is disclosed, not hidden: the coefficient of variation per cell is
#    computed; cells above NOISE_THRESHOLD% are flagged (`~`) and listed, and
#    BENCH_JSON publishes cov + sample count alongside every number.
#  * THE FLOOR IS CORROBORATED: since the published number is a minimum, cov is
#    the wrong thing to judge it by — one stall sends cov past 100% without
#    moving the minimum at all. BENCH_JSON also publishes results_floor_gap, how
#    far the second-lowest sample sits above the lowest. Two samples that agree
#    make the floor a measurement; a low sample nothing else comes near makes it
#    a fluke, and that is what blocks publication.
#  * A FAILED CELL IS RETRIED before it is written off, and a timeout is recorded
#    as distinct from an unsupported API — they reach the site as the same null.
#  * Peak RSS is sampled per runtime for RSS_ROWS (GNU time, skipped if absent).
#
# The fetch workload runs against a local HTTP server on 127.0.0.1:18923
# (started here with Node; the workload is skipped if Node is missing or the
# port is taken).
#
# Usage:  bench/run.sh                      (auto-detects installed runtimes)
#         bench/run.sh --list                     (groups, and any unclaimed row)
#         GROUP="fs net" bench/run.sh             (scoped run — one or more groups)
#         WORKLOADS="url encoding" bench/run.sh   (scoped run — explicit rows)
#         ESRUN=/path/to/esrun bench/run.sh
#         WORKLOAD_RUNS=15 bench/run.sh           (more samples per workload)
#         QUIET=1 bench/run.sh                    (pin CPU + disable ASLR, etc.)
#         BENCH_JSON=1 bench/run.sh > results.json (machine-readable output)
#
# Scoping is the normal way to work: the full suite is long, a group is a couple
# of minutes. WORKLOADS wins over GROUP when both are set. (The variable is GROUP,
# not GROUPS: bash reserves GROUPS for the caller's group IDs, so a GROUPS=fs in
# the environment silently never arrives.)
set -uo pipefail
cd "$(dirname "$0")"

ESRUN="${ESRUN:-../target/release/esrun}"
STARTUP_RUNS="${STARTUP_RUNS:-15}"
WORKLOAD_RUNS="${WORKLOAD_RUNS:-5}"
# Timed repetitions every row gets before the stability check can end it early.
MIN_REPS="${MIN_REPS:-3}"
# Coefficient-of-variation (%) above which a measured cell is flagged as noisy —
# and, below which, a row is considered settled enough to stop sampling.
NOISE_THRESHOLD="${NOISE_THRESHOLD:-5}"
# Rows to sample peak RSS for. Default: every selected row, so each workload
# reports what it cost in memory as well as in time — "how much RAM does this
# runtime need to do this" is half the question a reader is asking, and for a
# server it is often the half that decides.
#
# This was once narrowed to three rows because peak RSS needs its own extra
# launch per runtime and the numbers went unread. They went unread because they
# were not published; a matrix of nulls is not evidence that nobody wanted them.
# The cost is real — one more launch per row per runtime, ~25% more launches —
# so `RSS_ROWS="startup rss_load"` is still the way to skip it while iterating.
RSS_ROWS="${RSS_ROWS-}"
# A timed cell below this many ms is measuring the clock, not the runtime.
MIN_MS="${MIN_MS:-5}"
# Rows whose metric is process wall-time (launch + parse), measured externally
# rather than self-timed. Each is generated or fixed rather than a scripts/*.js
# workload; see the gen_* functions below.
LAUNCH_ROWS="startup bigscript modules"

# --- rows and groups --------------------------------------------------------
#
# One table defines every row: which group it belongs to, what unit it reports,
# where it is shown, and what to call it. Groups are named subsets, so a run can
# be scoped to the area being worked on instead of the whole suite
# (`GROUP=fs bench/run.sh` is a two-minute loop; the full suite is not), and they
# are also the vocabulary the site's sections use — a row moving between groups
# moves on the page too.
#
# The label and unit live here rather than in the site because the site should
# not be describing measurements it did not take. BENCH_JSON publishes this table
# as `rows` + `groups`, and every component renders from that: the benchmarks
# page asks for a group, the home-page roller asks for the `card` rows. Adding a
# row here is the whole of adding it to the site.
#
#   group | key | unit | display | label
#
# display:  card   — charted on the benchmarks page and carded on the home page
#           chart  — charted on the benchmarks page only
#           hidden — measured, shown nowhere (feeds another row)
#
# Every row must appear exactly once — `bench/run.sh --list` checks that against
# scripts/, and is the fastest way to catch a row wired up nowhere.
ROW_DEFS=(
  "launch|startup|ms|card|Cold start (near-empty script)"
  "launch|bigscript|ms|card|Parse + run ~100 KB script"
  "launch|modules|ms|card|Load a 300-module graph"

  "memory|rss_load|ms|hidden|Retain a live working set"

  "engine|compute|ms|card|Tight compute loop"
  "engine|json|ms|card|JSON parse/stringify"
  "engine|jsonbig|ms|card|JSON (large documents)"
  "engine|regex|ms|card|Regex (match, validate, replace)"
  "engine|strings|ms|card|String building and slicing"
  "engine|structured|ms|card|structuredClone"
  "engine|errors|ms|card|throw/catch + stack capture"
  "engine|async|ms|card|async/await throughput"
  "engine|timers|ms|card|setTimeout churn"

  "webapi|url|ms|card|URL parsing"
  "webapi|url_setter|ms|chart|URL setter"
  "webapi|urlpattern|ms|chart|URLPattern test"
  "webapi|encoding|ms|card|TextEncoder/TextDecoder (small)"
  "webapi|encoding_large|ms|card|TextEncoder/TextDecoder (64 KiB)"
  "webapi|base64|ms|card|base64 (atob/btoa)"
  "webapi|buffers|ms|card|TypedArray / DataView"
  "webapi|headers|ms|card|Headers / Request parsing"
  "webapi|formdata|ms|card|Multipart FormData round trip"
  "webapi|date_intl|ms|card|Date + Intl formatting"
  "webapi|streams|ms|card|ReadableStream piping"
  "webapi|compression|ms|chart|CompressionStream round trip (gzip)"

  "crypto|sha256|ms|card|SubtleCrypto SHA-256"
  "crypto|crypto|ms|card|HMAC + AES-GCM"
  "crypto|crypto_asym|ms|card|ECDSA P-256 sign + verify"
  "crypto|crypto_kdf|ms|card|PBKDF2 (10k iterations)"

  "net|fetch|ms|card|fetch (local server)"
  "net|fetch_upload|ms|chart|fetch (streamed upload)"
  "net|http|ms|chart|HTTP server (concurrent)"
  "net|websocket|ms|chart|WebSocket round trips"

  "fs|fsread_small|ms|card|File read (small)"
  "fs|fsread_large|ms|card|File read (large)"
  "fs|fswrite_small|ms|card|File write (small)"
  "fs|fswrite_large|ms|card|File write (large)"
  "fs|fsappend_small|ms|card|File append (small)"
  "fs|fsappend_large|ms|card|File append (large)"
  "fs|fsstat_small|ms|card|File stat (one path)"
  "fs|fsstat_many|ms|card|File stat (1000 paths)"
  "fs|fsexists_small|ms|card|File exists (one path)"
  "fs|fsexists_many|ms|card|File exists (1000 paths)"
  "fs|glob|ms|card|Glob scan"

  "system|spawn|ms|card|Spawn a child process"

  "serialization|jsonl_stream|ms|chart|JSONL streaming (large dataset)"
  "serialization|xml_small|ms|chart|XML parsing (small dataset)"
  "serialization|xml_large|ms|chart|XML parsing (large dataset)"
  "serialization|yaml_small|ms|chart|YAML parsing (small dataset)"
  "serialization|yaml_large|ms|chart|YAML parsing (large dataset)"
  "serialization|toml_small|ms|chart|TOML parsing (small dataset)"
  "serialization|toml_large|ms|chart|TOML parsing (large dataset)"
  "serialization|msgpack_small|ms|chart|MessagePack parsing (small dataset)"
  "serialization|msgpack_large|ms|chart|MessagePack parsing (large dataset)"

  "protobuf|protobuf_small|ms|chart|Protobuf decode (small dataset)"
  "protobuf|protobuf_large|ms|chart|Protobuf decode (large dataset)"

  "wasm|wasm_compile|ms|chart|WebAssembly.compile (~250 KB module)"
  "wasm|wasm_call|ms|chart|JS↔wasm call boundary"
  "wasm|wasm_mem|ms|chart|wasm linear memory (shared buffer)"

  "wasi|wasi_start|ms|chart|WASI bootstrap (instantiate + _start)"
  "wasi|wasi_syscall|ms|chart|WASI syscalls from inside wasm"
)

# Rows nothing runs: derived after aggregation from the peak-RSS samples (see
# RSS_ROWS). They are charted like any other row, so they carry the same
# metadata, but they are not in GROUP_MEMBERS — there is no script to select.
SYNTHETIC_ROW_DEFS=(
  "launch|rss|MB|card|Peak resident memory (idle)"
  "launch|rss_loaded|MB|card|Peak resident memory (under load)"
)

# Section headings the site renders for each group.
declare -A GROUP_TITLE
GROUP_TITLE[launch]="Startup & footprint"
GROUP_TITLE[memory]="Memory"
GROUP_TITLE[engine]="Engine"
GROUP_TITLE[webapi]="Web APIs"
GROUP_TITLE[crypto]="Crypto"
GROUP_TITLE[net]="Network"
GROUP_TITLE[fs]="Filesystem"
GROUP_TITLE[system]="System"
GROUP_TITLE[serialization]="Serialization"
GROUP_TITLE[protobuf]="Protobuf"
GROUP_TITLE[wasm]="WebAssembly"
GROUP_TITLE[wasi]="WASI"

# Rows where a bigger number is the better one. Empty today — every row is a
# time or a memory figure — but the site reads the direction from the data
# rather than keeping its own list, so a throughput row added here is ranked and
# captioned correctly everywhere without touching a component.
HIGHER_BETTER_ROWS=""

declare -A GROUP_MEMBERS ROW_GROUP ROW_UNIT ROW_DISPLAY ROW_LABEL
GROUP_ORDER=()
CATALOGUE_ORDER=() # every row, in table order (incl. synthetic and hidden)
DISPLAY_ORDER=()   # only the rows that are shown — what the groups list
_row_def() {  # "group|key|unit|display|label"  synthetic?
  local g k u d l
  IFS='|' read -r g k u d l <<<"$1"
  ROW_GROUP[$k]="$g"; ROW_UNIT[$k]="$u"; ROW_DISPLAY[$k]="$d"; ROW_LABEL[$k]="$l"
  case " ${GROUP_ORDER[*]} " in *" $g "*) ;; *) GROUP_ORDER+=("$g") ;; esac
  CATALOGUE_ORDER+=("$k")
  [ "$d" = hidden ] || DISPLAY_ORDER+=("$k")
  [ -n "${2:-}" ] && return 0
  GROUP_MEMBERS[$g]="${GROUP_MEMBERS[$g]:+${GROUP_MEMBERS[$g]} }$k"
}
for _d in "${ROW_DEFS[@]}"; do [ -n "$_d" ] && _row_def "$_d"; done
for _d in "${SYNTHETIC_ROW_DEFS[@]}"; do _row_def "$_d" synthetic; done

ALL_ROWS=""
for _g in "${GROUP_ORDER[@]}"; do ALL_ROWS="$ALL_ROWS ${GROUP_MEMBERS[$_g]}"; done
ALL_ROWS="${ALL_ROWS# }"

# Resolved here rather than at the knob, which is declared before the row table
# exists. Unset means every row; the sampler skips any that this run did not
# select, so a scoped run still only pays for its own rows.
RSS_ROWS="${RSS_ROWS:-$ALL_ROWS}"

# Scripts under scripts/ that are deliberately not rows: servers driven by
# rps.sh/http2.sh, the OOM probes memory-safety.sh runs, and the module builder
# the wasm rows import. Listed explicitly so that anything *else* unclaimed is a
# real oversight rather than noise a reader learns to scroll past.
NON_ROW_SCRIPTS="helloserver hono staticserver mem_large_string mem_nested_json mem_promise_leak wasm-mod"

# --list: print the groups and exit. Also reports any scripts/*.js that no group
# claims, which is how a new workload gets noticed instead of silently never
# running.
if [ "${1:-}" = "--list" ]; then
  for g in "${GROUP_ORDER[@]}"; do printf '%-14s %s\n' "$g" "${GROUP_MEMBERS[$g]}"; done
  orphans=""
  for f in scripts/*.js; do
    b="$(basename "$f" .js)"
    case " $ALL_ROWS " in *" $b "*) continue ;; esac
    case " $NON_ROW_SCRIPTS " in *" $b "*) continue ;; esac
    orphans="$orphans $b"
  done
  printf '\nnot rows (helpers, by design):%s\n' " $NON_ROW_SCRIPTS"
  if [ -n "$orphans" ]; then
    printf 'IN scripts/ BUT IN NO GROUP — never run:%s\n' "$orphans"
  else
    printf 'every other script in scripts/ belongs to a group.\n'
  fi
  # A group naming a row with no script is the same oversight in reverse.
  missing=""
  for r in $ALL_ROWS; do
    case " $LAUNCH_ROWS " in *" $r "*) continue ;; esac
    [ -f "scripts/$r.js" ] || missing="$missing $r"
  done
  [ -n "$missing" ] && printf 'IN A GROUP BUT HAS NO SCRIPT:%s\n' "$missing"
  exit 0
fi

# Selection: an explicit WORKLOADS list wins, then GROUP, then everything.
if [ -n "${WORKLOADS:-}" ]; then
  SELECTED="$WORKLOADS"
elif [ -n "${GROUP:-}" ]; then
  SELECTED=""
  for g in $GROUP; do
    if [ -z "${GROUP_MEMBERS[$g]:-}" ]; then
      echo "unknown group '$g' — try: ${GROUP_ORDER[*]}" >&2
      exit 2
    fi
    SELECTED="$SELECTED ${GROUP_MEMBERS[$g]}"
  done
  SELECTED="${SELECTED# }"
else
  SELECTED="$ALL_ROWS"
fi

# Split the selection: launch-kind rows are driven separately from the self-timed
# workloads, and only the latter are what the fetch/ws servers key off.
LAUNCH_SELECTED=""
WORKLOADS=""
for r in $SELECTED; do
  case " $LAUNCH_ROWS " in
    *" $r "*) LAUNCH_SELECTED="$LAUNCH_SELECTED $r" ;;
    *) WORKLOADS="$WORKLOADS $r" ;;
  esac
done
LAUNCH_SELECTED="${LAUNCH_SELECTED# }"
WORKLOADS="${WORKLOADS# }"
BENCH_JSON="${BENCH_JSON:-}"
FETCH_PORT=18923
WS_PORT=18924
# Per-workload wall-clock cap so a runtime that can't run a workload (or stalls
# trying — e.g. a partial server API that never responds) yields a clean n/a
# instead of hanging the whole run. Applied via `timeout` if available.
TIMEOUT_BIN=""
command -v timeout >/dev/null 2>&1 && TIMEOUT_BIN="timeout ${WORKLOAD_TIMEOUT:-60}"

# Resolve each runtime's invocation, in display order. Skipped if not found.
declare -A CMD VER
add() { # name  "invocation"  "version-cmd"
  CMD[$1]="$2"
  VER[$1]="$($3 2>/dev/null | head -1)"
}
ORDER=()
command -v node >/dev/null 2>&1 && { add node "node" "node --version"; ORDER+=(node); }
# `--revision`, not `--version`: a canary reports the *unreleased* version it is
# working towards, so `bun --version` on a `bun upgrade --canary` build says
# "1.4.0" with nothing to show 1.4.0 was never released. `--revision` says
# "1.4.0-canary.1+095eb31ae". Deno already discloses its channel ("stable,
# release") and LLRT says "beta"; bun was the only one whose published string
# could be read as a release when it was not one.
command -v bun  >/dev/null 2>&1 && { add bun  "bun"  "bun --revision";  ORDER+=(bun);  }
DENO="$(command -v deno 2>/dev/null)"
[ -z "$DENO" ] && for d in "$HOME/.deno/bin/deno" /tmp/deno/bin/deno; do
  [ -x "$d" ] && { DENO="$d"; break; }
done
[ -n "$DENO" ] && { add deno "$DENO run -A --quiet" "$DENO --version"; ORDER+=(deno); }
# LLRT (AWS Low Latency Runtime): QuickJS-based, cold-start/memory focused. Runs
# the engine + Web-API workloads it supports; the fs/streams/glob/http workloads
# fall through to n/a (it has no general HTTP server and only partial fs). Looked
# for on PATH, then the usual install spots.
LLRT="$(command -v llrt 2>/dev/null)"
[ -z "$LLRT" ] && for d in "$HOME/.llrt/bin/llrt" "$HOME/.local/bin/llrt" /tmp/llrt/llrt; do
  [ -x "$d" ] && { LLRT="$d"; break; }
done
[ -n "$LLRT" ] && { add llrt "$LLRT" "$LLRT --version"; ORDER+=(llrt); }
if [ -x "$ESRUN" ]; then add esrun "$ESRUN" "$ESRUN --version"; ORDER+=(esrun); else
  echo "esrun not found at $ESRUN — build it: cargo build --release -p es-runtime-cli" >&2; exit 1
fi

# Scratch dir (generated bigscript, RSS samples), cleaned on exit.
SCRATCH="$(mktemp -d)"
SERVER_PID=""
WS_SERVER_PID=""
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$WS_SERVER_PID" ] && kill "$WS_SERVER_PID" 2>/dev/null
  rm -rf "$SCRATCH"
}
trap cleanup EXIT

# --- fetch server -----------------------------------------------------------

# Starts the local HTTP server for the fetch + fetch_upload workloads. The server
# drains any request body (so the streamed-upload workload actually transfers its
# bytes) and replies with a fixed payload. Drops both workloads (with a notice) if
# Node is missing or the port doesn't come up.
start_fetch_server() {
  case " $WORKLOADS " in
    *" fetch "* | *" fetch_upload "*) ;;
    *) return ;;
  esac
  if ! command -v node >/dev/null 2>&1; then
    note "fetch/fetch_upload workloads skipped (needs node for the local server)"
    drop_fetch_workloads
    return
  fi
  # If something already holds the port, our server never binds and every
  # runtime would silently measure that stranger instead. Drop the workloads
  # rather than publish numbers about someone else's process.
  if (echo > "/dev/tcp/127.0.0.1/$FETCH_PORT") 2>/dev/null; then
    note "fetch/fetch_upload workloads skipped (port $FETCH_PORT already in use)"
    drop_fetch_workloads
    return
  fi
  node -e '
    const http = require("http");
    http.createServer((req, res) => {
      // Count the request body so the upload workload can verify the bytes
      // actually arrived (a runtime that does not truly stream the body gets a
      // mismatch and is recorded n/a, keeping the comparison fair). A GET still
      // gets the fixed 64-byte payload the buffered `fetch` workload expects.
      let n = 0;
      req.on("data", (c) => { n += c.length; });
      req.on("end", () => {
        res.setHeader("content-type", "text/plain");
        res.end(req.method === "POST" ? String(n) : "x".repeat(64));
      });
    }).listen('"$FETCH_PORT"', "127.0.0.1");
  ' &
  SERVER_PID=$!
  for _ in $(seq 50); do
    (echo > "/dev/tcp/127.0.0.1/$FETCH_PORT") 2>/dev/null && return
    sleep 0.1
  done
  note "fetch/fetch_upload workloads skipped (server did not come up on :$FETCH_PORT)"
  kill "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
  drop_fetch_workloads
}

# Removes whole workload tokens from the active set. Token-safe: a substring
# removal turns "fetch" into a dropped "fetch_upload" too, and leaves an empty
# token behind where it hits.
drop_workloads() {  # name...
  local kept="" w drop
  for w in $WORKLOADS; do
    for drop in "$@"; do
      [ "$w" = "$drop" ] && continue 2
    done
    kept="$kept $w"
  done
  WORKLOADS="${kept# }"
}

drop_fetch_workloads() { drop_workloads fetch fetch_upload; }

# --- websocket echo server --------------------------------------------------

# Starts the local WebSocket echo server for the websocket workload. The clients
# are each runtime's standard `WebSocket` global; the server is whichever
# built-in WS server is available — Bun (`Bun.serve`), Deno (`Deno.upgradeWebSocket`),
# or Node with the `ws` package — so it needs no bundled dependency. Drops the
# workload (with a notice) if none is available or the port doesn't come up.
start_ws_server() {
  case " $WORKLOADS " in *" websocket "*) ;; *) return ;; esac
  # Same trap as the fetch server: a squatter on the port would be measured in
  # place of the echo server we meant to start.
  if (echo > "/dev/tcp/127.0.0.1/$WS_PORT") 2>/dev/null; then
    note "websocket workload skipped (port $WS_PORT already in use)"
    drop_workloads websocket
    return
  fi
  local cmd="" script="$SCRATCH/ws-echo.js"
  if command -v bun >/dev/null 2>&1; then
    cmd="bun"
    cat > "$script" <<EOF
Bun.serve({ port: $WS_PORT, hostname: "127.0.0.1",
  fetch(req, server) { if (server.upgrade(req)) return; return new Response("", { status: 400 }); },
  websocket: { message(ws, m) { ws.send(m); } } });
EOF
  elif [ -n "$DENO" ]; then
    cmd="$DENO run -A --quiet"
    cat > "$script" <<EOF
Deno.serve({ port: $WS_PORT, hostname: "127.0.0.1" }, (req) => {
  const { socket, response } = Deno.upgradeWebSocket(req);
  socket.onmessage = (e) => socket.send(e.data);
  return response;
});
EOF
  elif command -v node >/dev/null 2>&1 && node -e 'require("ws")' >/dev/null 2>&1; then
    cmd="node"
    cat > "$script" <<EOF
const { WebSocketServer } = require("ws");
new WebSocketServer({ host: "127.0.0.1", port: $WS_PORT })
  .on("connection", (ws) => ws.on("message", (m) => ws.send(m, { binary: false })));
EOF
  else
    note "websocket workload skipped (needs bun, deno, or node+ws for the echo server)"
    drop_workloads websocket
    return
  fi
  $cmd "$script" >/dev/null 2>&1 &
  WS_SERVER_PID=$!
  for _ in $(seq 50); do
    (echo > "/dev/tcp/127.0.0.1/$WS_PORT") 2>/dev/null && return
    sleep 0.1
  done
  note "websocket workload skipped (echo server did not come up on :$WS_PORT)"
  kill "$WS_SERVER_PID" 2>/dev/null
  WS_SERVER_PID=""
  drop_workloads websocket
}

# --- generated big script (startup/parse cost) ------------------------------

# A module *graph*, not one big file. `bigscript` is a single ~100 KB source, so
# it measures parse throughput; real cold start is dominated by resolving and
# loading hundreds of small files, which is a different cost (path resolution,
# per-module instantiation, linking) and the one a serverless deploy actually
# pays. Flat fan-out from an entry, plus a shared util every module pulls in, so
# the resolver sees both a wide graph and a repeatedly-requested specifier.
gen_modules() {
  local dir="$SCRATCH/modtree" i
  mkdir -p "$dir"
  echo 'export const tag = (s) => s + "!";' > "$dir/util.js"
  for i in $(seq 300); do
    printf 'import { tag } from "./util.js";\nexport function m%d(x) { return tag("m%d") .length + x; }\n' "$i" "$i" \
      > "$dir/m$i.js"
  done
  {
    for i in $(seq 300); do printf 'import { m%d } from "./m%d.js";\n' "$i" "$i"; done
    echo 'let total = 0;'
    for i in $(seq 300); do printf 'total += m%d(%d);\n' "$i" "$i"; done
    echo 'void total;'
  } > "$dir/entry.js"
  echo "$dir/entry.js"
}

gen_bigscript() {
  local f="$SCRATCH/bigscript.js" i
  {
    for i in $(seq 700); do
      printf 'function fn%d(a, b) { const o = { id: %d, tag: "abcdefghij-%d" }; let t = 0; for (let j = 0; j < 3; j++) t += j * a + b + o.id; return t + o.tag.length; }\n' "$i" "$i" "$i"
    done
    echo 'let total = 0;'
    for i in $(seq 700); do printf 'total += fn%d(%d, 1);\n' "$i" "$i"; done
    echo 'void total;'
  } > "$f"
  echo "$f"
}

# --- measurement ------------------------------------------------------------

now() { date +%s.%N; }
to_ms() { awk "BEGIN{printf \"%.1f\", $1*1000}"; }

# Optional environment hardening (opt-in: QUIET=1). Pins every runtime to the
# same CPU and disables ASLR so all candidates face identical conditions; `nice`
# is applied only as root. Governor/turbo need sudo and are printed as a hint.
# The wrapper prefixes every timed launch (but not RSS sampling, where it would
# confuse the resident-set reading of the immediate child).
WRAP=""
if [ -n "${QUIET:-}" ]; then
  command -v taskset >/dev/null 2>&1 && WRAP="taskset -c ${BENCH_CPU:-0} "
  [ "$(id -u)" = 0 ] && WRAP="${WRAP}nice -n -20 "
  setarch -R true >/dev/null 2>&1 && WRAP="${WRAP}setarch -R "
  note "QUIET: launches wrapped with: ${WRAP:-<none available>}"
  note "QUIET: for lowest variance also run (sudo): cpupower frequency-set -g performance; echo 0 | sudo tee /sys/devices/system/cpu/cpufreq/boost — and close background apps"
fi

# A single timed launch → one ms sample, or a failure token: "ERR" (the runtime
# could not run it) or "TIMEOUT" (it was still going at WORKLOAD_TIMEOUT). The
# two are kept apart because they mean opposite things — one is a missing API,
# the other is a slow answer — and the site renders both as "n/a" otherwise.
#   startup: process wall-time.   workload: the script's self-reported RESULT_MS.
#
# The startup path deliberately runs without `timeout`: at 3-25 ms per launch,
# `timeout`'s own fork would be a measurable share of the very interval being
# measured, and it would weigh heaviest on the fastest runtime. Neither
# startup.js (`void 0`) nor the generated bigscript can hang.
sample_once() {  # kind cmd script
  local kind="$1" cmd="$2" script="$3" s e out rc
  if [ "$kind" = startup ]; then
    s=$(now); $WRAP $cmd "$script" >/dev/null 2>&1; e=$(now)
    to_ms "$(awk "BEGIN{print $e-$s}")"
  else
    $TIMEOUT_BIN $WRAP $cmd "$script" >"$SCRATCH/sample.out" 2>/dev/null; rc=$?
    [ "$rc" -eq 124 ] && { echo TIMEOUT; return; }
    out=$(grep -oE 'RESULT_MS=[0-9.]+' "$SCRATCH/sample.out" | head -1 | cut -d= -f2)
    [ -z "$out" ] && { echo ERR; return; }
    awk "BEGIN{printf \"%.1f\", $out}"
  fi
}

# Reduces a list of samples to "min cov% n gap%": min is the contention-free
# floor (interference only ever adds time); cov is the coefficient of variation,
# used to flag noisy cells and to decide when a row has stopped moving; n is how
# many samples that rests on, which the JSON publishes so a cell's precision is
# visible rather than implied.
#
# gap is how far the *second*-lowest sample sits above the lowest, as a percent.
# Since the published number is the minimum, cov is the wrong thing to judge it
# by: one catastrophic outlier (a writeback stall, a scheduler hiccup) sends cov
# over 100% while leaving the floor exactly where it was. What matters is whether
# anything corroborates that floor — two samples that agree mean the minimum is a
# real measurement, and a lone low sample nothing else comes near means it is a
# fluke. That is the number worth gating publication on.
aggregate() {  # "s1 s2 ..."
  awk '{ for (i=1;i<=NF;i++){ x=$i; sum+=x; sq+=x*x; n++;
           # min2 starts unset (-1), not at the first sample: seeding it to
           # sample one leaves every larger sample failing the `x<min2` test, so
           # a lone low outlier would report a gap of zero — the exact case this
           # exists to catch.
           if (n==1){ min=x; min2=-1 }
           else if (x<min){ min2=min; min=x }
           else if (min2<0 || x<min2){ min2=x } } }
       END{ if (n==0){ print "ERR 0 0 0"; exit }
            mean=sum/n; var=(n>1)?(sq-n*mean*mean)/(n-1):0; if (var<0) var=0;
            cov=(mean>0)?100*sqrt(var)/mean:0;
            gap=(min2>=0 && min>0)?100*(min2-min)/min:0;
            printf "%.1f %.1f %d %.1f", min, cov, n, gap }' <<<"$1"
}

# Peak RSS (MB) of one run, via GNU time or a python3 getrusage fallback.
# Empty (row omitted) if neither is available.
measure_rss() {
  local cmd="$1" script="$2" kb
  if [ -x /usr/bin/time ]; then
    kb=$(/usr/bin/time -v $cmd "$script" 2>&1 >/dev/null |
      grep -oE 'Maximum resident set size \(kbytes\): [0-9]+' | grep -oE '[0-9]+$')
    [ -n "$kb" ] && awk "BEGIN{printf \"%.0f\", $kb/1024}"
  elif command -v python3 >/dev/null 2>&1; then
    python3 - "$cmd" "$script" <<'EOF'
import resource, shlex, subprocess, sys
cmd = shlex.split(sys.argv[1]) + [sys.argv[2]]
subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
print(round(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024))
EOF
  fi
}

note() { [ -z "$BENCH_JSON" ] && echo "$*" >&2; }

shuffle() {  # randomize runtime order each repetition (falls back to fixed)
  if command -v shuf >/dev/null 2>&1; then shuf -e "$@"; else printf '%s\n' "$@"; fi
}

# --- run --------------------------------------------------------------------

start_fetch_server
start_ws_server
# Rows and their (kind, script path). Launch-kind rows come first and are
# generated on demand, so a scoped run does not pay to build a module tree it
# will not measure.
declare -A KIND PATHS
ROWS=()
for r in $LAUNCH_SELECTED; do
  KIND[$r]=startup
  case "$r" in
    startup) PATHS[$r]="scripts/startup.js" ;;
    bigscript) PATHS[$r]="$(gen_bigscript)" ;;
    modules) PATHS[$r]="$(gen_modules)" ;;
    *) echo "launch row '$r' has no generator" >&2; exit 2 ;;
  esac
  ROWS+=("$r")
done
for w in $WORKLOADS; do KIND[$w]=workload; PATHS[$w]="scripts/$w.js"; ROWS+=("$w"); done

declare -A SAMPLES RES COV NUM GAP DEAD

# True once every live cell in the row is within NOISE_THRESHOLD — the point
# where further repetitions stop changing the published minimum. Judged over the
# whole row, never cell by cell, so every runtime keeps an identical sample
# count and the interleaving stays balanced.
row_is_stable() {  # row
  local row="$1" r c
  for r in "${ORDER[@]}"; do
    [ -n "${DEAD[$row,$r]:-}" ] && continue
    [ -z "${SAMPLES[$row,$r]:-}" ] && return 1
    read -r _ c _ _ < <(aggregate "${SAMPLES[$row,$r]}")
    awk "BEGIN{exit !($c > $NOISE_THRESHOLD)}" && return 1
  done
  return 0
}

# Interleaved + randomized collection (see header). Repetition 0 is the
# discarded process-level warmup; a row a runtime can't run fails on that warmup
# and is then skipped entirely (marked DEAD), so unsupported workloads cost two
# launches instead of N.
#
# `reps` is a ceiling, not a quota: once the row is stable the remaining
# repetitions cannot move the minimum, so collection stops at MIN_REPS or later.
# Most cells settle in three, which is where most of the suite's time was going.
collect() {  # row max_reps
  local row="$1" reps="$2" rep r s
  for rep in $(seq 0 "$reps"); do
    while read -r r; do
      [ -n "${DEAD[$row,$r]:-}" ] && continue
      s=$(sample_once "${KIND[$row]}" "${CMD[$r]}" "${PATHS[$row]}")
      case "$s" in
        ERR | TIMEOUT)
          # Retry once before writing the cell off. A missing API fails
          # identically every time; a busy port, a cold page cache or a timeout
          # tripped under momentary load does not — and marking those DEAD
          # publishes "this runtime cannot do this" about a runtime that can.
          if [ "$rep" -eq 0 ]; then
            s=$(sample_once "${KIND[$row]}" "${CMD[$r]}" "${PATHS[$row]}")
            case "$s" in
              ERR) DEAD[$row,$r]=unsupported ;;
              TIMEOUT) DEAD[$row,$r]=timeout ;;
            esac
          fi
          continue
          ;;
      esac
      [ "$rep" -eq 0 ] && continue   # discard warmup repetition
      SAMPLES[$row,$r]="${SAMPLES[$row,$r]:-} $s"
    done < <(shuffle "${ORDER[@]}")
    # Only workload rows stop early. A startup launch costs ~10-25 ms, so the
    # full STARTUP_RUNS is ~3 s for the whole suite — nothing worth reclaiming
    # from the row whose wall-clock measurement is the noisiest of the lot.
    [ "${KIND[$row]}" = workload ] &&
      [ "$rep" -ge "$MIN_REPS" ] && row_is_stable "$row" && break
  done
  return 0
}

for row in "${ROWS[@]}"; do
  if [ "${KIND[$row]}" = startup ]; then collect "$row" "$STARTUP_RUNS"; else collect "$row" "$WORKLOAD_RUNS"; fi
done

# Aggregate each cell to its min + CoV + n; collect the noisy ones to disclose,
# and the ones so fast the measurement is dominated by timer resolution.
NOISY=()
TOOFAST=()
for row in "${ROWS[@]}"; do
  for r in "${ORDER[@]}"; do
    if [ -z "${SAMPLES[$row,$r]:-}" ]; then RES[$row,$r]=ERR; continue; fi
    read -r m c n g < <(aggregate "${SAMPLES[$row,$r]}")
    RES[$row,$r]=$m; COV[$row,$r]=$c; NUM[$row,$r]=$n; GAP[$row,$r]=$g
    awk "BEGIN{exit !($c > $NOISE_THRESHOLD)}" && NOISY+=("$row/$r ${c}%")
    [ "${KIND[$row]}" = workload ] &&
      awk "BEGIN{exit !($m < $MIN_MS)}" && TOOFAST+=("$row/$r ${m}ms")
  done
done

# RSS is a memory floor (contention doesn't inflate peak RSS): one sample each,
# and only for the rows in RSS_ROWS — see the note there.
declare -A RSS
for row in $RSS_ROWS; do
  [ -n "${KIND[$row]:-}" ] || continue
  for r in "${ORDER[@]}"; do
    if [ "${RES[$row,$r]:-}" != ERR ] && [ -n "${RES[$row,$r]:-}" ]; then
      RSS[$row,$r]=$(measure_rss "${CMD[$r]}" "${PATHS[$row]}")
    fi
  done
done

# Two memory rows, published as ordinary rows so the site charts them like any
# other. `rss` is the floor: peak resident set of a near-empty process, which is
# the figure every runtime looks best on and the least like production.
# `rss_loaded` is the same measurement taken while the rss_load workload holds a
# live working set, which is the number that decides whether a box stays up.
for r in "${ORDER[@]}"; do RES[rss,$r]="${RSS[startup,$r]:-}"; done
[ -n "${RES[rss,${ORDER[0]}]:-}" ] && ROWS+=(rss)
for r in "${ORDER[@]}"; do RES[rss_loaded,$r]="${RSS[rss_load,$r]:-}"; done
[ -n "${RES[rss_loaded,${ORDER[0]}]:-}" ] && ROWS+=(rss_loaded)

[ "${#NOISY[@]}" -gt 0 ] &&
  note "noisy cells (CoV > ${NOISE_THRESHOLD}%; min floor still shown, marked ~): ${NOISY[*]}"
# A cell under MIN_MS is not a fast runtime, it is a workload that stopped doing
# enough work to measure — scale its N up rather than publishing the ranking.
[ "${#TOOFAST[@]}" -gt 0 ] &&
  note "below the measurement floor (< ${MIN_MS}ms — raise that workload's N): ${TOOFAST[*]}"
# A cell dropped for taking too long is not a cell the runtime cannot do. Both
# reach the site as null, so say which is which here.
TIMEDOUT=()
for row in "${ROWS[@]}"; do
  for r in "${ORDER[@]}"; do
    [ "${DEAD[$row,$r]:-}" = timeout ] && TIMEDOUT+=("$row/$r")
  done
done
[ "${#TIMEDOUT[@]}" -gt 0 ] &&
  note "timed out at ${WORKLOAD_TIMEOUT:-60}s (recorded n/a, but NOT unsupported): ${TIMEDOUT[*]}"

# --- output -----------------------------------------------------------------

# Why a cell has no number, so the site can distinguish "this runtime has no
# such API" from "this run could not measure it". Values are pre-quoted: the
# emitter below prints them bare.
declare -A STATUS
for row in "${ROWS[@]}"; do
  for r in "${ORDER[@]}"; do
    case "${DEAD[$row,$r]:-}" in
      unsupported) STATUS[$row,$r]='"unsupported"' ;;
      timeout) STATUS[$row,$r]='"timeout"' ;;
      *)
        if [ -n "${RES[$row,$r]:-}" ] && [ "${RES[$row,$r]}" != ERR ]; then
          STATUS[$row,$r]='"ok"'
        else
          STATUS[$row,$r]='"unmeasured"'
        fi
        ;;
    esac
  done
done

# Emits one `row -> runtime -> value` object from an associative array keyed
# "$row,$rt". Values print bare, so callers pass numbers, `null`, or pre-quoted
# strings; an empty or ERR cell becomes null.
emit_matrix() {  # assoc-array-name
  local -n M="$1"
  local firstrow=1 first row r v
  for row in "${ROWS[@]}"; do
    [ -z "$firstrow" ] && printf ','
    firstrow=
    printf '\n    "%s": {' "$row"
    first=1
    for r in "${ORDER[@]}"; do
      [ -z "$first" ] && printf ','
      first=
      v="${M[$row,$r]:-null}"
      case "$v" in '' | ERR) v=null ;; esac
      printf '\n      "%s": %s' "$r" "$v"
    done
    printf '\n    }'
  done
}

if [ -n "$BENCH_JSON" ]; then
  printf '{\n  "runtimes": {'
  first=1
  for r in "${ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    printf '\n    "%s": "%s"' "$r" "${VER[$r]}"
  done
  printf '\n  },'
  # How the numbers below were produced, recorded next to them so the site can
  # state its own methodology instead of a human restating it in prose.
  printf '\n  "method": {'
  printf '\n    "aggregate": "min",'
  printf '\n    "interleaved": true,'
  printf '\n    "shuffled": true,'
  printf '\n    "warmup_reps_discarded": 1,'
  printf '\n    "min_reps": %s,' "$MIN_REPS"
  printf '\n    "max_workload_reps": %s,' "$WORKLOAD_RUNS"
  printf '\n    "max_startup_reps": %s,' "$STARTUP_RUNS"
  printf '\n    "noise_threshold_cov_pct": %s,' "$NOISE_THRESHOLD"
  printf '\n    "quiet": %s,' "$([ -n "${QUIET:-}" ] && echo true || echo false)"
  # Which cores the launches were pinned to, because the breadth changes what
  # the I/O rows mean: on one core a runtime's threadpool is serialized, so
  # fs/http/websocket become single-core numbers rather than practical ones.
  printf '\n    "pinned_cpus": %s' "$([ -n "${QUIET:-}" ] && printf '"%s"' "${BENCH_CPU:-0}" || echo null)"
  printf '\n  },'
  # The machine, recorded next to the numbers. The fs rows in particular are
  # meaningless without it — an append benchmark on ext4, on tmpfs and on a
  # Docker overlay are three different measurements — and the launch rows move
  # with CPU and frequency governor. Published so a reader can tell whether these
  # numbers describe hardware like theirs, rather than assuming they do.
  printf '\n  "environment": {'
  printf '\n    "os": "%s",' "$(uname -sr 2>/dev/null | tr -d '"')"
  printf '\n    "arch": "%s",' "$(uname -m 2>/dev/null)"
  printf '\n    "cpu": "%s",' "$(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//; s/"//g')"
  printf '\n    "cores": "%s",' "$(nproc 2>/dev/null)"
  printf '\n    "filesystem": "%s",' "$(stat -f -c %T . 2>/dev/null)"
  printf '\n    "governor": "%s"' "$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null)"
  printf '\n  },'
  # The row catalogue: what each row is called, what it reports, and where it is
  # shown. Published whole on every run — including a scoped one, which measures
  # a few rows but still knows about all of them — so the site never holds a
  # partial or stale copy of it. This is what lets the benchmarks page and the
  # home-page roller render from the data instead of from hand-kept lists.
  printf '\n  "rows": {'
  first=1
  for row in "${CATALOGUE_ORDER[@]}"; do
    [ -z "$first" ] && printf ','
    first=
    printf '\n    "%s": { "label": "%s", "unit": "%s", "group": "%s", "display": "%s", "better": "%s" }' \
      "$row" "${ROW_LABEL[$row]}" "${ROW_UNIT[$row]}" "${ROW_GROUP[$row]}" "${ROW_DISPLAY[$row]}" \
      "$(case " $HIGHER_BETTER_ROWS " in *" $row "*) echo higher ;; *) echo lower ;; esac)"
  done
  printf '\n  },'
  printf '\n  "groups": ['
  first=1
  for g in "${GROUP_ORDER[@]}"; do
    members=""
    for row in "${DISPLAY_ORDER[@]}"; do
      [ "${ROW_GROUP[$row]}" = "$g" ] && members="${members:+$members, }\"$row\""
    done
    [ -z "$members" ] && continue   # a group whose every row is hidden
    [ -z "$first" ] && printf ','
    first=
    printf '\n    { "id": "%s", "title": "%s", "rows": [%s] }' \
      "$g" "${GROUP_TITLE[$g]:-$g}" "$members"
  done
  printf '\n  ],'
  printf '\n  "results_ms": {'
  emit_matrix RES
  printf '\n  },\n  "results_rss": {'
  emit_matrix RSS
  printf '\n  },\n  "results_cov": {'
  emit_matrix COV
  printf '\n  },\n  "results_n": {'
  emit_matrix NUM
  printf '\n  },\n  "results_floor_gap": {'
  emit_matrix GAP
  printf '\n  },\n  "status": {'
  emit_matrix STATUS
  printf '\n  }\n}\n'
  exit 0
fi

echo "ES-Runtime cross-runtime benchmark"
echo "=================================="
for r in "${ORDER[@]}"; do printf "  %-6s %s\n" "$r" "${VER[$r]}"; done
echo
echo "Interleaved + randomized runs; min of N (the contention-free floor), after a"
echo "discarded warmup. startup/bigscript: process wall-time, min of $STARTUP_RUNS."
echo "Other workloads: self-timed after an in-process warmup, min of $WORKLOAD_RUNS."
echo "Memory: peak resident set (MB) sampled once per workload."
echo "All times in milliseconds, lower is better. Format: time / memory."
echo "~ marks a noisy cell (CoV > ${NOISE_THRESHOLD}%)."
echo

printf "%-15s" "workload"
for r in "${ORDER[@]}"; do printf " | %13s" "$r"; done
printf "\n"
printf -- "---------------"
for _ in "${ORDER[@]}"; do printf -- "+--------------"; done
printf "\n"
for row in "${ROWS[@]}"; do
  printf "%-15s" "$row"
  for r in "${ORDER[@]}"; do
    v="${RES[$row,$r]:--}"
    if [ "$v" = ERR ]; then
      v="n/a" # workload the runtime doesn't support
    else
      # Flag a noisy cell so a wobbly number isn't read as precise.
      c="${COV[$row,$r]:-0}"
      awk "BEGIN{exit !($c > $NOISE_THRESHOLD)}" && v="${v}~"
      if [ -n "${RSS[$row,$r]:-}" ]; then
        v="${v} / ${RSS[$row,$r]}M"
      fi
    fi
    printf " | %13s" "$v"
  done
  printf "\n"
done
