#!/usr/bin/env bash
# The protocol halves of the driver, with no server: framing, the password
# plugins against independent vectors, and the row and parameter encodings.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
esrun="${ESRUN:-$here/../../../../target/release/esrun}"
[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../../dist/index.js" ] || { echo "not built — tsr build" >&2; exit 1; }

status=0
for test in packets auth values; do
  # --allow-imports: esrun grants nothing by default (DECISIONS D65), and these
  # load the built package out of dist/. A codec touches no network and no disk.
  "$esrun" --allow-imports "$here/$test.mjs" || status=1
done
exit $status
