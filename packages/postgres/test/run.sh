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
[ -f "$here/../dist/index.js" ] || { echo "not built — tsr build" >&2; exit 1; }
[ "$here/../dist/index.js" -nt "$here/../src/connection.ts" ] || echo "warning: dist is older than src — run 'tsr build'" >&2

printf "\n== unit ==\n"
"$here/unit/run.sh" || exit 1

# The PG* variables, derived from PG_URL, so the environment test exercises the
# path libpq tools take rather than only the parsing around it.
pg_no_scheme="${PG_URL#*://}"
pg_creds="${pg_no_scheme%%@*}"
pg_hostpart="${pg_no_scheme#*@}"
export PGUSER="${pg_creds%%:*}"
export PGPASSWORD="${pg_creds#*:}"
export PGHOST="$(printf '%s' "${pg_hostpart%%/*}" | cut -d: -f1)"
export PGPORT="$(printf '%s' "${pg_hostpart%%/*}" | cut -d: -f2)"
pg_dbpart="${pg_hostpart#*/}"
export PGDATABASE="${pg_dbpart%%\?*}"
export PGSSLMODE=disable

status=0
for test in smoke conformance tls concurrency timeouts lost tls-ca script arrays async-messages statements pool environment cancel listen binary; do
  printf '\n== %s ==\n' "$test"
  # esrun grants nothing by default (DECISIONS D65). These tests load the
  # built package (imports), open a connection to the server (net) and read
  # the URL and PG*/REDIS* variables from the environment (env). No
  # subprocess, no filesystem: the grant stays at three.
  "$esrun" --allow-imports --allow-net --allow-env "$here/$test.mjs" || status=1
done
exit $status
