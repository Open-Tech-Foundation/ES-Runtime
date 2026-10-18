#!/usr/bin/env bash
# Stands up a Mosquitto and prints the environment e2e.mjs wants.
#
#   eval "$(test/mosquitto-server.sh)" && test/run.sh
#   docker rm -f esrun-mosquitto              # when you are done
#
# Mosquitto 2 will not open a listener off loopback or accept an anonymous
# client without a config that says so. Rather than mount that config (a host
# path the container runtime may refuse to share), it is written into the
# container through an entrypoint shell. When `openssl` is available a second,
# TLS listener is added on 8883 with a self-signed certificate carrying
# `127.0.0.1` as a subject alternative name (the SAN is what the client checks;
# the common name is not enough), and `MQTT_TLS_URL`/`MQTT_CA` are printed too.
set -euo pipefail

name="${MQTT_NAME:-esrun-mosquitto}"
port="${MQTT_PORT:-1883}"
tls_port="${MQTT_TLS_PORT:-8883}"
image="${MQTT_IMAGE:-eclipse-mosquitto:2}"

dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT

conf="listener $port
allow_anonymous true"

tls_env=""
if command -v openssl >/dev/null 2>&1; then
  # A private CA signs a server certificate: rustls refuses a self-signed leaf
  # (`CaUsedAsEndEntity`), so the leaf must chain to a CA the test trusts.
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
  conf="$conf

listener $tls_port
allow_anonymous true
certfile /mosquitto/config/cert.pem
keyfile /mosquitto/config/key.pem"
  tls_env="export MQTT_TLS_URL='mqtts://127.0.0.1:$tls_port'
export MQTT_CA='$(cat "$dir/ca.pem")'"
fi

setup="cat > /mosquitto/config/mosquitto.conf <<'ESRUN_CONF'
$conf
ESRUN_CONF
"
if [ -n "$tls_env" ]; then
  setup="$setup
cat > /mosquitto/config/cert.pem <<'ESRUN_CERT'
$(cat "$dir/cert.pem")
ESRUN_CERT
cat > /mosquitto/config/key.pem <<'ESRUN_KEY'
$(cat "$dir/key.pem")
ESRUN_KEY
"
fi
setup="$setup
exec /usr/sbin/mosquitto -c /mosquitto/config/mosquitto.conf"

docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" --entrypoint sh \
  -p "127.0.0.1:$port:$port" \
  -p "127.0.0.1:$tls_port:$tls_port" \
  "$image" -c "$setup" >/dev/null

# Readiness: the listener answering on its port is the thing the tests need.
for _ in $(seq 1 60); do
  if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    exec 3>&-
    break
  fi
  [ "$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null)" = "true" ] || {
    echo "mosquitto exited during startup" >&2
    docker logs "$name" >&2
    exit 1
  }
  sleep 0.5
done

echo "export MQTT_URL='mqtt://127.0.0.1:$port'"
[ -n "$tls_env" ] && echo "$tls_env"
exit 0
