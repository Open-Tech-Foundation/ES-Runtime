#!/usr/bin/env bash
# Runs the package's tests against a live Valkey.
#
# There is no mock, for the Redis package's reason: the value is in speaking a
# real server's protocol. Start a server and point VALKEY_URL at it. The Redis
# package's server scripts start Valkey when given its image, and what they
# print (REDIS_* variables, redis:// URLs) is read here as the Valkey spelling.
#
#   docker run -d --name esrun-valkey -p 6379:6379 valkey/valkey:9
#   export REDIS_IMAGE=valkey/valkey:9
#   eval "$(../redis/test/tls-server.sh)"        # optional; tls is skipped without it
#   eval "$(../redis/test/cluster-server.sh)"    # optional; cluster is skipped without it
#   eval "$(../redis/test/sentinel-server.sh)"   # optional; sentinel is skipped without it
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
esrun="${ESRUN:-$root/target/release/esrun}"
[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }

# redis:// → valkey://, rediss:// → valkeys://, in a comma-separated list.
valkey() { sed -E 's#(^|,)redis(s?)://#\1valkey\2://#g' <<<"$1"; }

export VALKEY_URL="${VALKEY_URL:-valkey://127.0.0.1:6379}"
[ -n "${VALKEY_TLS_URL:-}" ] || [ -z "${REDIS_TLS_URL:-}" ] || export VALKEY_TLS_URL="$(valkey "$REDIS_TLS_URL")"
[ -n "${VALKEY_CA:-}" ] || [ -z "${REDIS_CA:-}" ] || export VALKEY_CA="$REDIS_CA"
[ -n "${VALKEY_CLUSTER_URLS:-}" ] || [ -z "${REDIS_CLUSTER_URLS:-}" ] || export VALKEY_CLUSTER_URLS="$(valkey "$REDIS_CLUSTER_URLS")"
[ -n "${VALKEY_SENTINELS:-}" ] || [ -z "${REDIS_SENTINELS:-}" ] || export VALKEY_SENTINELS="$(valkey "$REDIS_SENTINELS")"

printf "\n== unit ==\n"
"$here/unit/run.sh" || exit 1

stage="$("$here/stage.sh")"
status=0
for test in smoke tls cluster sentinel; do
  printf '\n== %s ==\n' "$test"
  # The staged packages (imports), the server (net) and the URLs (env), as in
  # the Redis package's suite. Nothing here listens or touches the disk.
  "$esrun" --allow-imports --allow-net --allow-env "$stage/$test.mjs" || status=1
done
exit $status
