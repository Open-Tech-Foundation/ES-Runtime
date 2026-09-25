#!/usr/bin/env bash
# Stands up a MySQL with a certificate from a *private* authority — what an
# internal deployment looks like, and what neither the public roots nor MySQL's
# own self-signed certificate can stand in for. Prints the environment the TLS
# test wants.
#
#   eval "$(test/tls-server.sh)" && test/run.sh
#   docker rm -f esrun-mysql-tls          # when you are done
set -euo pipefail

name="${MYSQL_TLS_CONTAINER:-esrun-mysql-tls}"
port="${MYSQL_TLS_PORT:-3308}"
dir="$(mktemp -d)"

# A CA, and a server certificate it signs. The SAN matters: rustls checks the
# hostname, and a certificate without one is refused however well it verifies.
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$dir/ca.key" -out "$dir/ca.pem" \
  -days 2 -subj "/CN=esrun-test-ca" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -keyout "$dir/server-key.pem" -out "$dir/server.csr" \
  -subj "/CN=localhost" 2>/dev/null
openssl x509 -req -in "$dir/server.csr" -CA "$dir/ca.pem" -CAkey "$dir/ca.key" \
  -CAcreateserial -out "$dir/server-cert.pem" -days 2 \
  -extfile <(printf "subjectAltName=DNS:localhost,IP:127.0.0.1") 2>/dev/null
# Readable by mysqld, which is not the user that minted them — and the directory
# too, since mktemp makes it 0700 and docker cp keeps that.
chmod 644 "$dir"/*.pem
chmod 755 "$dir"

# Created, filled, then started — rather than a bind mount, which Docker Desktop
# refuses for paths it does not share. mysqld reads the key at startup, so the
# files have to be in place before it runs.
docker rm -f "$name" >/dev/null 2>&1 || true
docker create --name "$name" \
  -e MYSQL_ROOT_PASSWORD=esrun -e MYSQL_DATABASE=esrun_test \
  -p "127.0.0.1:$port:3306" mysql:8.4 \
  --ssl-ca=/certs/ca.pem --ssl-cert=/certs/server-cert.pem --ssl-key=/certs/server-key.pem \
  --require-secure-transport=ON >/dev/null
docker cp "$dir/." "$name:/certs" >/dev/null
docker start "$name" >/dev/null

# Over TCP, not the socket: the image's entrypoint runs a temporary server with
# networking off while it initialises, then restarts it — only the real one
# listens on the port.
until docker exec "$name" mysql -uroot -pesrun --protocol=TCP -h127.0.0.1 -e "SELECT 1" >/dev/null 2>&1; do
  sleep 1
done

echo "export MYSQL_TLS_URL='mysql://root:esrun@localhost:$port/esrun_test'"
echo "export MYSQL_CA='$(cat "$dir/ca.pem")'"
