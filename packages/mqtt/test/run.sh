#!/usr/bin/env bash
# The client's tests: the unit suites against a scripted broker, and — when
# test/mosquitto-server.sh has set the environment — a real Mosquitto.
#
#   test/run.sh                                        # unit only
#   eval "$(test/mosquitto-server.sh)" && test/run.sh    # and Mosquitto
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
# package (imports), dial the broker (net) and read its address (env). The
# addresses are scoped to the plain and optional TLS listeners used by the tests.
"$esrun" --allow-imports --allow-net=127.0.0.1:1883,127.0.0.1:8883,localhost:8883 --allow-env "$here/e2e.mjs" || status=1
exit $status
