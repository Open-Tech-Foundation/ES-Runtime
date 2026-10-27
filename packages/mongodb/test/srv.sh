#!/usr/bin/env bash
# Runs MongoDB's seedlist discovery suite (srv.mjs) against the replica set
# test/srv-replica-set.sh starts, whose CA it prints:
#
#   MONGODB_SRV_CA=$(test/srv-replica-set.sh) test/srv.sh
#
# Needs the specification checkout (mongodb-specs/fetch.sh) and public DNS.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
esrun="${ESRUN:-$root/target/release/esrun}"

[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../dist/index.js" ] || { echo "not built — tsr build" >&2; exit 1; }
"$root/mongodb-specs/fetch.sh"

# Reads the CA and the spec's test files; reaches DNS and the servers.
read="$root/mongodb-specs/upstream"
[ -n "${MONGODB_SRV_CA:-}" ] && read="$read,$(dirname "$MONGODB_SRV_CA")"
"$esrun" --allow-imports --allow-net --allow-env --allow-read="$read" "$here/srv.mjs"
