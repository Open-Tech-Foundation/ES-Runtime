#!/usr/bin/env bash
# Runs h2spec — the HTTP/2 conformance suite (RFC 7540/9113, RFC 7541) — against
# esrun's HTTP/2 server (DECISIONS D217), over TLS with ALPN h2 and over
# cleartext with prior knowledge.
#
# h2spec is fetched at a pinned release and checked against its checksum before
# it runs; nothing is installed. Over TLS every case must pass. On cleartext one
# case is expected to fail, by design: §3.5/2 sends an invalid preface, and on a
# cleartext port anything that is not the preface is an HTTP/1.1 request —
# answered by the HTTP/1.1 server, which h2spec cannot read as HTTP/2.
#
# Usage:  scripts/h2spec.sh                  (uses target/release/esrun)
#         ESRUN=target/debug/esrun scripts/h2spec.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
ESRUN="$(realpath "${ESRUN:-target/release/esrun}")"
[ -x "$ESRUN" ] || { echo "no esrun at $ESRUN — cargo build --release -p es-runtime-cli" >&2; exit 1; }
VERSION=v2.6.0
SHA256=157ee0de702e01ad40e752dbf074b366027e550c8e7504f9450da2809e279318
PLAIN="${PLAIN_PORT:-18930}"
TLS="${TLS_PORT:-18931}"

# A squatter on either port would be what gets measured.
for port in "$PLAIN" "$TLS"; do
  if (echo > "/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "h2spec.sh: port $port is already in use" >&2
    exit 1
  fi
done

tmp="$(mktemp -d)"
server=""
trap '[ -n "$server" ] && kill "$server" 2>/dev/null; rm -rf "$tmp"' EXIT
curl -sfL -o "$tmp/h2spec.tar.gz" \
  "https://github.com/summerwind/h2spec/releases/download/$VERSION/h2spec_linux_amd64.tar.gz"
echo "$SHA256  $tmp/h2spec.tar.gz" | sha256sum -c --quiet
mkdir "$tmp/bin" "$tmp/site"
tar -xzf "$tmp/h2spec.tar.gz" -C "$tmp/bin"

# A server that reads every body and answers with more than the windows some
# cases set (§6.9.2/2 needs a response longer than its window, or it skips).
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
  -subj /CN=localhost -addext subjectAltName=IP:127.0.0.1 \
  -keyout "$tmp/site/key.pem" -out "$tmp/site/cert.pem" 2>/dev/null
cat > "$tmp/site/server.js" <<JS
import { serve } from "runtime:http";
import { file } from "runtime:fs";
const body = "x".repeat(4096);
const handler = async (request) => {
  await request.arrayBuffer();
  return new Response(body, { headers: { "content-type": "text/plain" } });
};
serve({ hostname: "127.0.0.1", port: $PLAIN }, handler);
serve({
  hostname: "127.0.0.1",
  port: $TLS,
  secureTransport: "on",
  cert: await file("cert.pem").text(),
  key: await file("key.pem").text(),
}, handler);
JS
# esrun's sandbox is the directory it runs in.
(cd "$tmp/site" && exec "$ESRUN" --allow-all server.js) &
server=$!
for _ in $(seq 50); do
  (echo > "/dev/tcp/127.0.0.1/$TLS") 2>/dev/null && break
  sleep 0.1
done

status=0
"$tmp/bin/h2spec" -h 127.0.0.1 -p "$TLS" -t -k -o 3 > "$tmp/tls.txt" 2>&1 || true
echo "TLS (ALPN h2):        $(tail -1 "$tmp/tls.txt")"
grep -q " 0 failed$" "$tmp/tls.txt" || { grep -B2 -A6 "×" "$tmp/tls.txt"; status=1; }

"$tmp/bin/h2spec" -h 127.0.0.1 -p "$PLAIN" -o 3 > "$tmp/plain.txt" 2>&1 || true
echo "cleartext (h2c):      $(tail -1 "$tmp/plain.txt")"
# The failures, once each (h2spec repeats them in a summary at the end).
failed="$(grep "×" "$tmp/plain.txt" | sed 's/^ *//' | sort -u)"
if [ -n "$failed" ] && [ "$failed" != "× 2: Sends invalid connection preface" ]; then
  grep -B2 -A6 "×" "$tmp/plain.txt"
  status=1
fi
[ -n "$failed" ] && echo "                      (§3.5/2 expected: a cleartext non-preface is HTTP/1.1)"
exit $status
