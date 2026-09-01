#!/usr/bin/env bash
# Stands up a Sentinel deployment — one master, one replica, three sentinels —
# and prints the environment the sentinel test wants.
#
#   eval "$(test/sentinel-server.sh)" && test/run.sh
#   docker rm -f esrun-redis-sentinel      # when you are done
#
# One container, for the same reason the cluster script uses one: a sentinel
# gossips the master's address to clients, so the address it knows has to be
# reachable from inside (sentinel → master) and outside (client → master) at
# once. On a shared loopback, 127.0.0.1 is both.
#
# Three sentinels rather than one, because quorum is the thing worth testing:
# a single sentinel can announce a failover it has not agreed with anyone about.
set -euo pipefail

name="${REDIS_SENTINEL_CONTAINER:-esrun-redis-sentinel}"
docker rm -f "$name" >/dev/null 2>&1 || true

docker run -d --name "$name" \
  -p 7201-7202:7201-7202 -p 7301-7303:7301-7303 \
  redis:8 sh -c '
    redis-server --port 7201 --appendonly no --daemonize yes
    redis-server --port 7202 --appendonly no --replicaof 127.0.0.1 7201 --daemonize yes
    sleep 1
    for s in 7301 7302 7303; do
      conf="/tmp/sentinel-$s.conf"
      {
        echo "port $s"
        echo "sentinel monitor mymaster 127.0.0.1 7201 2"
        echo "sentinel down-after-milliseconds mymaster 1000"
        echo "sentinel failover-timeout mymaster 5000"
        echo "sentinel resolve-hostnames yes"
      } > "$conf"
      redis-server "$conf" --sentinel --daemonize yes
    done
    exec tail -f /dev/null
  ' >/dev/null

# Ready means a sentinel can name the master, not merely that a port answers.
#
# The probe's stdout goes into `grep` rather than through, for the reason the
# cluster script gives: this output is `eval`ed or appended to $GITHUB_ENV.
ready=
for _ in $(seq 1 120); do
  if docker exec "$name" redis-cli -p 7301 sentinel get-master-addr-by-name mymaster 2>/dev/null | grep -q 7201; then
    ready=1
    break
  fi
  sleep 0.5
done
if [ -z "$ready" ]; then
  echo "no sentinel could name the master" >&2
  docker logs "$name" >&2 2>&1 || true
  exit 1
fi

sentinels='redis://127.0.0.1:7301,redis://127.0.0.1:7302,redis://127.0.0.1:7303'
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "REDIS_SENTINELS=$sentinels"
  echo "REDIS_SENTINEL_CONTAINER=$name"
else
  echo "export REDIS_SENTINELS='$sentinels'"
  echo "export REDIS_SENTINEL_CONTAINER='$name'"
fi
