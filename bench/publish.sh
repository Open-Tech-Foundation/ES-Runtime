#!/usr/bin/env bash
#
# Publishes benchmark data from a checkout on a fast disk.
#
# Several workloads write files into the benchmark's own directory, and on a
# spinning disk those rows measure the disk: they come out slower than on the
# SSD the published numbers were taken on, and noisy enough that the validator
# refuses them run after run. So the suite runs from a detached worktree on a
# non-rotational disk — built binaries and drivers copied in, dependencies
# installed — and the data module it writes is copied back here.
#
#   bench/publish.sh                          every section
#   SECTIONS=pg_qps bench/publish.sh          one section (or several, quoted)
#   bench/publish.sh fsappend_large           re-measure rows over a kept full run
#
# PG_URL, MYSQL_URL, SECTIONS and every other gen-bench-data.sh knob pass
# through. BENCH_WORKTREE picks the checkout (default ~/es-bench), and must be
# on a solid-state disk; BENCH_ALLOW_ROTATIONAL=1 overrides that check.
#
# It measures the *committed* tree: the worktree is moved to this checkout's
# HEAD, so commit what should be measured first. Afterwards, review and commit
# website/src/benchmarks.js and bench/README.md here.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tree="${BENCH_WORKTREE:-$HOME/es-bench}"

# Whether `dir` lives on a spinning disk. Linux only; elsewhere it says no.
rotational() {
  command -v findmnt >/dev/null 2>&1 || return 1
  local source device
  source="$(findmnt -n -o SOURCE --target "$1" 2>/dev/null)" || return 1
  device="$(lsblk -n -o PKNAME "$source" 2>/dev/null | head -1)"
  [ -n "$device" ] || device="$(basename "$source")"
  [ "$(cat "/sys/block/$device/queue/rotational" 2>/dev/null)" = 1 ]
}

problems=()
for bin in esrun esdev; do
  [ -x "$root/target/release/$bin" ] || problems+=("no $bin at target/release — cargo build --release")
done
for pkg in postgres mysql; do
  [ -f "$root/packages/$pkg/dist/index.js" ] || problems+=("packages/$pkg is not built — tsr build")
done
probe="$tree"
while [ ! -e "$probe" ]; do probe="$(dirname "$probe")"; done
if rotational "$probe" && [ "${BENCH_ALLOW_ROTATIONAL:-0}" != 1 ]; then
  problems+=("$tree is on a spinning disk — set BENCH_WORKTREE to a path on an SSD")
fi
if [ ${#problems[@]} -gt 0 ]; then
  echo "not starting — fix these first:" >&2
  printf '  - %s\n' "${problems[@]}" >&2
  exit 1
fi
if [ -n "$(git -C "$root" status --porcelain -- bench website/src crates packages)" ]; then
  echo "note: uncommitted changes here are not measured — the worktree runs HEAD" >&2
fi

# The worktree, at this checkout's HEAD. Detached, so no branch is created, and
# forced, because the data module a previous publish left in it has already
# been copied back. Its bench/.cache is untracked and survives, which is what
# lets a failed run resume — gen-bench-data.sh keys it on the binaries.
head="$(git -C "$root" rev-parse HEAD)"
if [ -e "$tree/.git" ]; then
  git -C "$tree" checkout --quiet --detach --force "$head"
else
  git -C "$root" worktree add --quiet --detach "$tree" "$head"
fi

mkdir -p "$tree/target/release"
cp -p "$root/target/release/esrun" "$root/target/release/esdev" "$tree/target/release/"
for pkg in postgres mysql; do
  rm -rf "$tree/packages/$pkg/dist"
  cp -r "$root/packages/$pkg/dist" "$tree/packages/$pkg/dist"
done
(cd "$tree" && pnpm install --frozen-lockfile --silent)
if [ ! -d "$tree/bench/dev-server/apps/app-10000/node_modules" ]; then
  (cd "$tree/bench/dev-server" && node gen.mjs 10000 >/dev/null &&
    cd apps/app-10000 && npm install --no-audit --no-fund --silent)
fi

# Start from this checkout's data module, not the committed one, so phases
# published but not yet committed here accumulate instead of being dropped.
cp "$root/website/src/benchmarks.js" "$tree/website/src/benchmarks.js"

echo "measuring in $tree at ${head:0:8}" >&2
(cd "$tree/bench" && bash gen-bench-data.sh "$@")

# Only the data module comes back; the README's table is regenerated from it
# here, so an uncommitted edit elsewhere in this README is left alone.
cp "$tree/website/src/benchmarks.js" "$root/website/src/benchmarks.js"
(cd "$root/bench" && node sync-readme-table.mjs)
echo "published — review and commit website/src/benchmarks.js and bench/README.md" >&2
