#!/usr/bin/env python3
"""21-run wall clock and ru_maxrss for locus whoami, exec, and status.

Drops the first run. The child inherits stdout and stderr, matching the
numbers in docs/exec-latency.md. stdin is /dev/null.

Each child is started with the caller's LOCUS_HOME and a temporary HOME.
The script refuses to run unless LOCUS_HOME is set to a path other than
~/.locus, and it requires the locus binary as an argument.

Usage (live pin, same shell that ran `locus enter`):

    LOCUS_HOME=/tmp/locus-bench python3 scripts/bench_exec.py /path/to/locus
"""

import os
import shutil
import statistics
import sys
import tempfile
import time


def measure(argv, env):
    start = time.perf_counter()
    pid = os.fork()
    if pid == 0:
        try:
            fd = os.open("/dev/null", os.O_RDONLY)
            os.dup2(fd, 0)
            os.execvpe(argv[0], argv, env)
        except OSError:
            os._exit(127)
    _pid, status, ru = os.wait4(pid, 0)
    wall = time.perf_counter() - start
    code = os.waitstatus_to_exitcode(status)
    return wall, ru.ru_maxrss, code


def series(name, argv, env, runs=21):
    samples = []
    for i in range(runs):
        wall, _rss, code = measure(argv, env)
        if code != 0:
            print(
                f"error: {name} run {i + 1} of {runs} exited {code} "
                f"after {wall:.6f}s; no median",
                file=sys.stderr,
            )
            return 1
        if i == 0:
            continue
        samples.append((wall, _rss))
    walls = [w for w, _ in samples]
    rsss = [r for _, r in samples]
    print(
        f"{name}\tmin={min(walls):.6f}\tmedian={statistics.median(walls):.6f}\t"
        f"max={max(walls):.6f}\trss_min={min(rsss)}\trss_med={statistics.median(rsss):.0f}\t"
        f"rss_max={max(rsss)}\texits=[0]\tn={len(walls)}"
    )
    return 0


def require_locus_home():
    raw = os.environ.get("LOCUS_HOME", "")
    if not raw:
        print(
            "error: set LOCUS_HOME to a temporary directory (not ~/.locus)",
            file=sys.stderr,
        )
        return None
    home = os.path.realpath(raw)
    default = os.path.realpath(os.path.expanduser("~/.locus"))
    if home == default:
        print("error: LOCUS_HOME must not be ~/.locus", file=sys.stderr)
        return None
    return home


def main():
    if len(sys.argv) != 2:
        print(
            "usage: python3 scripts/bench_exec.py /path/to/locus",
            file=sys.stderr,
        )
        return 2
    locus = sys.argv[1]
    if os.path.sep not in locus and (
        os.path.altsep is None or os.path.altsep not in locus
    ):
        print(
            "error: pass the locus binary as a path, not a bare name",
            file=sys.stderr,
        )
        return 2
    if not os.path.isfile(locus):
        print(f"error: locus binary not found: {locus}", file=sys.stderr)
        return 2
    # Absolute path so execvpe runs this file and does not search PATH.
    locus = os.path.abspath(locus)
    locus_home = require_locus_home()
    if locus_home is None:
        return 2
    temp_home = tempfile.mkdtemp(prefix="locus-bench-home-")
    env = os.environ.copy()
    env["LOCUS_HOME"] = locus_home
    env["HOME"] = temp_home
    try:
        for name, args in (
            ("whoami", [locus, "whoami"]),
            ("exec", [locus, "exec", "--", "true"]),
            ("status", [locus, "status", "--oneline"]),
        ):
            if series(name, args, env) != 0:
                return 1
    finally:
        shutil.rmtree(temp_home, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
