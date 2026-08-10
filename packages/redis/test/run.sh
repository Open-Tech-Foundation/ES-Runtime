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
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
esrun="${ESRUN:-$root/target/release/esrun}"
export REDIS_URL="${REDIS_URL:-redis://127.0.0.1:6379}"
export REDIS_AUTH_URL="${REDIS_AUTH_URL:-redis://:esrun@127.0.0.1:6380}"

[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../dist/index.js" ] || { echo "not built — bun run build" >&2; exit 1; }
[ "$here/../dist/index.js" -nt "$here/../src/connection.ts" ] || echo "warning: dist is older than src — run 'bun run build'" >&2

printf "\n== unit ==\n"
"$here/unit/run.sh" || exit 1

status=0
for test in smoke db conformance errors auth pubsub blocking multi pipeline reconnect pool tls cluster; do
  printf '\n== %s ==\n' "$test"
  "$esrun" "$here/$test.mjs" || status=1
done
exit $status
