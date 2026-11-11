#!/usr/bin/env bash
# Fetches the whole hpack-test-case corpus, at the revision vendored in
# crates/http/tests/corpus/hpack-test-case/REVISION, into the ignored
# crates/http/tests/corpus/hpack-test-case-full/. `cargo test -p es-runtime-http`
# then decodes every story of every encoder, not only the vendored subset.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
corpus=crates/http/tests/corpus
rev="$(cat "$corpus/hpack-test-case/REVISION")"
out="$corpus/hpack-test-case-full"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -sfL "https://github.com/http2jp/hpack-test-case/archive/$rev.tar.gz" -o "$tmp/corpus.tar.gz"
rm -rf "$out"
mkdir -p "$out"
tar -xzf "$tmp/corpus.tar.gz" -C "$tmp"
for dir in "$tmp"/hpack-test-case-*/*/; do
  name="$(basename "$dir")"
  ls "$dir"/story_*.json >/dev/null 2>&1 && cp -r "$dir" "$out/$name"
done
echo "fetched $(ls "$out" | wc -l) encoders into $out"
