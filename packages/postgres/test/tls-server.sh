#!/usr/bin/env bash
# Stands up a PostgreSQL with a certificate from a *private* authority, which is
# what an internal deployment looks like and what the public roots have never
# heard of. Prints the environment the TLS test wants.
#
#   eval "$(test/tls-server.sh)" && test/run.sh
#   docker rm -f esrun-pg-tls          # when you are done
set -euo pipefail

name="${PG_TLS_CONTAINER:-esrun-pg-tls}"
port="${PG_TLS_PORT:-5434}"
dir="$(mktemp -d)"

# A CA, and a server certificate it signs. The SAN matters: rustls checks the
# hostname, and a certificate without one is refused however well it verifies.
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$dir/ca.key" -out "$dir/ca.crt" \
  -days 2 -subj "/CN=esrun-test-ca" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -keyout "$dir/server.key" -out "$dir/server.csr" \
  -subj "/CN=localhost" 2>/dev/null
openssl x509 -req -in "$dir/server.csr" -CA "$dir/ca.crt" -CAkey "$dir/ca.key" \
  -CAcreateserial -out "$dir/server.crt" -days 2 \
  -extfile <(printf "subjectAltName=DNS:localhost,IP:127.0.0.1") 2>/dev/null

docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" \
  -e POSTGRES_PASSWORD=esrun -e POSTGRES_DB=esrun_test \
  -p "127.0.0.1:$port:5432" postgres:latest >/dev/null

# The server has to start before the key can be installed, because PostgreSQL
# insists the key is unreadable by anyone but its owner or group — which means
# fixing ownership *inside* the container, which means the container is running.
until docker exec -u postgres "$name" pg_isready -q 2>/dev/null; do sleep 0.5; done
docker cp "$dir/server.crt" "$name:/var/lib/postgresql/server.crt" >/dev/null
docker cp "$dir/server.key" "$name:/var/lib/postgresql/server.key" >/dev/null
docker exec -u root "$name" sh -c '
  chown root:postgres /var/lib/postgresql/server.key /var/lib/postgresql/server.crt
  chmod 640 /var/lib/postgresql/server.key
  chmod 644 /var/lib/postgresql/server.crt'
for setting in \
  "ssl = on" \
  "ssl_cert_file='/var/lib/postgresql/server.crt'" \
  "ssl_key_file='/var/lib/postgresql/server.key'"
do
  # One statement per call: ALTER SYSTEM cannot run inside a transaction block,
  # and psql wraps a multi-statement -c in one.
  docker exec -u postgres "$name" psql -q -c "ALTER SYSTEM SET $setting"
done
docker restart "$name" >/dev/null
until docker exec -u postgres "$name" pg_isready -q 2>/dev/null; do sleep 0.5; done

echo "export PG_TLS_URL='postgres://postgres:esrun@localhost:$port/esrun_test'"
echo "export PG_CA='$(cat "$dir/ca.crt")'"
