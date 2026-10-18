#!/usr/bin/env bash
# Unit tests: the protocol, with no broker anywhere near it.
#
# These are the ones CI can always run. An integration suite needs a broker; a
# wire codec does not, and the cases worth pinning — the tag `s` that the
# specification and RabbitMQ disagree about, a frame split across one-byte
# reads, a property that is absent and shifts everything after it — are ones a
# live broker will not produce on demand anyway.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
esrun="${ESRUN:-$here/../../../../target/release/esrun}"
[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../../dist/index.js" ] || { echo "not built — tsr build" >&2; exit 1; }

status=0
for test in frame methods fields properties connection channel consumer rpc pool surface; do
  # --allow-imports: esrun grants nothing by default (DECISIONS D65) and
  # these load the built package out of dist/. Nothing else is needed —
  # a wire codec touches no network and no disk.
  "$esrun" --allow-imports "$here/$test.mjs" || status=1
done
exit $status
