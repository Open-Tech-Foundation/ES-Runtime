#!/usr/bin/env bash
# Measures what each runtime's WebSocket server does with a connection that
# never completes its opening handshake, and what its connection cap does with
# one over the limit. Then rewrites the table in the internals page between its
# markers.
#
#   bash bench/probe-ws-bounds.sh            # measure and update the doc
#   bash bench/probe-ws-bounds.sh --json     # print the measurements
#   bash bench/probe-ws-bounds.sh --check    # fail if the doc is out of date
#
# The Node server is a raw `node:http` server with a hand-written upgrade, not
# the `ws` package. That is not a shortcut: `ws` attaches to a `node:http`
# server, so the http server's own bound on a request head *is* what bounds a
# `ws` handshake. Measuring it directly costs no dependency and gives the same
# answer.
#
# Needs node, bun, deno and python3 on PATH, and a **current** release esrun —
# `cargo build --release` first, or this measures the last binary you built.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOC="$ROOT/website/app/docs/internals/websockets/page.mdx"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"; kill $(jobs -p) 2>/dev/null' EXIT

# How long before a connection counts as "never closed". Node needs more than a
# minute here, so a cap under ~90s would report a bounded server as unbounded —
# the one error that would make this table actively misleading.
CAP_WAIT="${PROBE_CAP:-150}"
ESRUN="$ROOT/target/release/esrun"

# ---------------------------------------------------------------- the servers
#
# Each prints "PORT <n>" once listening, and applies a one-connection cap when
# CAP is set — where the runtime has no such option, the option is passed
# anyway, so "no cap" is a measured result rather than an assumption.

cat > "$WORK/esrun.mjs" <<'EOF'
import { serve } from "runtime:websocket";
import { env } from "runtime:process";
const opts = { hostname: "127.0.0.1", port: 0 };
if (env.CAP) opts.maxConnections = Number(env.CAP);
const s = serve(opts);
console.log("PORT " + (await s.addr).port);
for await (const ws of s) ws.onmessage = () => {};
EOF

cat > "$WORK/node.mjs" <<'EOF'
import { createServer } from "node:http";
import { createHash } from "node:crypto";
const s = createServer((q, r) => r.end("no"));
s.on("upgrade", (req, sock) => {
  const accept = createHash("sha1")
    .update(req.headers["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")
    .digest("base64");
  sock.write("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n" +
    "Connection: Upgrade\r\nSec-WebSocket-Accept: " + accept + "\r\n\r\n");
});
if (process.env.CAP) s.maxConnections = Number(process.env.CAP);
s.listen(0, "127.0.0.1", () => console.log("PORT " + s.address().port));
EOF

cat > "$WORK/deno.mjs" <<'EOF'
const opts = { hostname: "127.0.0.1", port: 0,
  onListen: ({ port }) => console.log("PORT " + port) };
if (Deno.env.get("CAP")) opts.maxConnections = Number(Deno.env.get("CAP"));
Deno.serve(opts, (req) => {
  if (req.headers.get("upgrade") !== "websocket") return new Response("no");
  return Deno.upgradeWebSocket(req).response;
});
EOF

cat > "$WORK/bun.mjs" <<'EOF'
const opts = { hostname: "127.0.0.1", port: 0,
  fetch(req, server) { if (server.upgrade(req)) return; return new Response("no"); },
  websocket: { message() {} } };
if (process.env.CAP) opts.maxConnections = Number(process.env.CAP);
const s = Bun.serve(opts);
console.log("PORT " + s.port);
EOF

# ------------------------------------------------------------------ the probes

cat > "$WORK/silent.py" <<'EOF'
# Opens a TCP connection to a WebSocket port and never sends the upgrade
# request — the cheapest hold there is — then reports how long until the server
# gives up on it.
import socket, sys, time
port, cap = int(sys.argv[1]), float(sys.argv[2])
s = socket.create_connection(("127.0.0.1", port)); s.settimeout(cap)
t = time.time()
try:
    got = s.recv(4096)
    # Node answers 408 before closing; either way the connection is over.
    print("%.1fs" % (time.time() - t))
except socket.timeout:
    print("never (>%ds)" % cap)
except Exception:
    print("%.1fs" % (time.time() - t))
EOF

cat > "$WORK/cap.py" <<'EOF'
# Establishes one WebSocket, then opens a second while the cap of 1 is full, and
# reports what happened to it: held (still connected, unanswered), refused
# (closed or reset), or served (the cap did not apply).
import socket, sys, time, base64, os
port = int(sys.argv[1])
def handshake(s):
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall(("GET / HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
               "Sec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n\r\n" % key).encode())
    s.settimeout(5)
    return s.recv(256)
first = socket.create_connection(("127.0.0.1", port))
if b"101" not in handshake(first):
    print("n/a"); raise SystemExit
second = socket.create_connection(("127.0.0.1", port))
try:
    r = handshake(second)
    if r == b"":       print("refused")
    elif b"101" in r:  print("no cap")
    else:              print("refused")
except socket.timeout:      print("held")
except ConnectionResetError: print("refused")
EOF

have() { command -v "$1" >/dev/null 2>&1; }

# Starts one server and echoes its port, or nothing if it never listened.
start() {
  local work_out="$1"; shift
  "$@" > "$work_out" 2>/dev/null &
  echo $! > "$work_out.pid"
  for _ in $(seq 150); do
    local port
    port="$(grep -m1 '^PORT ' "$work_out" 2>/dev/null | awk '{print $2}')"
    [ -n "$port" ] && { echo "$port"; return; }
    sleep 0.2
  done
}

stop() { kill "$(cat "$1.pid" 2>/dev/null)" 2>/dev/null; wait "$(cat "$1.pid" 2>/dev/null)" 2>/dev/null; }

# Echoes "<silent> <cap>" for one runtime.
measure() {
  local rt="$1"
  local silent="n/a" capped="n/a"

  local port
  port="$(start "$WORK/$rt.silent" run "$rt")"
  if [ -n "$port" ]; then silent="$(python3 "$WORK/silent.py" "$port" "$CAP_WAIT")"; fi
  stop "$WORK/$rt.silent"

  port="$(CAP=1 start "$WORK/$rt.cap" run "$rt")"
  if [ -n "$port" ]; then capped="$(python3 "$WORK/cap.py" "$port")"; fi
  stop "$WORK/$rt.cap"

  # Tab-separated, not space: "never (>150s)" and "no cap" both contain a
  # space, and splitting on whitespace shifted them into the next column.
  printf '%s\t%s\n' "$silent" "$capped"
}

run() {
  case "$1" in
    esrun) exec "$ESRUN" "$WORK/esrun.mjs" ;;
    node)  exec node "$WORK/node.mjs" ;;
    deno)  exec deno run -A "$WORK/deno.mjs" ;;
    bun)   exec bun "$WORK/bun.mjs" ;;
  esac
}
export -f run
export WORK ESRUN

declare -A R
[ -x "$ESRUN" ] && R[esrun]="$(measure esrun)" || R[esrun]=$'n/a\tn/a'
have node && R[node]="$(measure node)" || R[node]=$'n/a\tn/a'
have bun  && R[bun]="$(measure bun)"   || R[bun]=$'n/a\tn/a'
have deno && R[deno]="$(measure deno)" || R[deno]=$'n/a\tn/a'

field() { echo "$1" | awk -F'\t' -v n="$2" '{print $n}'; }
row() {
  local out="| $1 |"
  for rt in esrun node bun deno; do out="$out $(field "${R[$rt]}" "$2") |"; done
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
  row "Connection that never sends a handshake, closed after" 1
  row "A connection over a cap of 1 is" 2
  echo
  echo "<sub>esrun $(version esrun) · Node $(version node) · Bun $(version bun) · Deno $(version deno) · $(uname -s) · $(date -u +%Y-%m-%d)</sub>"
)"

if [ "${1:-}" = "--json" ]; then
  for rt in esrun node bun deno; do echo "$rt silent/cap=${R[$rt]}"; done
  exit 0
fi

NEW="$(awk -v table="$TABLE" '
  /BEGIN probe:ws-bounds/ { print; print table; skip = 1; next }
  /END probe:ws-bounds/   { skip = 0 }
  !skip { print }
' "$DOC")"

if [ "${1:-}" = "--check" ]; then
  if [ "$NEW" = "$(cat "$DOC")" ]; then
    echo "the internals page is up to date" >&2
    exit 0
  fi
  echo "the internals page is out of date — run: bash bench/probe-ws-bounds.sh" >&2
  diff <(echo "$NEW") "$DOC" >&2 || true
  exit 1
fi

printf '%s\n' "$NEW" > "$DOC"
echo "updated $DOC" >&2
