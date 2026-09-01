#!/usr/bin/env bash
# Runs the driver's tests against a live Redis.
#
# There is no mock: the whole value of this package is that it speaks a real
# server's protocol, and a fake one would only ever agree with our reading of
# the specification. Start a server, point REDIS_URL at it.
#
#   docker run -d --name esrun-redis-plain -p 6379:6379 redis:latest
#   docker run -d --name esrun-redis-auth  -p 6380:6379 redis:latest \
#     redis-server --requirepass esrun
#   eval "$(test/tls-server.sh)"        # optional; tls is skipped without it
#   eval "$(test/cluster-server.sh)"    # optional; cluster is skipped without it
#   eval "$(test/sentinel-server.sh)"   # optional; sentinel is skipped without it
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
esrun="${ESRUN:-$root/target/release/esrun}"
export REDIS_URL="${REDIS_URL:-redis://127.0.0.1:6379}"
export REDIS_AUTH_URL="${REDIS_AUTH_URL:-redis://:esrun@127.0.0.1:6380}"

[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../dist/index.js" ] || { echo "not built — tsr build" >&2; exit 1; }
[ "$here/../dist/index.js" -nt "$here/../src/connection.ts" ] || echo "warning: dist is older than src — run 'tsr build'" >&2

printf "\n== unit ==\n"
"$here/unit/run.sh" || exit 1

status=0
for test in smoke db commands timeout conformance errors auth pubsub blocking multi pipeline reconnect pool tls cluster sentinel; do
  printf '\n== %s ==\n' "$test"
  # esrun grants nothing by default (DECISIONS D65). These tests load the
  # built package (imports), open a connection to the server (net) and read
  # the URL and PG*/REDIS* variables from the environment (env). No
  # subprocess, no filesystem: three, for all but one of them.
  grants=(--allow-imports --allow-net --allow-env)
  # `reconnect` also binds a socket of its own. One of its cases needs a server
  # that goes away and stays away, which is not something a container can be
  # asked to be on cue — so it stands up a socket that speaks just enough RESP
  # and then stops listening. The grant goes to the one test that listens rather
  # than to all sixteen: a capability every test holds is one no test is
  # checking the absence of.
  if [ "$test" = reconnect ]; then grants+=(--allow-listen); fi
  "$esrun" "${grants[@]}" "$here/$test.mjs" || status=1
done
exit $status
