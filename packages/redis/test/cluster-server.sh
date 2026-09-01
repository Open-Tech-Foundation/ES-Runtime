#!/usr/bin/env bash
# Stands up a three-primary Redis cluster and prints the environment the cluster
# test wants.
#
#   eval "$(test/cluster-server.sh)" && test/run.sh
#   docker rm -f esrun-redis-cluster        # when you are done
#
# All three nodes live in **one** container, which is not laziness. A cluster
# node advertises an address that both the other nodes and the client have to be
# able to reach, and `--cluster-announce-ip` can only be one value. With every
# node on the same loopback, `127.0.0.1` is true inside the container (for the
# cluster bus) and true outside it (for the client, through published ports).
# Separate containers force those two to disagree.
set -euo pipefail

name="${REDIS_CLUSTER_CONTAINER:-esrun-redis-cluster}"
docker rm -f "$name" >/dev/null 2>&1 || true

docker run -d --name "$name" \
  -p 7001-7003:7001-7003 -p 17001-17003:17001-17003 \
  redis:8 sh -c '
    for p in 7001 7002 7003; do
      redis-server --port "$p" --cluster-enabled yes \
        --cluster-config-file "nodes-$p.conf" --cluster-node-timeout 5000 \
        --cluster-announce-ip 127.0.0.1 --appendonly no --daemonize yes
    done
    sleep 2
    redis-cli --cluster create 127.0.0.1:7001 127.0.0.1:7002 127.0.0.1:7003 --cluster-yes
    exec tail -f /dev/null
  ' >/dev/null

# Ready means every slot is covered, not merely that a port answers: a client
# connecting mid-formation would read a topology the cluster is about to change.
#
# The probe's stdout goes into `grep` rather than through: this script's output
# is `eval`ed or appended to $GITHUB_ENV, so a docker diagnostic reaching stdout
# is executed or written into the job's environment.
ready=
for _ in $(seq 1 120); do
  if docker exec "$name" redis-cli -p 7001 cluster info 2>/dev/null | grep -q "cluster_state:ok"; then
    ready=1
    break
  fi
  sleep 0.5
done
if [ -z "$ready" ]; then
  echo "the cluster never reached cluster_state:ok" >&2
  docker logs "$name" >&2 2>&1 || true
  exit 1
fi

urls='redis://127.0.0.1:7001,redis://127.0.0.1:7002,redis://127.0.0.1:7003'
# Both spellings: `eval` wants the export, and $GITHUB_ENV wants a bare
# assignment. Printing the bare one when GITHUB_ENV is set keeps one script
# usable from a shell and from CI without a wrapper.
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "REDIS_CLUSTER_URLS=$urls"
else
  echo "export REDIS_CLUSTER_URLS='$urls'"
fi
