#!/usr/bin/env bash
# Stands up two Mailpit servers with a certificate from a *private* authority —
# one that requires STARTTLS, one that requires TLS from the first byte — each
# with a real password file, and prints the environment mailpit.mjs wants.
#
#   eval "$(test/mailpit-server.sh)" && test/run.sh
#   docker rm -f esrun-smtp-starttls esrun-smtp-tls     # when you are done
set -euo pipefail

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
printf 'app:s3cret\n' > "$dir/passwords"
chmod 644 "$dir"/*
chmod 755 "$dir"

start() { # name smtp-port api-port mode-flag
  docker rm -f "$1" >/dev/null 2>&1 || true
  # Created, filled, then started: a bind mount is refused by Docker Desktop
  # for paths it does not share.
  docker create --name "$1" -p "127.0.0.1:$2:1025" -p "127.0.0.1:$3:8025" axllent/mailpit \
    --smtp-tls-cert /certs/server-cert.pem --smtp-tls-key /certs/server-key.pem \
    --smtp-auth-file /certs/passwords "$4" >/dev/null
  docker cp "$dir/." "$1:/certs" >/dev/null
  docker start "$1" >/dev/null
  until curl -fs "http://127.0.0.1:$3/api/v1/info" >/dev/null 2>&1; do sleep 0.5; done
}

start esrun-smtp-starttls 1587 18025 --smtp-require-starttls
start esrun-smtp-tls 1465 18026 --smtp-require-tls

echo "export SMTP_STARTTLS='localhost:1587' SMTP_STARTTLS_API='http://127.0.0.1:18025'"
echo "export SMTP_TLS='localhost:1465' SMTP_TLS_API='http://127.0.0.1:18026'"
echo "export SMTP_CA='$(cat "$dir/ca.pem")'"
