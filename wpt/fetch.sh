#!/usr/bin/env bash
# Fetches the pinned Web Platform Tests subset into wpt/upstream (gitignored).
#
# A blobless, sparse, shallow checkout: the four directories the worker subset
# needs, not the 5 GB tree. Re-running is idempotent and moves an existing
# checkout to REV.
set -euo pipefail

# Pinned so a run is reproducible. Bump deliberately, and re-record
# expectations.json in the same commit.
REV=ce9441ee673c68c5d175d2363c9ff2e4893b827c

DIRS=(resources common workers webmessaging html/webappapis/structured-clone)

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest="$here/upstream"

if [ ! -d "$dest/.git" ]; then
  git clone --filter=blob:none --sparse --no-checkout \
    https://github.com/web-platform-tests/wpt.git "$dest"
fi

cd "$dest"
git sparse-checkout set "${DIRS[@]}"
git fetch --filter=blob:none --depth=1 origin "$REV"
git checkout --detach "$REV"

echo "wpt/upstream at $REV"
