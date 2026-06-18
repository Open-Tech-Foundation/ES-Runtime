#!/usr/bin/env bash
# Bun-style WebSocket "chat" broadcast benchmark runner.
#
# Two sweeps, both reporting messages/sec (steady-state rates → we take the max
# over a few reps as the contention-free ceiling):
#   1. Server sweep — fixed Bun client driver, server ∈ {bun, deno, esrun}. The
#      classic "chat server" number: server fan-out throughput. (Node has no
#      built-in WebSocket server, so it is not in this sweep.)
#   2. Client sweep — fixed Bun server, client driver ∈ {esrun, bun, deno, node}.
#      Each runtime's WebSocket *client* throughput under the same broadcast load.
#
# Knobs: WS_CLIENTS (default 32), WS_WARMUP_MS (1000), WS_MEASURE_MS (3000),
# REPS (3), ESRUN (path to the release binary).
#
# BENCH_JSON=1 emits the RECV (fan-out) msg/s for a C-sweep as a JSON object
# ({ "websocket": { "server": {C:{rt:v}}, "client": {C:{rt:v}} } }) — the form
# bench/gen-bench-data.sh merges into the site's data module. WS_CLIENTS_SWEEP
# (default "32 64 128") sets the C values.
set -u

DIR="$(cd "$(dirname "$0")" && pwd)"
C="${WS_CLIENTS:-32}"
WARMUP="${WS_WARMUP_MS:-1000}"
MEASURE="${WS_MEASURE_MS:-3000}"
REPS="${REPS:-3}"
ESRUN="${ESRUN:-$DIR/../../target/release/esrun}"
DENO="$(command -v deno 2>/dev/null)"
[ -z "$DENO" ] && [ -x "$HOME/.deno/bin/deno" ] && DENO="$HOME/.deno/bin/deno"
PORTN=4001

have() { case "$1" in
  bun)   command -v bun  >/dev/null 2>&1 ;;
  node)  command -v node >/dev/null 2>&1 ;;
  deno)  [ -n "$DENO" ] ;;
  esrun) [ -x "$ESRUN" ] ;;
esac }

client_cmd() { case "$1" in
  esrun) echo "$ESRUN" ;;
  bun)   echo "bun" ;;
  deno)  echo "$DENO run -A --quiet" ;;
  node)  echo "node" ;;
esac }

server_cmd() { case "$1" in
  bun)   echo "bun" ;;
  deno)  echo "$DENO run -A --quiet" ;;
  esrun) echo "$ESRUN" ;;
esac }

server_script() { case "$1" in
  bun)   echo "$DIR/server-bun.js" ;;
  deno)  echo "$DIR/server-deno.js" ;;
  esrun) echo "$DIR/server-esrun.js" ;;
esac }

# Config is injected as globalThis.__WS_* so the same .js runs on every runtime.
prelude() { # port
  printf 'globalThis.__WS_PORT=%s;globalThis.__WS_CLIENTS=%s;globalThis.__WS_WARMUP_MS=%s;globalThis.__WS_MEASURE_MS=%s;\n' \
    "$1" "$C" "$WARMUP" "$MEASURE"
}

# One server+client run → "sent recv" messages/sec (or "ERR ERR"). Each run uses
# a fresh port to dodge TIME_WAIT on rebind.
run_one() { # server_rt client_rt
  local srt="$1" crt="$2" port=$((PORTN++)) tmp stmp out spid up=""
  stmp="$(mktemp --suffix=.mjs)"; { prelude "$port"; cat "$(server_script "$srt")"; } > "$stmp"
  $(server_cmd "$srt") "$stmp" >/dev/null 2>&1 & spid=$!
  for _ in $(seq 60); do (echo > "/dev/tcp/127.0.0.1/$port") 2>/dev/null && { up=1; break; }; sleep 0.1; done
  if [ -z "$up" ]; then kill "$spid" 2>/dev/null; wait "$spid" 2>/dev/null; rm -f "$stmp"; echo "ERR ERR"; return; fi
  tmp="$(mktemp --suffix=.mjs)"; { prelude "$port"; cat "$DIR/client.js"; } > "$tmp"
  out="$($(client_cmd "$crt") "$tmp" 2>/dev/null)"
  kill "$spid" 2>/dev/null; wait "$spid" 2>/dev/null; rm -f "$tmp" "$stmp"
  local sps rps
  sps="$(grep -oE 'MSG_SENT_PER_SEC=[0-9]+' <<<"$out" | head -1 | cut -d= -f2)"
  rps="$(grep -oE 'MSG_RECV_PER_SEC=[0-9]+' <<<"$out" | head -1 | cut -d= -f2)"
  echo "${sps:-ERR} ${rps:-ERR}"
}

# Best (max) sent/recv over REPS reps.
best() { # server_rt client_rt
  local s c bs=0 br=0 r
  for r in $(seq "$REPS"); do
    read -r s c <<<"$(run_one "$1" "$2")"
    [ "$s" = ERR ] && { echo "ERR ERR"; return; }
    [ "$s" -gt "$bs" ] && bs="$s"
    [ "$c" -gt "$br" ] && br="$c"
  done
  echo "$bs $br"
}

# Machine-readable C-sweep for the site data module: RECV (fan-out) msg/s only.
if [ -n "${BENCH_JSON:-}" ]; then
  rows=""
  for C in ${WS_CLIENTS_SWEEP:-32 64 128}; do
    for s in bun deno esrun; do
      if have "$s" && have bun; then read -r _ rps <<<"$(best "$s" bun)"; else rps=ERR; fi
      rows="${rows}server $s $C $rps"$'\n'
    done
    for cl in esrun bun deno node; do
      if have "$cl" && have bun; then read -r _ rps <<<"$(best bun "$cl")"; else rps=ERR; fi
      rows="${rows}client $cl $C $rps"$'\n'
    done
  done
  printf '%s' "$rows" | node -e '
    const fs = require("fs");
    const out = { websocket: { server: {}, client: {} } };
    for (const line of fs.readFileSync(0, "utf8").trim().split("\n")) {
      const [sweep, rt, c, v] = line.split(" ");
      (out.websocket[sweep][c] ??= {})[rt] = v === "ERR" ? null : Number(v);
    }
    process.stdout.write(JSON.stringify(out));
  '
  exit 0
fi

echo "WebSocket chat broadcast benchmark — C=$C clients, ${MEASURE}ms window, best of $REPS"
echo "(SENT = client send/sec; RECV = total deliveries/sec = server fan-out)"
echo

echo "== Server sweep (client driver = bun) =="
printf "%-8s | %16s | %20s\n" "server" "SENT msg/s" "RECV msg/s (fanout)"
printf -- "---------+------------------+----------------------\n"
for s in bun deno esrun; do
  if have "$s" && have bun; then
    read -r sps rps <<<"$(best "$s" bun)"
    printf "%-8s | %16s | %20s\n" "$s" "$sps" "$rps"
  else
    printf "%-8s | %16s | %20s\n" "$s" "n/a" "n/a"
  fi
done
echo

echo "== Client sweep (server = bun) =="
printf "%-8s | %16s | %20s\n" "client" "SENT msg/s" "RECV msg/s (fanout)"
printf -- "---------+------------------+----------------------\n"
for c in esrun bun deno node; do
  if have "$c" && have bun; then
    read -r sps rps <<<"$(best bun "$c")"
    printf "%-8s | %16s | %20s\n" "$c" "$sps" "$rps"
  else
    printf "%-8s | %16s | %20s\n" "$c" "n/a" "n/a"
  fi
done
