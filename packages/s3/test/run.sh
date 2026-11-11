#!/usr/bin/env bash
# The client's tests: the unit suites, and — when test/s3-server.sh has set the
# environment — a real S3-compatible server.
#
#   test/run.sh                                  # unit only
#   eval "$(test/s3-server.sh)" && test/run.sh    # and a live server
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
# The endpoint's host:port is the only network grant, so an e2e run against AWS
# needs S3_E2E_NET (e.g. s3.eu-west-1.amazonaws.com) as well.
net="${S3_E2E_NET:-127.0.0.1:${S3_PORT:-9000}}"
"$esrun" --allow-imports --allow-net="$net" \
  --allow-env=S3_E2E_ENDPOINT,S3_E2E_REGION,S3_E2E_ACCESS_KEY,S3_E2E_SECRET_KEY \
  "$here/e2e.mjs" || status=1
exit $status
