#!/usr/bin/env bash
# Stands up a NATS and prints the environment e2e.mjs wants.
#
#   eval "$(test/nats-server.sh)" && test/run.sh
#   docker rm -f esrun-nats esrun-nats-tls   # when you are done
#
# A plain server needs no config: the official image accepts anonymous clients
# on 4222. When `openssl` is available a second server is started on 4223 with a
# TLS listener. The image is built from scratch — no `sh`, and a host directory
# cannot be mounted — so the config and certificates are copied in with
# `docker cp` against a created-but-stopped container, which is why the paths are
# absolute (the root directory always exists to copy into). A private CA signs
# the leaf because rustls refuses a self-signed end-entity certificate. The leaf
# carries `127.0.0.1` and `localhost` as subject alternative names — the SAN is
# what the client checks; the common name is not enough.
set -euo pipefail

name="${NATS_NAME:-esrun-nats}"
port="${NATS_PORT:-4222}"
tls_name="${NATS_TLS_NAME:-esrun-nats-tls}"
tls_port="${NATS_TLS_PORT:-4223}"
image="${NATS_IMAGE:-nats:2}"

dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT

docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" -p "127.0.0.1:$port:$port" "$image" >/dev/null

tls_env=""
if command -v openssl >/dev/null 2>&1; then
  openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
    -keyout "$dir/ca-key.pem" -out "$dir/ca.pem" \
    -subj "/CN=ES-Run e2e CA" >/dev/null 2>&1
  openssl req -newkey rsa:2048 -nodes \
    -keyout "$dir/key.pem" -out "$dir/csr.pem" \
    -subj "/CN=localhost" >/dev/null 2>&1
  cat >"$dir/ext.cnf" <<EOF
subjectAltName=IP:127.0.0.1,DNS:localhost
extendedKeyUsage=serverAuth
EOF
  openssl x509 -req -in "$dir/csr.pem" -CA "$dir/ca.pem" -CAkey "$dir/ca-key.pem" \
    -CAcreateserial -days 2 -out "$dir/cert.pem" -extfile "$dir/ext.cnf" >/dev/null 2>&1

  docker rm -f "$tls_name" >/dev/null 2>&1 || true
  cat >"$dir/nats.conf" <<'ESRUN_CONF'
port: 4222
tls {
  cert_file: "/cert.pem"
  key_file: "/key.pem"
}
ESRUN_CONF

  docker create --name "$tls_name" \
    -p "127.0.0.1:$tls_port:4222" "$image" -c /nats.conf >/dev/null
  docker cp "$dir/nats.conf" "$tls_name:/nats.conf"
  docker cp "$dir/cert.pem" "$tls_name:/cert.pem"
  docker cp "$dir/key.pem" "$tls_name:/key.pem"
  docker start "$tls_name" >/dev/null
  tls_env="export NATS_TLS_URL='tls://127.0.0.1:$tls_port'
export NATS_CA='$(cat "$dir/ca.pem")'"
fi

# Readiness: a listener answering on its port is the thing the tests need.
wait_for_port() {
  local what="$1" where="$2"
  for _ in $(seq 1 60); do
    if (exec 3<>"/dev/tcp/127.0.0.1/$where") 2>/dev/null; then
      exec 3>&-
      return 0
    fi
    sleep 0.5
  done
  echo "$what did not open port $where" >&2
  return 1
}
wait_for_port "nats" "$port"
[ -n "$tls_env" ] && wait_for_port "nats (tls)" "$tls_port"

echo "export NATS_URL='nats://127.0.0.1:$port'"
[ -n "$tls_env" ] && echo "$tls_env"
exit 0
