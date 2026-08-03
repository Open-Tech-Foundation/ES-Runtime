#!/usr/bin/env bash
# Measures what each runtime's HTTP server actually does with a connection that
# is not making progress, and what it advertises in its HTTP/2 SETTINGS. Then
# rewrites the table in the internals page between its markers.
#
# Documentation numbers rot. These are the ones a reader is most likely to act
# on and least able to check, so they are produced by running the four servers
# rather than by reading four sets of docs — and regenerating is one command.
#
#   bash bench/probe-runtimes.sh            # measure and update the doc
#   bash bench/probe-runtimes.sh --json     # print the measurements, touch nothing
#   bash bench/probe-runtimes.sh --check    # fail if the doc is out of date
#
# Needs node, bun and deno on PATH, and a release esrun (cargo build --release).
# A runtime that is missing is reported as "n/a" rather than silently skipped.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOC="$ROOT/website/app/docs/internals/networking/page.mdx"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"; kill $(jobs -p) 2>/dev/null' EXIT

# How long to wait before calling a connection "never closed". Node needs more
# than a minute (a 60s headersTimeout polled on a 30s interval), so a cap under
# ~90s would report a bound server as unbounded — the one error that would make
# this table actively misleading.
CAP="${PROBE_CAP:-150}"

ESRUN="$ROOT/target/release/esrun"

# ---------------------------------------------------------------- the servers

cat > "$WORK/node-h1.js" <<'EOF'
require("node:http").createServer((q, s) => s.end("x")).listen(Number(process.argv[2]));
EOF
cat > "$WORK/node-h2.js" <<'EOF'
require("node:http2").createServer((q, s) => s.end("x")).listen(Number(process.argv[2]));
EOF
cat > "$WORK/bun-h1.js" <<'EOF'
Bun.serve({ port: Number(process.argv[2]), fetch: () => new Response("x") });
EOF
# Bun serves cleartext HTTP/2 through node:http2; Bun.serve is HTTP/1.1-only.
cp "$WORK/node-h2.js" "$WORK/bun-h2.js"
cat > "$WORK/deno-h1.js" <<'EOF'
Deno.serve({ port: Number(Deno.args[0]) }, () => new Response("x"));
EOF
cp "$WORK/deno-h1.js" "$WORK/deno-h2.js"
# esrun's port is written into the module rather than passed as an argument, so
# the fixture stays the plain `serve()` a user would write.
esrun_fixture() {
  printf 'import { serve } from "runtime:http";\nserve({ port: %s }, () => new Response("x"));\n' "$1"
}

# ---------------------------------------------------------------- the probes

# Time to close for (1) a connection that says nothing and (2) one that
# completes a request and then goes idle. The socket is drained: a paused Node
# socket holding an unread response never emits "close", which reads as "still
# open" and silently turns every measurement into a maximum.
cat > "$WORK/timing.mjs" <<'EOF'
import net from "node:net";
const [portArg, capArg] = process.argv.slice(2);
const port = Number(portArg);
const CAP = Number(capArg) * 1000;

function timeToClose(greeting) {
  return new Promise((resolve) => {
    const started = Date.now();
    const s = net.connect(port, "127.0.0.1", () => { if (greeting) s.write(greeting); });
    const done = (what) => { s.destroy(); resolve(what); };
    s.on("data", () => {});
    s.on("close", () => done(`${((Date.now() - started) / 1000).toFixed(1)}s`));
    s.on("error", () => done("refused"));
    setTimeout(() => done(`never (>${CAP / 1000}s)`), CAP);
  });
}
const silent = await timeToClose(null);
const idle = await timeToClose("GET / HTTP/1.1\r\nHost: x\r\n\r\n");
console.log(JSON.stringify({ silent, idle }));
EOF

# What the server tells every client its limits are, read off its SETTINGS
# frame rather than from its documentation.
cat > "$WORK/settings.mjs" <<'EOF'
import http2 from "node:http2";
const port = Number(process.argv[2]);
const c = http2.connect(`http://127.0.0.1:${port}`);
const give = (o) => { console.log(JSON.stringify(o)); process.exit(0); };
c.on("error", () => give({ streams: "n/a", headerList: "n/a", window: "n/a" }));
c.on("remoteSettings", (s) => {
  const cap = (v) => (v >= 2 ** 31 ? "unlimited" : v);
  const kb = (v) => (v >= 2 ** 31 ? "unlimited" : v % 1048576 === 0 ? `${v / 1048576}MB` : `${Math.round(v / 1024)}KB`);
  give({ streams: cap(s.maxConcurrentStreams), headerList: kb(s.maxHeaderListSize), window: kb(s.initialWindowSize) });
});
setTimeout(() => give({ streams: "n/a", headerList: "n/a", window: "n/a" }), 5000);
EOF

have() { command -v "$1" >/dev/null 2>&1; }

# Starts one server, runs one probe against it, stops the server.
# probe <runtime> <h1|h2> <port> <probe.mjs>
probe() {
  local rt="$1"
  local kind="$2"
  local port="$3"
  local script="$4"
  local pid
  local out
  case "$rt" in
    node) have node || { echo '{}'; return; }; node "$WORK/node-$kind.js" "$port" >/dev/null 2>&1 & pid=$! ;;
    bun)  have bun  || { echo '{}'; return; }; bun  "$WORK/bun-$kind.js"  "$port" >/dev/null 2>&1 & pid=$! ;;
    deno) have deno || { echo '{}'; return; }; deno run -A --quiet "$WORK/deno-$kind.js" "$port" >/dev/null 2>&1 & pid=$! ;;
    esrun)
      [ -x "$ESRUN" ] || { echo '{}'; return; }
      esrun_fixture "$port" > "$WORK/esrun-$kind-$port.mjs"
      "$ESRUN" "$WORK/esrun-$kind-$port.mjs" >/dev/null 2>&1 & pid=$! ;;
  esac
  sleep 2
  out="$(node "$script" "$port" "$CAP" 2>/dev/null)"
  kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  echo "${out:-\{\}}"
}

echo "probing four runtimes (up to $((CAP * 2)) seconds per runtime)…" >&2

port=8410
declare -A T S
for rt in esrun node bun deno; do
  echo "  $rt: timings…" >&2
  T[$rt]="$(probe "$rt" h1 "$port" "$WORK/timing.mjs")"; port=$((port + 1))
  echo "  $rt: http/2 settings…" >&2
  S[$rt]="$(probe "$rt" h2 "$port" "$WORK/settings.mjs")"; port=$((port + 1))
done

field() { node -e 'const o=JSON.parse(process.argv[1]||"{}");process.stdout.write(String(o[process.argv[2]] ?? "n/a"))' "$1" "$2"; }

version() {
  case "$1" in
    node) have node && node --version | tr -d 'v' || echo "n/a" ;;
    bun)  have bun  && bun --version || echo "n/a" ;;
    deno) have deno && deno --version | head -1 | awk '{print $2}' || echo "n/a" ;;
    esrun) [ -x "$ESRUN" ] && "$ESRUN" --version 2>/dev/null | awk '{print $NF}' || echo "n/a" ;;
  esac
}

row() {
  # Separate statements on purpose: under `set -u`, bash declares every name in
  # one `local` before evaluating any of the right-hand sides, so a later
  # assignment cannot read an earlier one from the same statement.
  local label="$1"
  local src="$2"
  local key="$3"
  local out="| $label |"
  for rt in esrun node bun deno; do
    if [ "$src" = T ]; then out="$out $(field "${T[$rt]}" "$key") |"; else out="$out $(field "${S[$rt]}" "$key") |"; fi
  done
  echo "$out"
}

TABLE="$(
  echo "| | esrun | Node.js | Bun | Deno |"
  echo "| --- | --- | --- | --- | --- |"
  row "Silent connection closed after" T silent
  row "Idle keep-alive closed after" T idle
  row "HTTP/2 concurrent streams" S streams
  row "HTTP/2 header list" S headerList
  row "HTTP/2 initial window" S window
  echo
  echo "<sub>esrun $(version esrun) · Node $(version node) · Bun $(version bun) · Deno $(version deno) · $(uname -s) · $(date -u +%Y-%m-%d)</sub>"
)"

if [ "${1:-}" = "--json" ]; then
  for rt in esrun node bun deno; do echo "$rt timings=${T[$rt]} settings=${S[$rt]}"; done
  exit 0
fi

# Splice between the markers, leaving the surrounding prose alone.
NEW="$(awk -v table="$TABLE" '
  /BEGIN probe:table/ { print; print table; skip = 1; next }
  /END probe:table/   { skip = 0 }
  !skip { print }
' "$DOC")"

if [ "${1:-}" = "--check" ]; then
  if [ "$NEW" = "$(cat "$DOC")" ]; then
    echo "the internals page is up to date" >&2
    exit 0
  fi
  echo "the internals page is out of date — run: bash bench/probe-runtimes.sh" >&2
  diff <(echo "$NEW") "$DOC" >&2 || true
  exit 1
fi

printf '%s\n' "$NEW" > "$DOC"
echo "updated $DOC" >&2
