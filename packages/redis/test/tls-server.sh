#!/usr/bin/env bash
# Stands up a Redis with a certificate from a *private* authority, which is what
# an internal deployment looks like and what the public roots have never heard
# of. Prints the environment the TLS test wants.
#
#   eval "$(test/tls-server.sh)" && test/run.sh
#   docker rm -f esrun-redis-tls       # when you are done
set -euo pipefail

name="${REDIS_TLS_CONTAINER:-esrun-redis-tls}"
port="${REDIS_TLS_PORT:-6390}"
# Docker refuses to mount paths it does not consider shared, and /tmp is
# commonly one of those. A directory under $HOME is the portable choice.
dir="$(mktemp -d "${REDIS_TLS_CERTS:-$HOME/.cache}/esrun-redis-tls.XXXXXX")"

# A CA, and a server certificate it signs. The SAN matters: rustls checks the
# hostname, and a certificate without one is refused however well it verifies.
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$dir/ca.key" -out "$dir/ca.crt" \
  -days 2 -subj "/CN=esrun-test-ca" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -keyout "$dir/server.key" -out "$dir/server.csr" \
  -subj "/CN=localhost" 2>/dev/null
openssl x509 -req -in "$dir/server.csr" -CA "$dir/ca.crt" -CAkey "$dir/ca.key" \
  -CAcreateserial -out "$dir/server.crt" -days 2 \
  -extfile <(printf "subjectAltName=DNS:localhost,IP:127.0.0.1") 2>/dev/null
chmod 644 "$dir"/*.key "$dir"/*.crt

docker rm -f "$name" >/dev/null 2>&1 || true
# `--tls-port` with `--port 0` means TLS only: a test that could fall back to
# the plaintext port would not be testing TLS.
docker run -d --name "$name" -v "$dir:/certs:ro" \
  -p "127.0.0.1:$port:$port" redis:latest \
  redis-server --port 0 --tls-port "$port" \
  --tls-cert-file /certs/server.crt \
  --tls-key-file /certs/server.key \
  --tls-ca-cert-file /certs/ca.crt \
  --tls-auth-clients no >/dev/null

# The port answering is the only readiness signal that means anything here:
# `redis-cli ping` would need the TLS flags too.
for _ in $(seq 1 60); do
  if docker exec "$name" sh -c "exec 3<>/dev/tcp/127.0.0.1/$port" 2>/dev/null; then break; fi
  sleep 0.5
done

echo "export REDIS_TLS_URL='rediss://localhost:$port'"
echo "export REDIS_CA='$(cat "$dir/ca.crt")'"
