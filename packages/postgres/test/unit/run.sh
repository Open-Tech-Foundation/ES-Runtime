#!/usr/bin/env bash
# Unit tests: the protocol, with no database anywhere near it.
#
# These are the ones CI can always run. The integration suite needs a server;
# a wire codec does not, and the cases worth pinning — a message split across
# three chunks, a quoted NULL, RFC 7677's vectors — are ones a live server will
# not produce on demand anyway.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
esrun="${ESRUN:-$here/../../../../target/release/esrun}"
[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../../dist/index.js" ] || { echo "not built — bun run build" >&2; exit 1; }

status=0
for test in scram frame values; do
  "$esrun" "$here/$test.mjs" || status=1
done
exit $status
