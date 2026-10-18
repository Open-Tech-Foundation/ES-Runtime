#!/usr/bin/env bash
# Stands up a RabbitMQ and prints the environment e2e.mjs wants.
#
#   eval "$(test/rabbitmq-server.sh)" && test/run.sh
#   docker rm -f esrun-rabbitmq                 # when you are done
#
# The user is not `guest`, because a published port makes the broker see the
# Docker gateway rather than loopback, and `guest` is refused from anywhere but
# loopback. And every `docker exec` runs as `rabbitmq`, because the default root
# makes the command's own Erlang node write a root-owned `.erlang.cookie` over
# the server's, which the server then cannot read (`eacces`, at startup).
set -euo pipefail

name="${RABBITMQ_NAME:-esrun-rabbitmq}"
port="${RABBITMQ_PORT:-5672}"
user="${RABBITMQ_USER:-esrun}"
pass="${RABBITMQ_PASS:-esrun}"

docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" -p "127.0.0.1:$port:5672" rabbitmq:4-alpine >/dev/null

until docker exec -u rabbitmq "$name" rabbitmq-diagnostics -q check_running >/dev/null 2>&1; do
  [ "$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null)" = "true" ] || {
    echo "rabbitmq exited during startup" >&2
    exit 1
  }
  sleep 0.5
done

docker exec -u rabbitmq "$name" rabbitmqctl add_user "$user" "$pass" >/dev/null
docker exec -u rabbitmq "$name" rabbitmqctl set_permissions -p / "$user" ".*" ".*" ".*" >/dev/null

echo "export RABBITMQ_URL='amqp://$user:$pass@127.0.0.1:$port/'"
