#!/usr/bin/env bash
# Asserts that no copyleft dependency can reach a deployed service.
#
# `deny.toml` allows only permissive licences and carries no exceptions, so
# today this passes trivially. It is kept anyway, because the check it makes is
# one `cargo deny` cannot: *where in the graph* a licence sits.
#
# The history is the argument for it. The CSS pipeline was first built on
# lightningcss, which is MPL-2.0, and the case for accepting it rested entirely
# on placement — copyleft that is per-file, in a crate linked only into `esdev`,
# never into the `esrun` a service deploys. That reasoning was sound and it was
# also one moved dependency line away from silently ceasing to be true, with
# nothing to notice. lightningcss is gone (D67) and the reasoning is no longer
# load-bearing, but the next crate worth an exception will make the same
# argument, and this is what holds it to it.
#
# A script rather than a `run =` line in tasks.toml: the check is a pipeline and
# tsr's mini-shell has no pipes by design.
set -euo pipefail

# The binaries a service deploys. `esdev` is deliberately absent: it is the
# development toolchain, and the one place an exception would ever be argued
# for.
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

What a service deploys must carry no copyleft code. deny.toml currently grants
no exceptions at all, so a hit here means a new dependency brought one in.

Either move it behind `esdev`, which is not in the list above, or — if it
genuinely belongs in a deployed crate — argue the exception in deny.toml rather
than editing this list. The licence reasoning belongs there; this script only
reports when it has stopped holding.
EOF
    exit 1
fi

echo "no copyleft dependency reaches a deployed crate (${#DEPLOYED[@]} checked)"
