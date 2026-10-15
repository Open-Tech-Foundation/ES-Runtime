#!/usr/bin/env bash
# Runs a command with the machine's memory and disk printed beside it.
#
# The Ubuntu test jobs end "cancelled" partway through `tsr test`, with no
# error, on every run: the shape of a runner that ran out of memory or disk
# and was stopped, which takes its log's explanation with it. This prints
# what is left of both at the start and every 30 seconds into the same log,
# so the last lines before the cut say which one ran out and what was using it.
#
#   bash scripts/with-resource-log.sh tsr test
#
# Linux only (`free`, `ps --sort`). The command's exit status is this script's.
set -uo pipefail

interval="${RESOURCE_LOG_INTERVAL:-30}"

snapshot() {
  echo "::group::resources $(date -u +%H:%M:%S)"
  free -m | sed -n '1,2p'
  df -h / "$PWD" 2>/dev/null | awk '!seen[$0]++'
  echo "largest processes (RSS MB, command):"
  ps -eo rss=,args= --sort=-rss | head -5 | awk '{ $1 = int($1 / 1024); print "  " substr($0, 1, 160) }'
  echo "::endgroup::"
}

echo "cores: $(nproc)"
snapshot

# One line per interval outside a group too, so a log read without
# expanding anything still shows the trend up to the cut.
(
  while sleep "$interval"; do
    read -r _ total used _ _ _ available < <(free -m | sed -n 2p)
    disk=$(df -h "$PWD" | awk 'NR == 2 { print $4 " free of " $2 }')
    top=$(ps -eo rss=,comm= --sort=-rss | head -1 | awk '{ print $2 " " int($1 / 1024) "MB" }')
    echo "[resources $(date -u +%H:%M:%S)] memory ${used}/${total}MB used, ${available}MB available; disk ${disk}; largest ${top}" >&2
  done
) &
monitor=$!
trap 'kill "$monitor" 2>/dev/null' EXIT

"$@"
status=$?

snapshot
exit "$status"
