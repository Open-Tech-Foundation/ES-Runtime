#!/usr/bin/env bash
# Unit tests: the protocol, with no server anywhere near it.
#
# These are the ones CI can always run. The integration suite needs a Redis; a
# wire codec does not, and the cases worth pinning — a reply split across five
# chunks, an attribute nobody asked for, a CRLF inside a bulk string — are ones
# a live server will not produce on demand anyway.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
esrun="${ESRUN:-$here/../../../../target/release/esrun}"
[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../../dist/index.js" ] || { echo "not built — bun run build" >&2; exit 1; }

status=0
for test in resp values url blocking slots; do
  "$esrun" "$here/$test.mjs" || status=1
done
exit $status
