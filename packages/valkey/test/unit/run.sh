#!/usr/bin/env bash
# Unit tests: the Valkey family's names, with no server near them. The protocol
# under them is @opentf/esrun-redis's, and is unit-tested there.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
esrun="${ESRUN:-$here/../../../../target/release/esrun}"
[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
stage="$("$here/../stage.sh")"

status=0
for test in url; do
  # --allow-imports: esrun grants nothing by default (DECISIONS D65) and these
  # load the staged packages. A refused scheme is refused before any socket is
  # opened, so nothing else is needed.
  "$esrun" --allow-imports "$stage/unit/$test.mjs" || status=1
done
exit $status
