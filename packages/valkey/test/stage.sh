#!/usr/bin/env bash
# Stages the built package, and the Redis package it is built on, as a consumer
# would have them installed — and the tests beside them — then prints where.
#
# Staged rather than run in place: esrun jails the module loader to the project
# root it detects from the entry file (DECISIONS D25), which here is
# packages/valkey, and pnpm links @opentf/esrun-redis in from packages/redis,
# outside it. The loader refuses that, correctly. Copying is what crosses the
# line honestly, as bench/db/redis does, and it means the tests import the
# artifact that ships, by its package name.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pkg="$(cd "$here/.." && pwd)"
redis="$(cd "$pkg/../redis" && pwd)"
stage="$here/.stage"

[ -f "$pkg/dist/index.js" ] || { echo "not built — tsr build" >&2; exit 1; }
[ -f "$redis/dist/index.js" ] || { echo "@opentf/esrun-redis is not built — tsr build" >&2; exit 1; }

rm -rf "$stage"
for dir in "$pkg:esrun-valkey" "$redis:esrun-redis"; do
  src="${dir%%:*}"
  dest="$stage/node_modules/@opentf/${dir##*:}"
  mkdir -p "$dest"
  cp -r "$src/package.json" "$src/dist" "$dest/"
done
mkdir -p "$stage/unit"
shopt -s nullglob
for test in "$here"/*.mjs; do cp "$test" "$stage/"; done
cp "$here"/unit/*.mjs "$stage/unit/"
printf '{ "private": true, "type": "module" }\n' > "$stage/package.json"
echo "$stage"
