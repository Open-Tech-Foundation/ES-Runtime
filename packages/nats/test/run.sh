#!/usr/bin/env bash
# The client's tests: the unit suites against a scripted server, and — when
# test/nats-server.sh has set the environment — a real NATS.
#
#   test/run.sh                                    # unit only
#   eval "$(test/nats-server.sh)" && test/run.sh    # and a live server
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
esrun="${ESRUN:-$root/target/release/esrun}"

[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../dist/index.js" ] || { echo "not built — esdev build --lib src" >&2; exit 1; }

printf "\n== unit ==\n"
"$here/unit/run.sh" || exit 1

status=0
printf "\n== e2e ==\n"
# esrun grants nothing by default (DECISIONS D65): the tests load the built
# package (imports), dial the server (net) and read its address (env). The
# address is scoped to the plain and optional TLS listeners used by the tests.
"$esrun" --allow-imports --allow-net=127.0.0.1:4222,127.0.0.1:4223,localhost:4223 --allow-env "$here/e2e.mjs" || status=1
exit $status
