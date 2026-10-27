#!/usr/bin/env bash
# Starts a three-member replica set, rs0, for the replica-set tests, and prints
# the URL to point MONGODB_RS_URL at.
#
# One container runs all three mongods on ports 27031-27033, configured as
# localhost:<port> and published on the same ports: the members reach each other
# inside the container, and the driver reaches the very same addresses from the
# host. Each member is tagged with a data center, `a` or `b`, for the tag-set
# tests, and test commands are enabled for `sleep` and fail points.
#
#   test/replica-set.sh [mongo image tag, default 8]
set -euo pipefail

tag="${1:-8}"
name="esrun-mongodb-rs"
docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" -p 27031-27033:27031-27033 "mongo:$tag" bash -c '
  for p in 27031 27032 27033; do
    mkdir -p /data/$p
    mongod --replSet rs0 --port $p --dbpath /data/$p --bind_ip_all \
      --setParameter enableTestCommands=1 --fork --logpath /data/$p.log
  done
  tail -f /dev/null' >/dev/null

for p in 27031 27032 27033; do
  for _ in $(seq 60); do
    docker exec "$name" mongosh --quiet --port $p --eval "db.runCommand({ ping: 1 }).ok" >/dev/null 2>&1 && break
    sleep 1
  done
done
docker exec "$name" mongosh --quiet --port 27031 --eval '
  rs.initiate({ _id: "rs0", members: [
    { _id: 0, host: "localhost:27031", priority: 2, tags: { dc: "a" } },
    { _id: 1, host: "localhost:27032", tags: { dc: "b" } },
    { _id: 2, host: "localhost:27033", tags: { dc: "b" } },
  ] })' >/dev/null
for _ in $(seq 60); do
  state=$(docker exec "$name" mongosh --quiet --port 27031 --eval \
    'const s = rs.status().members.map(m => m.stateStr); s.filter(x => x === "PRIMARY").length + "/" + s.filter(x => x === "SECONDARY").length' 2>/dev/null || true)
  # An elected primary and two secondaries.
  [ "$state" = "1/2" ] && break
  sleep 1
done
echo "mongodb://localhost:27031/esrun_test?replicaSet=rs0"
