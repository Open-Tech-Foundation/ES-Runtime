#!/usr/bin/env bash
# What an isolate costs, and what it costs to run many on a few threads
# (DECISIONS D227). The numbers on the workers internals page come from here.
#
#   bash bench/probe-isolates.sh [ISOLATES] [THREADS]
#
# Runs two release-built probes:
#
#   isolate_density  memory and start time of idle isolates: a bare V8 context,
#                    and one restored from the prelude snapshot every agent uses
#   isolate_locker   per-turn cost of many isolates on few threads: entered and
#                    left on the thread that owns them (what the pool does),
#                    and under v8::Locker, pinned and migrating
#   isolate-start.js an Isolate's start, cold and from Isolate.prepare, through
#                    a release esrun
#
# Linux only for the memory rows: they read /proc/self/status.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISOLATES="${1:-1000}"
THREADS="${2:-8}"

cd "$ROOT"
cargo build --release --quiet --example isolate_density -p es-runtime-default-providers
cargo build --release --quiet --example isolate_locker -p es-runtime-engine
cargo build --release --quiet -p es-runtime-cli

echo "## isolate_density $ISOLATES"
target/release/examples/isolate_density "$ISOLATES"
echo
echo "## isolate_locker $ISOLATES $THREADS"
target/release/examples/isolate_locker "$ISOLATES" "$THREADS"
echo
echo "## isolate-start.js"
target/release/esrun --allow-workers bench/isolate-start.js
