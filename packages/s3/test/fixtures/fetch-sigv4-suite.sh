#!/usr/bin/env bash
# Refreshes test/fixtures/sigv4 from the AWS signing test suite in
# awslabs/aws-c-auth (Apache-2.0), pinned to one commit so the fixtures are
# reproducible. Only the SigV4 (not SigV4a) cases and the files the tests read.
#
#   test/fixtures/fetch-sigv4-suite.sh [commit]
set -euo pipefail

commit="${1:-bdb7cef99d8bd1ad3c189b1400140a7787e47105}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="$here/sigv4"
base="https://raw.githubusercontent.com/awslabs/aws-c-auth/$commit/tests/aws-signing-test-suite/v4"
api="https://api.github.com/repos/awslabs/aws-c-auth/contents/tests/aws-signing-test-suite/v4?ref=$commit"
files="context.json request.txt header-canonical-request.txt header-string-to-sign.txt header-signature.txt query-canonical-request.txt query-string-to-sign.txt query-signature.txt"

cases="$(curl -fsS "$api" | grep '"name"' | cut -d'"' -f4)"
rm -rf "$out"
for name in $cases; do
  mkdir -p "$out/$name"
  for f in $files; do
    curl -fsS "$base/$name/$f" -o "$out/$name/$f" || rm -f "$out/$name/$f"
  done
done
printf '%s\n' "$commit" >"$out/COMMIT"
# Apache-2.0 §4: the licence, and the NOTICE when there is one, travel with the copy.
curl -fsS "https://raw.githubusercontent.com/awslabs/aws-c-auth/$commit/LICENSE" -o "$out/LICENSE"
curl -fsS "https://raw.githubusercontent.com/awslabs/aws-c-auth/$commit/NOTICE" -o "$out/NOTICE" || rm -f "$out/NOTICE"
cat >"$out/README.md" <<README
# AWS SigV4 test suite

The \`v4\` cases of the AWS signing test suite, from
[awslabs/aws-c-auth](https://github.com/awslabs/aws-c-auth/tree/$commit/tests/aws-signing-test-suite/v4)
at \`$commit\`, licensed Apache-2.0 (LICENSE, NOTICE). Unmodified; refreshed by
\`../fetch-sigv4-suite.sh\` and read by \`test/unit/sigv4.mjs\`.
README
echo "fetched $(printf '%s\n' $cases | wc -l) cases at $commit into $out"
