#!/usr/bin/env bash
#
# esrun against esrun as an HTTP/2 *client*: two or more builds running the
# same fetch workloads against one fixed server — Node's `http2` secure server,
# so the server is neither build's and only the client differs. Reports, per
# build and workload, the best wall time and the client's user+system CPU per
# request.
#
#   seq    $SEQ GETs, one after another              latency of one exchange
#   par    $PAR GETs, $WIDTH at a time               multiplexing on one connection
#   post   $POSTS POSTs of 64 KiB, $WIDTH at a time  request bodies under flow control
#
# Methodology is http2.sh's: server and client on separate halves of the cores,
# every repetition runs each build once in a shuffled order, a discarded warmup
# repetition, best of $REPS.
#
# Usage (from bench/, builds in ../target/release; needs node and openssl):
#   BINS="esrun-before esrun-after" bench/compare-fetch-h2.sh
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
: "${BINS:?name the builds to compare: BINS=\"esrun-a esrun-b\" (in ../target/release)}"
BIN_DIR="$(realpath "${BIN_DIR:-../target/release}")"
PORT="${PORT:-18950}"
REPS="${REPS:-4}"
SEQ="${SEQ:-3000}"
PAR="${PAR:-30000}"
POSTS="${POSTS:-3000}"
WIDTH="${WIDTH:-100}"
for bin in $BINS; do
  [ -x "$BIN_DIR/$bin" ] || { echo "no build at $BIN_DIR/$bin" >&2; exit 1; }
done
command -v node >/dev/null || { echo "compare-fetch-h2.sh needs node" >&2; exit 1; }
if (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then
  echo "compare-fetch-h2.sh: port $PORT is already in use" >&2
  exit 1
fi

NCPU="$(nproc)"
half=$((NCPU / 2))
SERVER_PIN=(taskset -c "0-$((half - 1))")
CLIENT_PIN=(taskset -c "$half-$((NCPU - 1))")

# esrun's sandbox is the directory it runs in, so the client script and the CA
# it trusts live in a directory of their own under bench/.
work="$(mktemp -d "$PWD/.fetch-h2-XXXXXX")"
server=""
trap '[ -n "$server" ] && kill "$server" 2>/dev/null; rm -rf "$work"' EXIT
# A CA, and a server certificate it signs: webpki refuses a CA certificate
# presented as the server's own, which is what one self-signed cert would be.
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
  -subj /CN=bench-ca -keyout "$work/ca-key.pem" -out "$work/ca.pem" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=localhost \
  -keyout "$work/key.pem" -out "$work/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:localhost\nbasicConstraints=critical,CA:FALSE\n' > "$work/ext.cnf"
openssl x509 -req -in "$work/server.csr" -CA "$work/ca.pem" -CAkey "$work/ca-key.pem" \
  -CAcreateserial -days 1 -extfile "$work/ext.cnf" -out "$work/cert.pem" 2>/dev/null

cat > "$work/server.cjs" <<JS
const http2 = require("node:http2");
const fs = require("node:fs");
const server = http2.createSecureServer({
  cert: fs.readFileSync("$work/cert.pem"),
  key: fs.readFileSync("$work/key.pem"),
  settings: { maxConcurrentStreams: 1000 },
});
server.on("stream", (stream, headers) => {
  if (headers[":method"] === "POST") {
    let n = 0;
    stream.on("data", (c) => (n += c.length));
    stream.on("end", () => {
      stream.respond({ ":status": 200, "content-type": "text/plain" });
      stream.end(String(n));
    });
  } else {
    stream.respond({ ":status": 200, "content-type": "text/plain" });
    stream.end("Hello, World!");
  }
});
server.listen($PORT, "127.0.0.1");
JS
cat > "$work/client.mjs" <<JS
const url = "https://localhost:$PORT/";
const [workload] = (await import("runtime:process")).args;
const t0 = performance.now();
if (workload === "seq") {
  for (let i = 0; i < $SEQ; i++) await (await fetch(url)).text();
} else if (workload === "par") {
  for (let i = 0; i < $PAR; i += $WIDTH) {
    await Promise.all(Array.from({ length: $WIDTH }, async () => (await fetch(url)).text()));
  }
} else {
  const body = new Uint8Array(64 * 1024);
  for (let i = 0; i < $POSTS; i += $WIDTH) {
    await Promise.all(Array.from({ length: $WIDTH }, async () =>
      (await fetch(url, { method: "POST", body })).text()));
  }
}
console.log(\`MS=\${(performance.now() - t0).toFixed(1)}\`);
JS

"${SERVER_PIN[@]}" node "$work/server.cjs" &
server=$!
for _ in $(seq 50); do (echo > "/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break; sleep 0.1; done

# run <build> <workload> → "<ms> <µs/req>" or ERR
run() {
  local n
  case "$2" in seq) n=$SEQ ;; par) n=$PAR ;; post) n=$POSTS ;; esac
  local out
  # The client's own CPU, from getrusage on the child: no GNU time needed.
  out="$(cd "$work" && python3 -c '
import resource, subprocess, sys
subprocess.run(sys.argv[1:])
r = resource.getrusage(resource.RUSAGE_CHILDREN)
print(f"CPU={r.ru_utime} {r.ru_stime}")
' "${CLIENT_PIN[@]}" "$BIN_DIR/$1" --allow-all --tls-ca=ca.pem client.mjs "$2" 2>&1)"
  local ms cpu
  ms="$(sed -n 's/^MS=//p' <<<"$out")"
  cpu="$(sed -n 's/^CPU=//p' <<<"$out" | awk '{print ($1 + $2) * 1e6}')"
  [ -n "$ms" ] && [ -n "$cpu" ] || { echo ERR; return; }
  awk -v ms="$ms" -v cpu="$cpu" -v n="$n" 'BEGIN { printf "%s %.2f\n", ms, cpu / n }'
}

declare -A BEST CPU
WORKLOADS=(seq par post)
for rep in $(seq 0 "$REPS"); do
  for bin in $(printf "%s\n" $BINS | shuf); do
    for w in "${WORKLOADS[@]}"; do
      result="$(run "$bin" "$w")"
      [ "$rep" -eq 0 ] && continue
      read -r ms us <<<"$result"
      [ "$ms" = ERR ] && continue
      if [ -z "${BEST[$bin $w]:-}" ] || awk -v a="$ms" -v b="${BEST[$bin $w]}" 'BEGIN { exit !(a < b) }'; then
        BEST[$bin $w]="$ms"
        CPU[$bin $w]="$us"
      fi
    done
  done
done

printf "%-24s %-6s %10s %10s\n" build work "ms" "µs/req"
for bin in $BINS; do
  for w in "${WORKLOADS[@]}"; do
    printf "%-24s %-6s %10s %10s\n" "$bin" "$w" "${BEST[$bin $w]:-ERR}" "${CPU[$bin $w]:-}"
  done
done
