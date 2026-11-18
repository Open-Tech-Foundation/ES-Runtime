#!/usr/bin/env bash
# Creates every template in packages/esdev-templates, in every combination of
# its options, and checks that what was created works: it installs, typechecks,
# passes its own tests, builds, and the build serves its first page.
#
# A template is what a new user runs first, and nothing else exercises one end
# to end with its real dependencies: the Rust suite cannot reach npm, so it
# scaffolds against a stand-in registry and stubs what a package provides. A
# template that fails its own `typecheck` shipped that way (FsFile.type).
#
# The combinations come from each template.json, so a template or a choice
# added there is covered without editing this file.
#
# Usage:  scripts/template-matrix.sh [template...]
#   ESDEV_BIN=<dir>   test the esdev and esrun in <dir> (default: the ones on PATH)
#   KEEP=1            leave the created projects behind, and say where
#
# Needs network access (npm), pnpm and node. Run from anywhere.
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
templates="$repo/packages/esdev-templates"
if [ -n "${ESDEV_BIN:-}" ]; then
  export PATH="$ESDEV_BIN:$PATH"
fi
command -v esdev >/dev/null || { echo "esdev is not on PATH" >&2; exit 2; }
command -v esrun >/dev/null || { echo "esrun is not on PATH" >&2; exit 2; }

work="$(mktemp -d "${TMPDIR:-/tmp}/template-matrix.XXXXXX")"
if [ -z "${KEEP:-}" ]; then
  trap 'rm -rf "$work"' EXIT
fi

# One line per combination: "<template> [--option=value ...]".
combinations() {
  node - "$templates/templates" "$@" <<'EOF'
const fs = require("node:fs");
const path = require("node:path");
const [dir, ...only] = process.argv.slice(2);
for (const name of fs.readdirSync(dir).sort()) {
  const manifest = path.join(dir, name, "template.json");
  if (!fs.existsSync(manifest)) continue;
  if (only.length && !only.includes(name)) continue;
  const { options = [] } = JSON.parse(fs.readFileSync(manifest, "utf8"));
  let rows = [[]];
  for (const option of options) {
    const values = option.choices
      ? option.choices.map((c) => `--${option.name}=${c.value}`)
      : [`--${option.name}`, `--no-${option.name}`];
    rows = rows.flatMap((row) => values.map((value) => [...row, value]));
  }
  for (const row of rows) console.log([name, ...row].join(" "));
}
EOF
}

# Whether package.json in "$1" has the script "$2".
has_script() {
  node -e 'process.exit(require(process.argv[1]).scripts?.[process.argv[2]] ? 0 : 1)' \
    "$1/package.json" "$2"
}

# Waits for "$1" to answer 200, for up to 15 seconds.
answers() {
  for _ in $(seq 1 60); do
    if [ "$(curl -s -o /dev/null -w '%{http_code}' "$1" 2>/dev/null)" = "200" ]; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

free_port() {
  node -e 'const s=require("node:net").createServer().listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})'
}

# Runs "$@" in the background in its own process group, so stopping it stops
# what it started (pnpm → esrun).
serve() {
  setsid "$@" >"$log" 2>&1 &
  server=$!
}
stop() {
  kill -TERM -- "-$server" 2>/dev/null
  wait "$server" 2>/dev/null
}

failed=()
passed=0
while read -r template options; do
  slug="$(echo "$template $options" | tr -c 'a-z0-9\n' '-' | sed 's/-\+/-/g; s/-$//')"
  project="$work/$slug"
  log="$work/$slug.log"
  step() {
    echo "  $1" >>"$log"
    shift
    (cd "$project" && "$@") >>"$log" 2>&1
  }
  fail() {
    failed+=("$template $options — $1")
    echo "✖ $template $options — $1 (log: $log)"
    tail -20 "$log" | sed 's/^/    /'
  }

  # shellcheck disable=SC2086 # the options are separate flags
  if ! (cd "$work" && esdev create "$slug" --template="$template" $options \
    --from="$templates" --install=pnpm --yes) >"$log" 2>&1; then
    fail "create"
    continue
  fi
  # Checked against the runtime's declarations as this commit has them, which
  # is what the next release ships with these templates — not the ones the
  # registry has, which a fix here cannot reach until it is published.
  if ! step "types from this commit" pnpm add --save-dev "@opentf/esrun-types@file:$repo/packages/types"; then
    fail "installing this commit's @opentf/esrun-types"
    continue
  fi
  ok=1
  for script in typecheck test test:dom build; do
    if has_script "$project" "$script" && ! step "$script" pnpm run "$script"; then
      fail "$script"
      ok=0
      break
    fi
  done
  [ "$ok" = 1 ] || continue

  # The build serves its first page, the way the template says to run it.
  if has_script "$project" start; then
    # The start script carries the production grant, port included.
    port="$(node -e 'const s=require(process.argv[1]).scripts.start;const m=s.match(/--allow-listen=[^ ]*:(\d+)/);console.log(m?m[1]:3000)' "$project/package.json")"
    serve bash -c "cd '$project' && exec pnpm run start"
    url="http://127.0.0.1:$port/"
  elif has_script "$project" preview; then
    port="$(free_port)"
    serve bash -c "cd '$project' && exec esdev preview --port=$port"
    url="http://127.0.0.1:$port/"
  else
    url=""
  fi
  if [ -n "$url" ]; then
    if ! answers "$url"; then
      stop
      fail "serving $url"
      continue
    fi
    stop
  fi
  passed=$((passed + 1))
  echo "✔ $template $options"
done < <(combinations "$@")

echo
echo "$passed passed, ${#failed[@]} failed"
if [ -n "${KEEP:-}" ]; then
  echo "projects kept in $work"
fi
for failure in "${failed[@]}"; do
  echo "  ✖ $failure"
done
[ "${#failed[@]}" -eq 0 ]
