#!/usr/bin/env bash
# Runs each cargo-fuzz target for a bounded time.
#
# A script rather than a `run =` line in tasks.toml: this needs a loop and the
# ::group:: annotations, and tsr's mini-shell deliberately has no for-loops,
# pipes or redirection. Pointing `run` at a script is the documented escape
# hatch, and it keeps the command identical locally and in CI.
#
#   scripts/fuzz.sh            every target, 60s each
#   FUZZ_SECONDS=5 scripts/fuzz.sh url    one target, briefly
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

seconds="${FUZZ_SECONDS:-60}"
targets=("$@")
if [ ${#targets[@]} -eq 0 ]; then
  targets=(url encoding urlpattern compression serialization keys)
fi

for target in "${targets[@]}"; do
  echo "::group::fuzz $target"
  # New coverage-increasing inputs go to the (gitignored) corpus directory; the
  # seeds are read-only, so a run cannot bloat them.
  #
  # Created here because it is gitignored: naming a corpus directory explicitly
  # means libFuzzer expects it to already exist, and on a fresh checkout — which
  # is every CI run — none of them do.
  mkdir -p "fuzz/corpus/$target"
  cargo +nightly fuzz run "$target" \
    "fuzz/corpus/$target" "fuzz/seeds/$target" \
    -- "-max_total_time=$seconds" -rss_limit_mb=4096
  echo "::endgroup::"
done
