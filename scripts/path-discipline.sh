#!/usr/bin/env bash
# Refuses hand-made path spellings outside the one path module (D165).
#
# `clippy.toml` bans the *functions* that spell a path two ways —
# `canonicalize`, `Url::from_file_path` and the rest. What it cannot ban is the
# string arithmetic that does the same job by hand, because the methods
# involved (`replace`, `strip_prefix`, `format!`) are everywhere for good
# reasons. These are the shapes that produced the Windows bugs D165 lists:
#
#   strip_prefix("file://")   `/D:/…` on Windows, and `%20` left encoded
#   format!("file://…")       `file://C:\…`, which is not a file URL
#   replace('\\', "/")        corrupts a POSIX name that contains a backslash
#
# Each has a replacement in `es_runtime_default_providers::path`:
# `specifier_path`, `file_url` and `to_slash`. A line that means it — a test
# asserting a literal URL — says so with a `path-discipline:` comment.
#
# A script rather than a `run =` line in tasks.toml: the check is a pipeline and
# tsr's mini-shell has no pipes by design.
set -euo pipefail

cd "$(dirname "$0")/.."

pattern='strip_prefix\("file://"\)|trim_start_matches\("file://"\)|format!\("file://|replace\(('"'"'\\\\'"'"'|"\\\\"), "/"\)'

hits=$(grep -rnE --include='*.rs' "$pattern" crates \
    | grep -v '^crates/default-providers/src/path\.rs:' \
    | grep -v 'path-discipline:' || true)

if [[ -n "$hits" ]]; then
    echo "Hand-made path spellings (D165). Use es_runtime_default_providers::path:"
    echo "  specifier_path (for a file: URL), file_url (for a path), to_slash (for /)."
    echo
    echo "$hits"
    exit 1
fi
echo "path-discipline: no hand-made path spellings"
