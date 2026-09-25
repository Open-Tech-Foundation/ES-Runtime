#!/usr/bin/env bash
# The halves of the client that need no server: reply parsing, the framing of
# DATA, the MIME encoders and message builder, and the login encodings.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
esrun="${ESRUN:-$here/../../../../target/release/esrun}"
[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../../dist/index.js" ] || { echo "not built — tsr build" >&2; exit 1; }

status=0
for test in reply data mime auth; do
  # --allow-imports: esrun grants nothing by default (DECISIONS D65), and these
  # load the built package out of dist/. None of them touches the network.
  "$esrun" --allow-imports "$here/$test.mjs" || status=1
done
exit $status
