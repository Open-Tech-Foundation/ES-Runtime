#!/usr/bin/env bash
# The client's tests: the unit tests, the protocol against a scriptable server
# (test/server.mjs), and — when test/mailpit-server.sh has set the environment —
# a real mail server with a private certificate authority.
#
#   test/run.sh                                        # unit + protocol
#   eval "$(test/mailpit-server.sh)" && test/run.sh    # and Mailpit
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
esrun="${ESRUN:-$root/target/release/esrun}"

[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../dist/index.js" ] || { echo "not built — tsr build" >&2; exit 1; }

printf "\n== unit ==\n"
"$here/unit/run.sh" || exit 1

status=0
printf "\n== protocol ==\n"
# esrun grants nothing by default (DECISIONS D65): the tests load the built
# package (imports), run a server (listen) and connect to it (net).
"$esrun" --allow-imports --allow-net --allow-listen "$here/protocol.mjs" || status=1

printf "\n== mailpit ==\n"
# Also reads the server's address and CA from the environment (env).
"$esrun" --allow-imports --allow-net --allow-env "$here/mailpit.mjs" || status=1
exit $status
