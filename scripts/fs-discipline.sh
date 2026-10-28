#!/usr/bin/env bash
# Refuses direct filesystem I/O outside the files that are allowed it (D180).
#
# A path the program names reaches a file through one `Jail`
# (`crates/default-providers/src/jail.rs`), via `SystemFileSystem`,
# `SystemSyncFileSystem` or the jail itself. A new `std::fs`/`tokio::fs` call
# anywhere else in the crates that run guest code is how a door onto the
# filesystem gets added without the jail, the scope lists or the anchoring —
# the security bug that arrives looking like an ordinary feature.
#
# So every file that touches the filesystem directly is listed below with the
# reason it may. A file not on the list fails the check. Adding one is a review
# decision, made here where the reason is written down, not a side effect of a
# feature: if the path can come from guest code, go through the `Jail` instead.
#
# Scope: the crates in the runtime's dependency chain. `dev-cli` is development
# tooling acting on the developer's own project, outside this boundary; the
# guest code it runs uses the same `cli-common` wiring and therefore the jail.
set -euo pipefail

cd "$(dirname "$0")/.."

crates=(common engine providers runtime default-providers cli-common runtime-cli/src)

pattern='std::fs::|tokio::fs::|\bfs::(read|write|File|OpenOptions|create_dir|remove_|rename|copy|metadata|symlink_metadata|read_dir|read_link|hard_link|set_permissions)|File::(open|create)\(|OpenOptions::new\(|os::(unix|windows)::fs::symlink'

# file|reason — one line each. Keep the reason honest: it is what a reviewer
# reads when deciding whether a new entry belongs here.
allowed='
crates/default-providers/src/jail.rs|the jail: resolution and its checks
crates/default-providers/src/jail/anchor.rs|the jail: acting by descriptor on what it admitted
crates/default-providers/src/jail/tree.rs|the jail: a pinned directory written beneath (D180)
crates/default-providers/src/system_fs.rs|a door: every operation resolves through its Jail first
crates/default-providers/src/system_sync_fs.rs|a door: every operation resolves through its Jail first
crates/default-providers/src/system_db.rs|a door: the database VFS opens through its Jail; the rest is tests
crates/default-providers/src/path.rs|the one path module (D165): canonicalize
crates/default-providers/src/path_allowlist.rs|parses the command-line scope lists (host input)
crates/default-providers/src/modules.rs|module loading, under the import policy (D21/D40)
crates/default-providers/src/node_modules.rs|module resolution, confined with jail::confine
crates/default-providers/src/system_command.rs|executable lookup on PATH for an allowed command (D38); the rest is tests
crates/default-providers/src/system_net.rs|Unix socket files, under the socket path list (D140), not the file jail
crates/default-providers/src/dns/config.rs|/etc/resolv.conf for the name servers to ask (D188), a fixed path
crates/cli-common/src/run.rs|reads the entry module the operator named
crates/cli-common/src/dotenv.rs|reads the --env-file the operator named
crates/cli-common/src/sourcemap.rs|reads source maps beside modules already loaded
crates/runtime/src/lib.rs|/proc/self/statm for memory stats; the rest is tests
crates/runtime/src/mongodb/spec_tests.rs|test-only: reads the fetched MongoDB specification suites
crates/engine/src/sysinfo.rs|/proc/self/status for process stats
crates/engine/src/sysmem.rs|/proc and cgroup files for memory limits
'

dirs=()
for c in "${crates[@]}"; do dirs+=("crates/$c"); done

files=$(grep -rlE --include='*.rs' "$pattern" "${dirs[@]}" | sort -u)

bad=''
for f in $files; do
    case "$f" in
        */tests/*) continue ;; # integration tests drive the binary from outside
        */build.rs) continue ;; # runs at compile time, before any guest exists
    esac
    if ! grep -q "^$f|" <<<"$allowed"; then
        bad+="$f"$'\n'
    fi
done

stale=''
while IFS='|' read -r f _; do
    [[ -z "$f" ]] && continue
    if ! grep -qx "$f" <<<"$files"; then
        stale+="$f"$'\n'
    fi
done <<<"$allowed"

if [[ -n "$bad" ]]; then
    echo "Direct filesystem I/O outside the jail (D180):"
    echo
    printf '%s' "$bad"
    echo
    echo "A path guest code can name must go through the Jail — SystemFileSystem,"
    echo "SystemSyncFileSystem, or es_runtime_default_providers::Jail. If this file"
    echo "only reads host input the guest cannot steer, add it to"
    echo "scripts/fs-discipline.sh with the reason."
    exit 1
fi
if [[ -n "$stale" ]]; then
    echo "fs-discipline: these listed files no longer touch the filesystem; remove them:"
    printf '%s' "$stale"
    exit 1
fi
echo "fs-discipline: filesystem I/O only where it is allowed"
