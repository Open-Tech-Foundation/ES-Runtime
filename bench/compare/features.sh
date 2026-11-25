#!/usr/bin/env bash
# The capability tables on the comparison page, by running each runtime.
#
#   bash bench/compare/features.sh          # print the rows, with versions
#
# Runs probe.mjs on esrun (a release build), Node.js, Bun and Deno from a
# scratch directory holding the modules it imports. Node runs twice: plainly,
# and under --permission, since process.permission exists only there. A
# runtime that is missing is left out.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ESRUN="${ESRUN:-$ROOT/target/release/esrun}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cp "$ROOT/bench/compare/probe.mjs" "$WORK/"
echo '{"ok":true}' > "$WORK/data.json"
echo 'globalThis.onmessage = () => {};' > "$WORK/worker.js"
# (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add)
printf '\x00asm\x01\x00\x00\x00\x01\x07\x01\x60\x02\x7f\x7f\x01\x7f\x03\x02\x01\x00\x07\x07\x01\x03add\x00\x00\x0a\x09\x01\x07\x00\x20\x00\x20\x01\x6a\x0b' > "$WORK/add.wasm"
# _start calls fd_write(1, iovs=0, 1, nwritten=8) on "ok\n" at 16; memory exported.
printf '\x00asm\x01\x00\x00\x00\x01\x0c\x02\x60\x04\x7f\x7f\x7f\x7f\x01\x7f\x60\x00\x00\x02\x23\x01\x16wasi_snapshot_preview1\x08fd_write\x00\x00\x03\x02\x01\x01\x05\x03\x01\x00\x01\x07\x13\x02\x06_start\x00\x01\x06memory\x02\x00\x0a\x0f\x01\x0d\x00\x41\x01\x41\x00\x41\x01\x41\x08\x10\x00\x1a\x0b\x0b\x19\x01\x00\x41\x00\x0b\x13\x10\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00ok\n' > "$WORK/start.wasm"

run() { (cd "$WORK" && timeout 30 "$@" probe.mjs 2>/dev/null | tail -1) || true; }

versions=()
results=()
if [ -x "$ESRUN" ]; then versions+=("$("$ESRUN" --version)"); results+=("$(run "$ESRUN" --allow-all)"); fi
if command -v node >/dev/null; then
  versions+=("node $(node --version)")
  plain="$(run node)"
  perm="$(run node --permission --allow-fs-read=* --allow-worker --allow-net)"
  results+=("$(node -e 'const [a,b]=process.argv.slice(1).map(JSON.parse);a.permissionQuery=b.permissionQuery;a.permissionDrop=b.permissionDrop;console.log(JSON.stringify(a))' "$plain" "$perm")")
fi
if command -v bun >/dev/null; then versions+=("bun $(bun --version)"); results+=("$(run bun)"); fi
if command -v deno >/dev/null; then versions+=("$(deno --version | head -1 | cut -d' ' -f1-2)"); results+=("$(run deno run -A --unstable-net --quiet)"); fi

printf '%s\n' "${versions[@]}"
echo
node -e '
const rows = process.argv.slice(1).map((r) => { try { return JSON.parse(r); } catch { return null; } }).filter(Boolean);
const names = Object.keys(rows[0]).filter((k) => k !== "runtime");
console.log(["row", ...rows.map((r) => r.runtime)].join("\t"));
for (const n of names) console.log([n, ...rows.map((r) => r[n])].join("\t"));
' "${results[@]}"
