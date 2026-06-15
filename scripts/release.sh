#!/usr/bin/env bash
#
# Controlled release for ES-Runtime.
#
#   scripts/release.sh
#
# Asks for a major / minor / patch bump, then:
#   1. bumps the version (workspace Cargo.toml — package version + the internal
#      es-runtime* dep pins — and Cargo.lock). The site
#      (site/package.json) is versioned independently — docs change far more
#      often than the runtime — so it is intentionally left alone.
#   2. promotes CHANGELOG.md's [Unreleased] section to the new version + date,
#   3. shows the diff and asks you to confirm,
#   4. commits + creates an annotated git tag,
#   5. asks whether to push the branch and tag.
#
# Pushing the tag triggers .github/workflows/release.yml, which runs the tests
# and only then builds and publishes the artifacts. Nothing here is automatic —
# every step waits for you.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

CARGO_TOML="Cargo.toml"
CHANGELOG="CHANGELOG.md"

red() { printf '\033[31m%s\033[0m\n' "$*" >&2; }
bold() { printf '\033[1m%s\033[0m\n' "$*"; }
err() { red "error: $*"; exit 1; }

# --- preconditions ----------------------------------------------------------
[ -f "$CARGO_TOML" ] || err "must run inside the ES-Runtime repo"
git diff --quiet && git diff --cached --quiet || err "working tree is dirty — commit or stash first"

# --- local quality gate (before touching anything) -------------------------
# Verify locally first so a release is never started on code that doesn't
# format, test, or lint clean. `cargo test --workspace` includes the CLI
# end-to-end test (crates/runtime-cli/tests) that spawns the real `esrun`.
# Reuses the existing target/ cache (no clean) — incremental on a warm tree;
# a cold tree is slow once (downloads/links the prebuilt V8 static lib).
#
# Order matters: tests compile + link (which fetches the v8 prebuilt lib), so
# they run before clippy — clippy runs in check mode and won't fetch it on a
# cold tree.
bold "Running fmt, the full test suite, and clippy…"
cargo fmt --all --check || err "formatting check failed — run 'cargo fmt' and retry"
cargo test --workspace --locked || err "tests failed — fix them and retry"
cargo clippy --workspace --all-targets --locked -- -D warnings ||
  err "clippy failed — fix the warnings and retry"

branch="$(git rev-parse --abbrev-ref HEAD)"

# Current version = the workspace.package.version (first `version = "x"` line).
current="$(grep -m1 -E '^version = "' "$CARGO_TOML" | sed -E 's/^version = "([^"]+)".*/\1/')"
[ -n "$current" ] || err "could not read the workspace version from $CARGO_TOML"
IFS=. read -r MA MI PA <<<"${current%%-*}"

bold "ES-Runtime release"
echo "  current version: $current"
echo "  branch:          $branch"
echo

# --- choose the bump --------------------------------------------------------
patch="$MA.$MI.$((PA + 1))"
minor="$MA.$((MI + 1)).0"
major="$((MA + 1)).0.0"

echo "Release type:"
echo "  1) patch  -> $patch"
echo "  2) minor  -> $minor"
echo "  3) major  -> $major"
asis=""
if ! git rev-parse -q --verify "refs/tags/v$current" >/dev/null; then
  asis="$current"
  echo "  0) current -> $current   (release the current version as-is; not yet tagged)"
fi
printf "Choice: "
read -r choice
case "$choice" in
  1) new="$patch" ;;
  2) new="$minor" ;;
  3) new="$major" ;;
  0) [ -n "$asis" ] && new="$asis" || err "invalid choice" ;;
  *) err "invalid choice" ;;
esac

tag="v$new"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && err "tag $tag already exists"
date="$(date -u +%Y-%m-%d)"
echo
bold "Preparing $tag ($date)"

# --- 1. bump versions -------------------------------------------------------
# workspace.package.version — the first `version = "..."` line in the root manifest.
NEW="$new" perl -0777 -i -pe 'BEGIN{$v=$ENV{NEW}} s/^(version = ")[^"]+(")/${1}$v${2}/m' "$CARGO_TOML"
# Internal-crate dep pins in [workspace.dependencies]: each `es-runtime*` path
# dep also carries an explicit `version = "..."` that must track the workspace
# version (a path dep still has to satisfy its own version requirement, or
# `cargo update` can't resolve). Bump every internal pin in lockstep — these are
# single-line entries (`es-runtime-x = { path = "crates/x", version = "..." }`).
NEW="$new" perl -i -pe \
  'BEGIN{$v=$ENV{NEW}} s/(version = ")[^"]+(")/${1}$v${2}/ if /^es-runtime[\w-]* = .*\bpath = "crates\//;' \
  "$CARGO_TOML"
# Cargo.lock — refresh only the workspace members' recorded versions (no build).
# On failure, restore the manifest so a half-done bump never strands a dirty tree.
cargo update --workspace --offline >/dev/null 2>&1 ||
  cargo update --workspace >/dev/null 2>&1 || {
    git checkout -- "$CARGO_TOML" 2>/dev/null || true
    err "could not update Cargo.lock (run 'cargo update --workspace' and retry)"
  }

# --- 2. changelog: promote [Unreleased] -> [new] - date, keep a fresh one ---
grep -q '^## \[Unreleased\]' "$CHANGELOG" || err "no '## [Unreleased]' section in $CHANGELOG"
NEW="$new" DATE="$date" perl -0777 -i -pe \
  'BEGIN{$v=$ENV{NEW};$d=$ENV{DATE}} s/^## \[Unreleased\]/## [Unreleased]\n\n## [$v] - $d/m' \
  "$CHANGELOG"

# --- 3. review + confirm ----------------------------------------------------
echo
bold "Changes for $tag:"
git --no-pager diff -- "$CARGO_TOML" "$CHANGELOG"
echo
printf "Commit these and tag %s? [y/N] " "$tag"
read -r confirm
case "$confirm" in
  y | Y) ;;
  *)
    bold "Aborted — reverting changes."
    git checkout -- "$CARGO_TOML" "$CHANGELOG" Cargo.lock 2>/dev/null || true
    exit 1
    ;;
esac

# --- 4. commit + tag --------------------------------------------------------
git add "$CARGO_TOML" "$CHANGELOG" Cargo.lock
git commit -m "release: $tag"
git tag -a "$tag" -m "$tag"
bold "Committed and tagged $tag."

# --- 5. push ----------------------------------------------------------------
echo
printf "Push '%s' and tag '%s' to origin now? [y/N] " "$branch" "$tag"
read -r dopush
case "$dopush" in
  y | Y)
    git push origin "$branch" "$tag"
    bold "Pushed. The Release workflow will run tests, build, and publish $tag."
    ;;
  *)
    bold "Not pushed. When ready:"
    echo "  git push origin $branch $tag"
    ;;
esac
