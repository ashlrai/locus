#!/usr/bin/env python3
"""21-run wall clock and ru_maxrss for locus whoami, exec, and status.

Drops the first run. The child inherits stdout and stderr, matching the
numbers in docs/exec-latency.md. stdin is /dev/null.

Usage (live pin, same shell that ran `locus enter`):

    python3 scripts/bench_exec.py /path/to/locus
"""

import os
import statistics
import sys
import time


def measure(argv):
    start = time.perf_counter()
    pid = os.fork()
    if pid == 0:
        try:
            fd = os.open("/dev/null", os.O_RDONLY)
            os.dup2(fd, 0)
            os.execvpe(argv[0], argv, os.environ)
        except OSError:
            os._exit(127)
    _pid, status, ru = os.wait4(pid, 0)
    wall = time.perf_counter() - start
    code = os.waitstatus_to_exitcode(status)
    return wall, ru.ru_maxrss, code


def series(name, argv, runs=21):
    samples = []
    codes = []
    for i in range(runs):
        wall, rss, code = measure(argv)
        codes.append(code)
        if i == 0:
            continue
        samples.append((wall, rss))
    walls = [w for w, _ in samples]
    rsss = [r for _, r in samples]
    print(
        f"{name}\tmin={min(walls):.6f}\tmedian={statistics.median(walls):.6f}\t"
        f"max={max(walls):.6f}\trss_min={min(rsss)}\trss_med={statistics.median(rsss):.0f}\t"
        f"rss_max={max(rsss)}\texits={sorted(set(codes))}\tn={len(walls)}"
    )


def main():
    locus = sys.argv[1] if len(sys.argv) > 1 else "locus"
    series("whoami", [locus, "whoami"])
    series("exec", [locus, "exec", "--", "true"])
    series("status", [locus, "status", "--oneline"])


if __name__ == "__main__":
    main()
