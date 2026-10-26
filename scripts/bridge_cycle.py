#!/usr/bin/env python3
"""Dispatch bridge Codex cycles with pinned native Locus and mandatory cleanup.

The owning bridge can call run_loop or use this executable for bounded batches.
It must refresh the publication manifest and stop worktree writers before cleanup.
This adapter does not install itself into an existing daemon or change identity.
"""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import stat
import subprocess
import time

from bridge_worktree_reaper import run_cycle


class Refused(ValueError):
    """A fixed, non-sensitive diagnostic suitable for bridge logs."""


NATIVE_MAGIC = {
    b"\x7fELF",  # Linux
    b"\xfe\xed\xfa\xce",
    b"\xce\xfa\xed\xfe",  # Mach-O 32
    b"\xfe\xed\xfa\xcf",
    b"\xcf\xfa\xed\xfe",  # Mach-O 64
    b"\xca\xfe\xba\xbe",
    b"\xbe\xba\xfe\xca",  # Universal Mach-O
    b"\xca\xfe\xba\xbf",
    b"\xbf\xba\xfe\xca",
}


def native_pin(binary, digest):
    """Verify an explicit release artifact, never resolve Locus through PATH.

    The release directory must be controlled by the bridge operator and immutable
    during dispatch. Hash checking cannot prevent a concurrent trusted installer
    replacing the file between this check and Codex's later exec.
    """
    path = Path(binary)
    if not path.is_absolute() or not re.fullmatch(r"[0-9a-fA-F]{64}", digest):
        raise Refused("invalid-native-pin")
    # Refuse every symlink component and the npm cache even if a native binary
    # happens to exist there. Pins name immutable release files, not aliases.
    if path.resolve(strict=True) != path or any(
        part in {"node_modules", "_npx"} for part in path.parts
    ):
        raise Refused("indirect-native-pin")
    # NONBLOCK lets us inspect and refuse special files without blocking on a
    # FIFO; NOFOLLOW closes the final-component symlink race before opening.
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as handle:
        metadata = os.fstat(handle.fileno())
        if not stat.S_ISREG(metadata.st_mode) or not os.access(path, os.X_OK):
            raise Refused("native-pin-not-executable")
        magic = handle.read(4)
        if magic not in NATIVE_MAGIC:
            raise Refused("native-pin-is-wrapper")
        checksum = hashlib.sha256(magic)
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            checksum.update(chunk)
        if checksum.hexdigest() != digest.lower():
            raise Refused("native-pin-digest-mismatch")
    return str(path)


def codex_command(codex, arguments, locus, locus_sha256, mcp, mcp_sha256, server):
    if not re.fullmatch(r"[A-Za-z0-9_-]+", server):
        raise Refused("invalid-server-name")
    # Arguments after -- are literal prompt text; overrides before it would
    # permit replacing our pinned launch table (including broad mcp_servers).
    options = arguments[: arguments.index("--")] if "--" in arguments else arguments
    if any(
        arg == "--config" or arg.startswith("--config=") or arg.startswith("-c")
        for arg in options
    ):
        raise Refused("codex-config-overrides-not-supported")
    cli = native_pin(locus, locus_sha256)
    worker = native_pin(mcp, mcp_sha256)
    return [
        str(codex),
        "exec",
        "-c",
        f"mcp_servers.{server}.command={json.dumps(cli)}",
        "-c",
        f"mcp_servers.{server}.args={json.dumps(['mcp', '--supervised', '--mcp-bin', worker])}",
        *arguments,
    ]


def run_loop(dispatch, manifest, root, state, *, cycles=1, interval=0):
    """Reap on each dispatch, including idle callbacks, errors and interruption.

    A callback may refresh publication metadata before dispatch. Exceptions still
    trigger cleanup and then propagate. A nonzero child exit is retained while
    subsequent requested cycles run. No stdout/stderr or environment is logged.
    """
    if cycles < 1 or not math.isfinite(interval) or interval < 0:
        raise Refused("invalid-cycle-bounds")
    result = 0
    for index in range(cycles):
        try:
            code = dispatch()
            result = result or code or 0
        finally:
            run_cycle(manifest, root, state)
        if index + 1 < cycles and interval:
            time.sleep(interval)
    return result


def dispatch_codex(command, *, timeout=None):
    """Own a process group and close its remaining descendants on every exit.

    Each Codex session gets its own group, so cleanup never uses global process
    names. The native Locus parent watcher covers children that leave this group.
    """
    if timeout is not None and (not math.isfinite(timeout) or timeout <= 0):
        raise Refused("invalid-session-timeout")
    child = subprocess.Popen(command, start_new_session=True)
    try:
        return child.wait(timeout=timeout)
    finally:
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        deadline = time.monotonic() + 0.5
        while time.monotonic() < deadline:
            child.poll()  # Reap the leader, including an interrupted wait.
            try:
                os.killpg(child.pid, 0)
            except ProcessLookupError:
                break
            time.sleep(0.01)
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--worktree-root", required=True, type=Path)
    parser.add_argument("--state-dir", type=Path, default=Path.home() / ".ashlr-runs")
    parser.add_argument("--locus-bin", required=True, type=Path)
    parser.add_argument("--locus-sha256", required=True)
    parser.add_argument("--mcp-bin", required=True, type=Path)
    parser.add_argument("--mcp-sha256", required=True)
    parser.add_argument("--codex-bin", default="codex")
    parser.add_argument("--server-name", default="locus")
    parser.add_argument("--cycles", type=int, default=1)
    parser.add_argument("--interval", type=float, default=0)
    parser.add_argument(
        "--session-timeout",
        type=float,
        help="Optional maximum seconds per Codex session",
    )
    parser.add_argument(
        "--idle", action="store_true", help="Run cleanup cycles without dispatch"
    )
    parser.add_argument(
        "arguments", nargs=argparse.REMAINDER, help="Codex exec arguments after --"
    )
    args = parser.parse_args()
    arguments = args.arguments[1:] if args.arguments[:1] == ["--"] else args.arguments
    stopping = False

    def stop(signum, _frame):
        nonlocal stopping
        if stopping:
            return
        stopping = True
        # A second shutdown request must not interrupt group termination or the
        # reaper's finally block. Main restores the original handlers afterward.
        for shutdown_signal in (signal.SIGTERM, signal.SIGHUP):
            signal.signal(shutdown_signal, signal.SIG_IGN)
        # launchd/bridge shutdown normally arrives as TERM, which otherwise
        # bypasses Python finally blocks and leaves the owned session running.
        raise SystemExit(128 + signum)

    previous_handlers = {
        signum: signal.signal(signum, stop)
        for signum in (signal.SIGTERM, signal.SIGHUP)
    }

    def dispatch():
        if args.idle:
            return 0
        command = codex_command(
            args.codex_bin,
            arguments,
            args.locus_bin,
            args.locus_sha256,
            args.mcp_bin,
            args.mcp_sha256,
            args.server_name,
        )
        return dispatch_codex(command, timeout=args.session_timeout)

    try:
        return run_loop(
            dispatch,
            args.manifest,
            args.worktree_root,
            args.state_dir,
            cycles=args.cycles,
            interval=args.interval,
        )
    except Refused as error:
        parser.exit(2, f"bridge cycle refused: {error}\n")
    except OSError:
        parser.exit(2, "bridge cycle failed: filesystem-or-launch-error\n")
    except subprocess.TimeoutExpired:
        parser.exit(124, "bridge cycle failed: session-timeout\n")
    except KeyboardInterrupt:
        return 130
    finally:
        for signum, handler in previous_handlers.items():
            signal.signal(signum, handler)


if __name__ == "__main__":
    raise SystemExit(main())
