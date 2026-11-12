#!/usr/bin/env bash
# Runs the Autobahn WebSocket testsuite — the conformance suite for RFC 6455 —
# against esrun's WebSocket framing (DECISIONS D217), both ways:
#
#   server  Autobahn's fuzzing client against two echo servers: one on
#           runtime:http's upgradeWebSocket, one on runtime:websocket's serve.
#   client  Autobahn's fuzzing server, driven by an echo client on the
#           WebSocket global.
#
# Cases 12 and 13 (permessage-deflate) are excluded: no extension is
# negotiated. Every other case must be OK, NON-STRICT or INFORMATIONAL — never
# FAILED. The image is pinned by digest and nothing is installed. It runs under
# docker or podman (ENGINE= picks one). No bind mounts — Docker Desktop has none
# — so the config goes in on stdin and the reports come out with `cp`. Under
# Docker Desktop the container reaches the host as host.docker.internal, so the
# servers under test bind every interface for the run; elsewhere the container
# shares the host's network.
#
# Usage:  scripts/autobahn.sh               (both; target/release/esrun)
#         scripts/autobahn.sh server
#         ESRUN=target/debug/esrun scripts/autobahn.sh client
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
ESRUN="$(realpath "${ESRUN:-target/release/esrun}")"
[ -x "$ESRUN" ] || { echo "no esrun at $ESRUN — cargo build --release -p es-runtime-cli" >&2; exit 1; }
# crossbario/autobahn-testsuite 25.10.1.
IMAGE=docker.io/crossbario/autobahn-testsuite@sha256:519915fb568b04c9383f70a1c405ae3ff44ab9e35835b085239c258b6fac3074
ENGINE="${ENGINE:-$(command -v docker || command -v podman || true)}"
[ -n "$ENGINE" ] || { echo "autobahn.sh needs docker or podman" >&2; exit 1; }
if "$ENGINE" info --format '{{.OperatingSystem}}' 2>/dev/null | grep -q "Docker Desktop"; then
  BIND=0.0.0.0
  HOST=host.docker.internal
  NET=(--add-host=host.docker.internal:host-gateway)
else
  BIND=127.0.0.1
  HOST=127.0.0.1
  NET=(--network host)
fi
MODE="${1:-both}"
UPGRADE_PORT=19001
SERVE_PORT=19002
FUZZ_PORT=19003

for port in $UPGRADE_PORT $SERVE_PORT $FUZZ_PORT; do
  if (echo > "/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "autobahn.sh: port $port is already in use" >&2
    exit 1
  fi
done

tmp="$(mktemp -d)"
pids=()
containers=()
cleanup() {
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  for c in "${containers[@]}"; do "$ENGINE" rm -f "$c" >/dev/null 2>&1 || true; done
  rm -rf "$tmp"
}
trap cleanup EXIT
mkdir -p "$tmp/site" "$tmp/reports"

# wstest in a named container, its config on stdin (written to /config.json),
# so its reports can be copied out afterwards.
wstest() { # name mode config [engine args...]
  local name="$1" mode="$2" config="$3"
  shift 3
  containers+=("$name")
  "$ENGINE" run -i --name "$name" "${NET[@]}" "$@" "$IMAGE" \
    sh -c "cat > /config.json && exec wstest -m $mode -s /config.json" < "$config"
}

# Tallies Autobahn's verdicts from its index.json and fails on any FAILED.
verdicts() {
  python3 - "$1" <<'PY'
import json, sys
index = json.load(open(sys.argv[1]))
bad = 0
for agent, cases in sorted(index.items()):
    tally = {}
    for case, result in cases.items():
        verdict = result["behavior"]
        close = result.get("behaviorClose", "OK")
        if close == "FAILED":
            verdict = "FAILED"
        tally[verdict] = tally.get(verdict, 0) + 1
        if verdict == "FAILED":
            bad += 1
            print(f"  FAILED {agent} {case}")
    print(f"{agent}: " + ", ".join(f"{n} {v}" for v, n in sorted(tally.items())))
sys.exit(1 if bad else 0)
PY
}

status=0

if [ "$MODE" = both ] || [ "$MODE" = server ]; then
  cat > "$tmp/site/server.js" <<JS
// Echo, on both server paths.
import { serve as http } from "runtime:http";
import { serve, upgradeWebSocket } from "runtime:websocket";
const echo = (socket) => {
  socket.binaryType = "arraybuffer";
  socket.onmessage = (e) => socket.send(e.data);
};
http({ hostname: "$BIND", port: $UPGRADE_PORT }, (request) => {
  const { response, socket } = upgradeWebSocket(request);
  echo(socket);
  return response;
});
const server = serve({ hostname: "$BIND", port: $SERVE_PORT, maxBufferedAmount: 0 });
for await (const socket of server) echo(socket);
JS
  (cd "$tmp/site" && exec "$ESRUN" --allow-all server.js) > "$tmp/esrun-server.log" 2>&1 &
  pids+=($!)
  for _ in $(seq 50); do
    (echo > "/dev/tcp/127.0.0.1/$SERVE_PORT") 2>/dev/null && break
    sleep 0.1
  done
  # One run per agent: Autobahn's multi-server mode stops a later agent short
  # once an earlier one's connections start closing early.
  : > "$tmp/server.log"
  for agent in "esrun-upgrade:$UPGRADE_PORT" "esrun-serve:$SERVE_PORT"; do
    name="${agent%%:*}"
    port="${agent##*:}"
    cat > "$tmp/fuzzingclient.json" <<JSON
{
  "outdir": "/reports/servers",
  "servers": [{ "agent": "$name", "url": "ws://$HOST:$port" }],
  "cases": ["*"],
  "exclude-cases": ["12.*", "13.*"],
  "exclude-agent-cases": {}
}
JSON
    # Autobahn's fuzzing client ends its run without a word when a connection
    # is refused — which Docker Desktop's host gateway does now and then under
    # a burst — so a short run is detected, retried once, and never counted.
    for attempt in 1 2; do
      "$ENGINE" rm -f "autobahn-$name" >/dev/null 2>&1 || true
      wstest "autobahn-$name" fuzzingclient "$tmp/fuzzingclient.json" > "$tmp/$name.log" 2>&1 || true
      rm -rf "$tmp/reports/$name" && mkdir -p "$tmp/reports/$name"
      "$ENGINE" cp "autobahn-$name:/reports/servers/." "$tmp/reports/$name" >/dev/null
      want="$(sed -n 's/^Ok, will run \([0-9]*\) test cases.*/\1/p' "$tmp/$name.log")"
      ran="$(grep -c '^Running test case' "$tmp/$name.log" || true)"
      [ -n "$want" ] && [ "$ran" = "$want" ] && break
      echo "$name: Autobahn stopped after $ran of ${want:-?} cases (attempt $attempt)" >&2
    done
    cat "$tmp/$name.log" >> "$tmp/server.log"
    if [ -z "$want" ] || [ "$ran" != "$want" ]; then
      echo "$name: incomplete run — not a result" >&2
      status=1
    fi
  done
  echo "server (Autobahn fuzzing client → esrun):"
  for name in esrun-upgrade esrun-serve; do
    verdicts "$tmp/reports/$name/index.json" || status=1
  done
fi

if [ "$MODE" = both ] || [ "$MODE" = client ]; then
  cat > "$tmp/fuzzingserver.json" <<JSON
{
  "url": "ws://0.0.0.0:$FUZZ_PORT",
  "outdir": "/reports/clients",
  "cases": ["*"],
  "exclude-cases": ["12.*", "13.*"],
  "exclude-agent-cases": {}
}
JSON
  # The container first, then wstest inside it: cleanup always has a
  # container to remove, and readiness is the server answering, not the
  # published port (which accepts before anything inside listens).
  containers+=(autobahn-fuzzingserver)
  "$ENGINE" run -d --name autobahn-fuzzingserver -p "127.0.0.1:$FUZZ_PORT:$FUZZ_PORT" \
    --entrypoint sleep "$IMAGE" infinity >/dev/null
  "$ENGINE" exec -i autobahn-fuzzingserver sh -c "cat > /config.json" < "$tmp/fuzzingserver.json"
  "$ENGINE" exec -d autobahn-fuzzingserver sh -c "wstest -m fuzzingserver -s /config.json > /wstest.log 2>&1"
  for _ in $(seq 150); do
    curl -sf -o /dev/null "http://127.0.0.1:$FUZZ_PORT/" && break
    sleep 0.2
  done
  cat > "$tmp/site/client.js" <<JS
// Echo every case Autobahn's fuzzing server runs, then ask for the report.
const base = "ws://127.0.0.1:$FUZZ_PORT";
const open = (path, onmessage) =>
  new Promise((done) => {
    const ws = new WebSocket(base + path);
    ws.binaryType = "arraybuffer";
    if (onmessage) ws.onmessage = (e) => onmessage(ws, e);
    ws.onclose = () => done();
    ws.onerror = () => {};
  });
let count = 0;
await open("/getCaseCount", (_, e) => (count = Number(e.data)));
for (let n = 1; n <= count; n++) {
  await open(\`/runCase?case=\${n}&agent=esrun\`, (ws, e) => ws.send(e.data));
}
await open("/updateReports?agent=esrun");
JS
  (cd "$tmp/site" && "$ESRUN" --allow-all client.js) > "$tmp/client.log" 2>&1 || true
  "$ENGINE" cp autobahn-fuzzingserver:/reports/clients "$tmp/reports/clients" >/dev/null
  echo "client (esrun → Autobahn fuzzing server):"
  verdicts "$tmp/reports/clients/index.json" || status=1
fi

if [ -n "${KEEP_REPORTS:-}" ]; then
  rm -rf "$KEEP_REPORTS" && cp -r "$tmp/reports" "$KEEP_REPORTS"
  cp "$tmp"/*.log "$KEEP_REPORTS"/ 2>/dev/null || true
  echo "reports: $KEEP_REPORTS"
fi
exit $status
