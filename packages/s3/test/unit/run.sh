#!/usr/bin/env bash
# Unit tests: the signer against the AWS test suite, and the client against a
# fake S3 that checks every signature it receives.
#
# These are the ones CI can always run. The fake is what makes faults
# scriptable — a part that fails twice, a 200 that carries an <Error>, a server
# clock an hour ahead — which a real server will not produce on demand.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
esrun="${ESRUN:-$here/../../../../target/release/esrun}"
[ -x "$esrun" ] || { echo "no esrun at $esrun — cargo build --release -p es-runtime-cli" >&2; exit 1; }
[ -f "$here/../../dist/index.js" ] || { echo "not built — esdev build --lib src" >&2; exit 1; }

status=0
# esrun grants nothing by default (DECISIONS D65): the suites load the built
# package (imports), read the committed AWS vectors (read), and the client suite
# serves the fake on loopback and dials it (listen, net).
"$esrun" --allow-imports --allow-read="$here/../fixtures" "$here/sigv4.mjs" || status=1
"$esrun" --allow-imports --allow-net=127.0.0.1 --allow-listen=127.0.0.1 "$here/client.mjs" || status=1
exit $status
