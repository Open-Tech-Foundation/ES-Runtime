"""Runs one child process and reports wall/user/sys time and peak RSS.

`/usr/bin/time -v` is not present everywhere, and `bench/run.sh` already falls
back to `resource.getrusage` for peak RSS — this is the same measurement, plus
the CPU split, for a single run. Printed as one JSON object so the shell does
not have to parse three formats.
"""

import json
import resource
import subprocess
import sys
import time

cmd = sys.argv[1:]
before = resource.getrusage(resource.RUSAGE_CHILDREN)
start = time.monotonic()
proc = subprocess.run(cmd, capture_output=True, text=True)
wall = (time.monotonic() - start) * 1000
after = resource.getrusage(resource.RUSAGE_CHILDREN)

print(
    json.dumps(
        {
            "ok": proc.returncode == 0,
            "wall_ms": round(wall, 1),
            # ru_maxrss is the high-water mark across *all* reaped children, so
            # the max is taken rather than the difference: a smaller child after
            # a larger one would otherwise report a negative peak.
            "rss_mb": round(after.ru_maxrss / 1024, 1),
            "user_ms": round((after.ru_utime - before.ru_utime) * 1000, 1),
            "sys_ms": round((after.ru_stime - before.ru_stime) * 1000, 1),
            "out": proc.stdout.strip().splitlines()[-1] if proc.stdout.strip() else "",
            "err": proc.stderr.strip()[-400:],
        }
    )
)
