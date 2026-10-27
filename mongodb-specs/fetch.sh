#!/usr/bin/env bash
# Fetches the pinned MongoDB driver specification tests into
# mongodb-specs/upstream (gitignored).
#
# They are fetched rather than committed: the specifications repository is
# licensed CC BY-NC-SA 3.0, which this Apache-2.0 tree does not redistribute.
# A blobless, sparse, shallow checkout of the test directories the runners use.
# Re-running is idempotent and moves an existing checkout to REV.
set -euo pipefail

# Pinned so a run is reproducible. Bump deliberately.
REV=9ea18a92308ed437e16dd95e6ff9c496daa61ecd

DIRS=(source/bson-corpus/tests source/bson-binary-vector/tests)

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest="$here/upstream"

if [ ! -d "$dest/.git" ]; then
  git clone --filter=blob:none --sparse --no-checkout \
    https://github.com/mongodb/specifications.git "$dest"
fi

cd "$dest"
git sparse-checkout set "${DIRS[@]}"
git fetch --filter=blob:none --depth=1 origin "$REV"
git checkout --detach "$REV"

echo "mongodb-specs/upstream at $REV"
