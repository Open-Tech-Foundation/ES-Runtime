#!/usr/bin/env bash
# Measures what each runtime lets you find out about a connection that failed
# before it became a request, then rewrites the table in the internals page
# between its markers.
#
# The probe is one packet: plaintext sent at a TLS port. Every server rejects it
# — the question is whether anything downstream of the server can learn that it
# happened, and from which peer. A TLS listener whose certificate no client will
# accept behaves identically to a listener nobody is calling, so this is the
# difference between a diagnosable misconfiguration and an invisible one.
#
#   bash bench/probe-observability.sh            # measure and update the doc
#   bash bench/probe-observability.sh --json     # print the measurements
#   bash bench/probe-observability.sh --check    # fail if the doc is out of date
#
# Needs node, bun, deno and openssl on PATH, and a release esrun
# (cargo build --release). A runtime that is missing is reported as "n/a".
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOC="$ROOT/website/app/docs/internals/http/page.mdx"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"; kill $(jobs -p) 2>/dev/null' EXIT

ESRUN="$ROOT/target/release/esrun"

openssl req -x509 -newkey rsa:2048 -keyout "$WORK/key.pem" -out "$WORK/cert.pem" \
  -days 2 -nodes -subj "/CN=localhost" 2>/dev/null

# ---------------------------------------------------------------- the servers
#
# Each prints "PORT <n>" on stdout once listening, and installs whatever hook
# the runtime documents for a connection-level failure — the point is to give
# every runtime its best case, not to catch one out. A hook that fires prints a
# line starting with "HOOK".

cat > "$WORK/esrun.mjs" <<'EOF'
import { serve } from "runtime:http";
import { env } from "runtime:process";
const s = serve(
  { hostname: "127.0.0.1", port: 0, secureTransport: "on", cert: env.CERT, key: env.KEY },
  () => new Response("x"),
);
console.log("PORT " + (await s.addr).port);
EOF

cat > "$WORK/node.mjs" <<'EOF'
import { createServer } from "node:https";
const s = createServer({ cert: process.env.CERT, key: process.env.KEY }, (q, r) => r.end("x"));
s.on("tlsClientError", (e, sock) =>
  console.log("HOOK tlsClientError " + (e.code ?? "") + " " + sock.remoteAddress + ":" + sock.remotePort));
s.on("clientError", (e) => console.log("HOOK clientError " + (e.code ?? "")));
s.listen(0, "127.0.0.1", () => console.log("PORT " + s.address().port));
EOF

cat > "$WORK/deno.mjs" <<'EOF'
Deno.serve({
  hostname: "127.0.0.1", port: 0,
  cert: Deno.env.get("CERT"), key: Deno.env.get("KEY"),
  onListen: ({ port }) => console.log("PORT " + port),
  onError: (e) => { console.log("HOOK onError " + e); return new Response("e", { status: 500 }); },
}, () => new Response("x"));
EOF

cat > "$WORK/bun.mjs" <<'EOF'
const s = Bun.serve({
  hostname: "127.0.0.1", port: 0,
  tls: { cert: process.env.CERT, key: process.env.KEY },
  error: (e) => { console.log("HOOK error " + e); return new Response("e", { status: 500 }); },
  fetch: () => new Response("x"),
});
console.log("PORT " + s.port);
EOF

# ----------------------------------------------------------------- the probe

have() { command -v "$1" >/dev/null 2>&1; }

# Runs one server, sends plaintext at its TLS port, and echoes
# "<hook> <default> <verbose>" — each yes/no.
#   hook     a documented callback fired, and it named the peer
#   default  something reached stderr with no extra configuration
#   verbose  something reached stderr under the runtime's own debug switch
probe() {
  local rt="$1" verbose_env="$2"
  local hook=no default=no verbose=no
  for mode in plain verbose; do
    local env_prefix=()
    [ "$mode" = verbose ] && [ -n "$verbose_env" ] && env_prefix=(env "$verbose_env")

    case "$rt" in
      esrun) "${env_prefix[@]}" env CERT="$(cat "$WORK/cert.pem")" KEY="$(cat "$WORK/key.pem")" \
               "$ESRUN" "$WORK/esrun.mjs" >"$WORK/$rt.$mode.out" 2>"$WORK/$rt.$mode.err" & ;;
      node)  "${env_prefix[@]}" env CERT="$(cat "$WORK/cert.pem")" KEY="$(cat "$WORK/key.pem")" \
               node "$WORK/node.mjs" >"$WORK/$rt.$mode.out" 2>"$WORK/$rt.$mode.err" & ;;
      deno)  "${env_prefix[@]}" env CERT="$(cat "$WORK/cert.pem")" KEY="$(cat "$WORK/key.pem")" \
               deno run -A "$WORK/deno.mjs" >"$WORK/$rt.$mode.out" 2>"$WORK/$rt.$mode.err" & ;;
      bun)   "${env_prefix[@]}" env CERT="$(cat "$WORK/cert.pem")" KEY="$(cat "$WORK/key.pem")" \
               bun "$WORK/bun.mjs" >"$WORK/$rt.$mode.out" 2>"$WORK/$rt.$mode.err" & ;;
    esac
    local pid=$!

    local port=""
    for _ in $(seq 150); do
      port="$(grep -m1 '^PORT ' "$WORK/$rt.$mode.out" 2>/dev/null | awk '{print $2}')"
      [ -n "$port" ] && break
      sleep 0.2
    done
    if [ -z "$port" ]; then kill "$pid" 2>/dev/null; continue; fi

    # One plaintext record at a TLS port. rustls calls it a corrupt message;
    # OpenSSL calls it an HTTP request. Either way the handshake cannot start.
    printf 'GET / HTTP/1.1\r\nHost: x\r\n\r\n' | timeout 3 nc 127.0.0.1 "$port" >/dev/null 2>&1
    sleep 1
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null

    if [ "$mode" = plain ]; then
      grep -q '^HOOK' "$WORK/$rt.$mode.out" && grep -q '127\.0\.0\.1:' "$WORK/$rt.$mode.out" && hook=yes
      grep -qiE 'handshake|ssl|tls' "$WORK/$rt.$mode.err" 2>/dev/null && default=yes
    else
      grep -qiE 'handshake|ssl routines|tls_validate' "$WORK/$rt.$mode.err" 2>/dev/null && verbose=yes
    fi
  done
  echo "$hook $default $verbose"
}

declare -A R
# The switch each runtime documents for its own verbose output.
R[esrun]="$(have "$ESRUN" || [ -x "$ESRUN" ] && probe esrun "RUST_LOG=runtime::http=debug" || echo "n/a n/a n/a")"
R[node]="$(have node  && probe node "NODE_DEBUG=tls"    || echo "n/a n/a n/a")"
R[bun]="$( have bun   && probe bun  "BUN_DEBUG_ALL=1"   || echo "n/a n/a n/a")"
R[deno]="$(have deno  && probe deno "DENO_LOG=debug"    || echo "n/a n/a n/a")"

field() { echo "$1" | awk -v n="$2" '{print $n}'; }

row() {
  local label="$1" n="$2"
  local out="| $label |"
  for rt in esrun node bun deno; do out="$out $(field "${R[$rt]}" "$n") |"; done
  echo "$out"
}

version() {
  case "$1" in
    esrun) [ -x "$ESRUN" ] && "$ESRUN" --version 2>/dev/null | awk '{print $NF}' || echo n/a ;;
    *) have "$1" && "$1" --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo n/a ;;
  esac
}

TABLE="$(
  echo "| | esrun | Node.js | Bun | Deno |"
  echo "| --- | --- | --- | --- | --- |"
  row "Reported by default" 2
  row "Reported under the runtime's debug switch" 3
  row "A callback receives it, with the peer" 1
  echo
  echo "<sub>esrun $(version esrun) · Node $(version node) · Bun $(version bun) · Deno $(version deno) · $(uname -s) · $(date -u +%Y-%m-%d)</sub>"
)"

if [ "${1:-}" = "--json" ]; then
  for rt in esrun node bun deno; do echo "$rt hook/default/verbose=${R[$rt]}"; done
  exit 0
fi

NEW="$(awk -v table="$TABLE" '
  /BEGIN probe:observability/ { print; print table; skip = 1; next }
  /END probe:observability/   { skip = 0 }
  !skip { print }
' "$DOC")"

if [ "${1:-}" = "--check" ]; then
  if [ "$NEW" = "$(cat "$DOC")" ]; then
    echo "the internals page is up to date" >&2
    exit 0
  fi
  echo "the internals page is out of date — run: bash bench/probe-observability.sh" >&2
  diff <(echo "$NEW") "$DOC" >&2 || true
  exit 1
fi

printf '%s\n' "$NEW" > "$DOC"
echo "updated $DOC" >&2
