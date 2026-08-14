#!/usr/bin/env bash
# Asserts that the tree's one copyleft dependency cannot reach a deployed service.
#
# `deny.toml` allows lightningcss (MPL-2.0) as a deliberate exception, and the
# case for it rests on where the crate ends up: linked into `esdev`, the binary a
# developer runs to produce an artifact, and never into `esrun`, the binary that
# runs one in production. MPL's copyleft is per-file, so that placement is what
# keeps the obligation confined to lightningcss's own sources.
#
# `cargo deny` checks licenses against a policy; it has no way to say "this
# license, only in that part of the graph". So the graph is checked here. Without
# it, moving a workspace dependency one line would quietly invalidate the
# argument the exception is granted on, and nothing would fail.
#
# A script rather than a `run =` line in tasks.toml: the check is a pipeline and
# tsr's mini-shell has no pipes by design.
set -euo pipefail

# The binaries a service deploys. `esdev` is deliberately absent — it is the
# crate the exception exists for.
DEPLOYED=(es-runtime-cli es-runtime es-runtime-engine es-runtime-providers
    es-runtime-default-providers es-runtime-common es-runtime-cli-common)

# Every license family `deny.toml` refuses. Matched against the licenses actually
# in the resolved graph rather than against a list of crate names, so a
# *different* copyleft crate arriving is caught by the same check.
#
# Matched on whole words, against the license field alone. Both halves matter: a
# substring search over the whole line finds "MPL" in `thiserror-impl` and "EPL"
# in `self-replace`, which is how this check would come to be ignored.
COPYLEFT='^(MPL|GPL|LGPL|AGPL|CDDL|EPL|CPL|OSL|EUPL|SSPL)(-|$)'

status=0
for crate in "${DEPLOYED[@]}"; do
    # `--all-features` because a feature is the easy way to reintroduce a
    # dependency that the default build does not have.
    #
    # A tab separates the two fields, and awk splits the license expression on
    # the operators an SPDX expression is built from, so `MIT OR Apache-2.0`
    # is tested as two licenses and not as one string that starts with neither.
    found=$(cargo tree --package "$crate" --all-features --edges normal,build \
        --prefix none --format '{p}%{l}' 2>/dev/null |
        awk -F'%' -v pattern="$COPYLEFT" '
            {
                n = split($2, licenses, /[ ]*(OR|AND|WITH|\/)[ ]*|[()]/)
                for (i = 1; i <= n; i++) {
                    if (licenses[i] ~ pattern) { print $1 " (" licenses[i] ")"; break }
                }
            }' |
        sort -u || true)
    if [[ -n $found ]]; then
        echo "error: $crate reaches a copyleft dependency:" >&2
        echo "$found" | sed 's/^/  /' >&2
        status=1
    fi
done

if [[ $status -ne 0 ]]; then
    cat >&2 <<'EOF'

What a service deploys must carry no copyleft code. deny.toml grants exactly one
exception — lightningcss, MPL-2.0 — and grants it on the grounds that the crate
is confined to `esdev`, which is not in the list above.

Either move the dependency back behind `esdev`, or, if it genuinely belongs in a
deployed crate, revisit the exception in deny.toml rather than this list. The
license reasoning there is what changes; this script only reports that it has.
EOF
    exit 1
fi

echo "no copyleft dependency reaches a deployed crate (${#DEPLOYED[@]} checked)"
