#!/usr/bin/env bash
# Runs the driver's tests against a live MongoDB.
#
# There is no mock: the value of this driver is that it speaks a real server's
# protocol, and a fake one would only ever agree with our reading of the
# specifications. Start a server with test commands enabled (the cancellation
# tests use `sleep`), and point MONGODB_URL at it:
#
#   docker run -d -p 27017:27017 mongo:8 --setParameter enableTestCommands=1
#
# `auth` runs only when MONGODB_AUTH_URL names a server with access control on,
# as a user who may create users, and says so otherwise:
#
#   docker run -d -p 27018:27017 -e MONGO_INITDB_ROOT_USERNAME=root \
#     -e MONGO_INITDB_ROOT_PASSWORD=esrun mongo:8
#   MONGODB_AUTH_URL=mongodb://root:esrun@127.0.0.1:27018/
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
esrun="${ESRUN:-$root/target/release/esrun}"
export MONGODB_URL="${MONGODB_URL:-mongodb://127.0.0.1:27017/esrun_test}"

[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../dist/index.js" ] || { echo "not built — tsr build" >&2; exit 1; }

status=0
for test in smoke crud cursor batch errors cancel sessions auth replica retry; do
  printf '\n== %s ==\n' "$test"
  # esrun grants nothing by default (DECISIONS D65): these load the built
  # package (imports), reach the server (net), and read MONGODB_*_URL (env).
  "$esrun" --allow-imports --allow-net --allow-env "$here/$test.mjs" || status=1
done

printf '\n== exit ==\n'
# A client left open must not keep the program running.
rc=0
timeout 10 "$esrun" --allow-imports --allow-net --allow-env "$here/exit.mjs" || rc=$?
if [ "$rc" -eq 124 ]; then
  echo "FAILED exit: a client that was never closed kept the program running"
  status=1
elif [ "$rc" -ne 0 ]; then
  status=1
fi
exit $status
