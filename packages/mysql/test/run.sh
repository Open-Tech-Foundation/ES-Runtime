#!/usr/bin/env bash
# Runs the driver's tests against a live MySQL.
#
# There is no mock: the value of this package is that it speaks a real server's
# protocol, and a fake one would only ever agree with our reading of the
# documentation. Start a server, point MYSQL_URL at it.
#
#   docker run -d -p 3307:3306 -e MYSQL_ROOT_PASSWORD=esrun -e MYSQL_DATABASE=esrun_test mysql:8.4
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
esrun="${ESRUN:-$root/target/release/esrun}"
export MYSQL_URL="${MYSQL_URL:-mysql://root:esrun@127.0.0.1:3307/esrun_test?ssl-mode=DISABLED}"

[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../dist/index.js" ] || { echo "not built — tsr build" >&2; exit 1; }

printf "\n== unit ==\n"
"$here/unit/run.sh" || exit 1

status=0
# `tls` runs only when test/tls-server.sh has set MYSQL_TLS_URL, and says so.
for test in smoke conformance types statements results pool cancel auth tls; do
  printf '\n== %s ==\n' "$test"
  # esrun grants nothing by default (DECISIONS D65): these load the built
  # package (imports), reach the server (net), and read MYSQL_URL (env).
  "$esrun" --allow-imports --allow-net --allow-env "$here/$test.mjs" || status=1
done
exit $status
