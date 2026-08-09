#!/usr/bin/env bash
# Runs the driver's tests against a live PostgreSQL.
#
# There is no mock: the whole value of this package is that it speaks a real
# server's protocol, and a fake one would only ever agree with our reading of
# the specification. Start a server, point PG_URL at it.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
esrun="${ESRUN:-$root/target/release/esrun}"
export PG_URL="${PG_URL:-postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable}"

[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../dist/index.js" ] || { echo "not built — bun run build" >&2; exit 1; }

status=0
for test in smoke conformance tls concurrency timeouts; do
  printf '\n== %s ==\n' "$test"
  "$esrun" "$here/$test.mjs" || status=1
done
exit $status
