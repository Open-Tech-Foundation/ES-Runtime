#!/usr/bin/env bash
# Starts the replica set MongoDB's seedlist discovery tests expect, for srv.mjs:
# repl0, three members on localhost:27017-27019, with TLS. The tests reach it
# through public DNS records under test.build.10gen.cc, which point to
# localhost.test.build.10gen.cc (127.0.0.1) on those ports.
#
# TLS is preferred rather than required: one test turns it off with ssl=false
# and still expects a ping.
#
# The certificate comes from a CA made here, valid for those names, and is
# copied into the container rather than mounted, so this works where bind
# mounts do not (Docker Desktop). Prints the CA's path, for MONGODB_SRV_CA.
#
#   test/srv-replica-set.sh [mongo image tag, default 8]
set -euo pipefail

tag="${1:-8}"
name="esrun-mongodb-srv"
certs="${TMPDIR:-/tmp}/esrun-mongodb-srv"
rm -rf "$certs"
mkdir -p "$certs"

openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=esrun test CA" \
  -keyout "$certs/ca.key" -out "$certs/ca.pem" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" \
  -keyout "$certs/server.key" -out "$certs/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:localhost,DNS:localhost.test.build.10gen.cc,DNS:localhost.sub.test.build.10gen.cc,IP:127.0.0.1\n' \
  >"$certs/san.ext"
openssl x509 -req -in "$certs/server.csr" -CA "$certs/ca.pem" -CAkey "$certs/ca.key" -CAcreateserial \
  -days 2 -extfile "$certs/san.ext" -out "$certs/server.crt" 2>/dev/null
cat "$certs/server.crt" "$certs/server.key" >"$certs/server.pem"

docker rm -f "$name" >/dev/null 2>&1 || true
docker create --name "$name" -p 27017-27019:27017-27019 "mongo:$tag" bash -c '
  for p in 27017 27018 27019; do
    mkdir -p /data/$p
    mongod --replSet repl0 --port $p --dbpath /data/$p --bind_ip_all \
      --tlsMode preferTLS --tlsCertificateKeyFile /certs/server.pem --tlsCAFile /certs/ca.pem \
      --tlsAllowConnectionsWithoutCertificates --fork --logpath /data/$p.log
  done
  tail -f /dev/null' >/dev/null
docker cp "$certs/." "$name:/certs" >/dev/null
docker start "$name" >/dev/null

shell() {
  docker exec "$name" mongosh --quiet --tls --tlsCAFile /certs/ca.pem --host localhost "$@"
}
for p in 27017 27018 27019; do
  for _ in $(seq 60); do
    shell --port $p --eval "db.runCommand({ ping: 1 }).ok" >/dev/null 2>&1 && break
    sleep 1
  done
done
shell --port 27017 --eval '
  rs.initiate({ _id: "repl0", members: [
    { _id: 0, host: "localhost:27017", priority: 2 },
    { _id: 1, host: "localhost:27018" },
    { _id: 2, host: "localhost:27019" },
  ] })' >/dev/null
for _ in $(seq 60); do
  state=$(shell --port 27017 --eval \
    'const s = rs.status().members.map(m => m.stateStr); s.filter(x => x === "PRIMARY").length + "/" + s.filter(x => x === "SECONDARY").length' 2>/dev/null || true)
  [ "$state" = "1/2" ] && break
  sleep 1
done
echo "$certs/ca.pem"
