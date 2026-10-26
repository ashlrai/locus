#!/usr/bin/env python3
"""Pinned dispatch and per-cycle cleanup contract tests; no live sessions."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import bridge_cycle as bridge


class BridgeCycleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.state = self.root / "state"
        self.manifest = self.root / "published.json"
        self.manifest.write_text("[]")
        self.binary = self.root / "locus"
        shutil.copyfile(Path(sys.executable).resolve(), self.binary)
        self.binary.chmod(0o700)
        self.digest = hashlib.sha256(self.binary.read_bytes()).hexdigest()
        self.environment = patch.dict(os.environ, {"HOME": str(self.root)})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def command(self, arguments=None, **changes):
        params = dict(
            codex="codex",
            arguments=arguments or [],
            locus=self.binary,
            locus_sha256=self.digest,
            mcp=self.binary,
            mcp_sha256=self.digest,
            server="locus",
        )
        params.update(changes)
        return bridge.codex_command(**params)

    def records(self):
        return [
            json.loads(line)
            for line in (self.state / "worktree-reaper.jsonl").read_text().splitlines()
        ]

    def test_supervised_launch_overrides_command_and_args_only(self):
        command = self.command(["--", "prompt"])
        self.assertEqual(command[:2], ["codex", "exec"])
        self.assertEqual(
            command[3], f"mcp_servers.locus.command={json.dumps(str(self.binary))}"
        )
        launch = json.loads(command[5].split("=", 1)[1])
        self.assertEqual(launch, ["mcp", "--supervised", "--mcp-bin", str(self.binary)])
        self.assertEqual(command[-2:], ["--", "prompt"])
        self.assertFalse(any("env=" in arg or "npx" in arg for arg in command))

    def test_custom_server_name(self):
        self.assertIn("mcp_servers.ashlr.command=", self.command(server="ashlr")[3])
        with self.assertRaises(bridge.Refused):
            self.command(server="locus.env")

    def test_overrides_cannot_bypass_pin(self):
        for arguments in (
            ["-c", "mcp_servers={}"],
            ["--config=mcp_servers={}"],
            ["--config", "mcp_servers={}"],
            ["-cmcp_servers={}"],
        ):
            with self.subTest(arguments=arguments), self.assertRaises(bridge.Refused):
                self.command(arguments)
        # Literal prompts may contain text that looks like options.
        self.assertEqual(self.command(["--", "--config=prompt"])[-1], "--config=prompt")

    def test_refuses_wrappers_aliases_relative_paths_and_package_cache(self):
        wrapper = self.root / "wrapper"
        wrapper.write_text("#!/bin/sh\nexit 0\n")
        wrapper.chmod(0o700)
        alias = self.root / "alias"
        alias.symlink_to(self.binary)
        cache = self.root / "_npx"
        cache.mkdir()
        cached = cache / "locus"
        shutil.copyfile(self.binary, cached)
        cached.chmod(0o700)
        fifo = self.root / "fifo"
        os.mkfifo(fifo, 0o700)
        for path in (wrapper, alias, Path("locus"), cached, fifo):
            with self.subTest(path=path.name), self.assertRaises(bridge.Refused):
                self.command(locus=path)

    def test_both_artifacts_require_exact_digest(self):
        for key in ("locus_sha256", "mcp_sha256"):
            for digest in ("0" * 64, "not-a-hash"):
                with self.subTest(key=key), self.assertRaises(bridge.Refused):
                    self.command(**{key: digest})

    def test_twenty_dispatches_reap_and_log_each_cycle(self):
        # This verifies the bridge's dispatch boundary, not paid Codex sessions.
        with patch.object(
            bridge.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)
        ) as run:
            result = bridge.run_loop(
                lambda: bridge.subprocess.run(self.command()).returncode,
                self.manifest,
                self.root,
                self.state,
                cycles=20,
            )
        self.assertEqual(result, 0)
        self.assertEqual(run.call_count, 20)
        records = self.records()
        self.assertEqual(len(records), 20)
        for record in records:
            self.assertIn("before", record)
            self.assertIn("after", record)

    def test_idle_and_failed_dispatches_still_reap(self):
        statuses = iter([None, 7, 0])
        result = bridge.run_loop(
            lambda: next(statuses), self.manifest, self.root, self.state, cycles=3
        )
        self.assertEqual(result, 7)
        self.assertEqual(len(self.records()), 3)

    def test_exception_and_interrupt_still_reap(self):
        for error in (
            OSError("synthetic"),
            KeyboardInterrupt(),
            bridge.Refused("synthetic"),
        ):

            def dispatch():
                raise error

            with self.assertRaises(type(error)):
                bridge.run_loop(dispatch, self.manifest, self.root, self.state)
        self.assertEqual(len(self.records()), 3)

    def test_invalid_bounds_do_not_dispatch(self):
        for cycles, interval in ((0, 0), (1, -1), (1, float("nan")), (1, float("inf"))):
            with self.assertRaises(bridge.Refused):
                bridge.run_loop(
                    lambda: self.fail("dispatched invalid bounds"),
                    self.manifest,
                    self.root,
                    self.state,
                    cycles=cycles,
                    interval=interval,
                )

    def test_cli_pins_real_child_argv_before_prompt_separator(self):
        fake_codex = self.root / "codex"
        output = self.root / "argv.json"
        fake_codex.write_text(
            f"#!{sys.executable}\nimport json,sys\nfrom pathlib import Path\n"
            f"Path({str(output)!r}).write_text(json.dumps(sys.argv[1:]))\n"
        )
        fake_codex.chmod(0o700)
        completed = subprocess.run(
            [
                sys.executable,
                str(Path(bridge.__file__).resolve()),
                "--manifest",
                str(self.manifest),
                "--worktree-root",
                str(self.root),
                "--state-dir",
                str(self.state),
                "--locus-bin",
                str(self.binary),
                "--locus-sha256",
                self.digest,
                "--mcp-bin",
                str(self.binary),
                "--mcp-sha256",
                self.digest,
                "--codex-bin",
                str(fake_codex),
                "--",
                "--",
                "prompt",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(
            json.loads(output.read_text()), self.command(["--", "prompt"])[1:]
        )
        self.assertEqual(len(self.records()), 1)

    def test_owned_process_group_closes_orphan_on_success_and_timeout(self):
        for timeout in (None, 2):
            ready = self.root / "child-ready"
            stopped = self.root / "child-stopped"
            ready.unlink(missing_ok=True)
            stopped.unlink(missing_ok=True)
            descendant = (
                "import signal,time,sys; from pathlib import Path; "
                f"signal.signal(signal.SIGTERM, lambda *_: (Path({str(stopped)!r}).touch(), sys.exit(0))); "
                f"Path({str(ready)!r}).touch(); time.sleep(60)"
            )
            leader = (
                "import subprocess,sys,time; from pathlib import Path; "
                f"subprocess.Popen([sys.executable, '-c', {descendant!r}]); "
                f"ready=Path({str(ready)!r})\n"
                "while not ready.exists(): time.sleep(0.01)\n"
                + ("time.sleep(60)\n" if timeout else "")
            )
            command = [sys.executable, "-c", leader]
            if timeout:
                with self.assertRaises(subprocess.TimeoutExpired):
                    bridge.dispatch_codex(command, timeout=timeout)
            else:
                self.assertEqual(bridge.dispatch_codex(command), 0)
            self.assertTrue(
                stopped.exists(), "owned descendant did not receive shutdown"
            )

    def test_process_group_cleanup_escalates_and_reaps_leader(self):
        ready = self.root / "ignoring-term"
        command = [
            sys.executable,
            "-c",
            "import signal,time; from pathlib import Path; "
            "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
            f"Path({str(ready)!r}).touch(); time.sleep(60)",
        ]
        started = time.monotonic()
        with self.assertRaises(subprocess.TimeoutExpired):
            bridge.dispatch_codex(command, timeout=0.5)
        self.assertTrue(ready.exists())
        self.assertLess(time.monotonic() - started, 5)

    def test_failed_spawn_never_signals_any_group(self):
        with patch.object(bridge.os, "killpg") as kill:
            with self.assertRaises(OSError):
                bridge.dispatch_codex([str(self.root / "missing")])
        kill.assert_not_called()

    def test_cli_sigterm_closes_session_and_logs_cycle(self):
        ready = self.root / "codex-ready"
        stopped = self.root / "codex-stopped"
        fake_codex = self.root / "codex"
        fake_codex.write_text(
            f"#!{sys.executable}\nimport signal,sys,time\nfrom pathlib import Path\n"
            f"signal.signal(signal.SIGTERM, lambda *_: (Path({str(stopped)!r}).touch(), sys.exit(0)))\n"
            f"Path({str(ready)!r}).touch()\ntime.sleep(60)\n"
        )
        fake_codex.chmod(0o700)
        child = subprocess.Popen(
            [
                sys.executable,
                str(Path(bridge.__file__).resolve()),
                "--manifest",
                str(self.manifest),
                "--worktree-root",
                str(self.root),
                "--state-dir",
                str(self.state),
                "--locus-bin",
                str(self.binary),
                "--locus-sha256",
                self.digest,
                "--mcp-bin",
                str(self.binary),
                "--mcp-sha256",
                self.digest,
                "--codex-bin",
                str(fake_codex),
                "--",
                "prompt",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
        try:
            deadline = time.monotonic() + 5
            while (
                not ready.exists()
                and child.poll() is None
                and time.monotonic() < deadline
            ):
                time.sleep(0.01)
            self.assertTrue(ready.exists(), "fake Codex did not start")
            child.terminate()
            self.assertEqual(child.wait(timeout=5), 143, child.stderr.read().decode())
            self.assertTrue(stopped.exists())
            self.assertEqual(len(self.records()), 1)
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
            child.stderr.close()

    def test_repeated_shutdown_signals_cannot_interrupt_cleanup(self):
        original = {
            sig: signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGHUP)
        }
        completed = []

        def cleanup(*_args):
            # Deliver synchronously in the finally block, without scheduling or
            # sleeps. Both signals must be ignored until cleanup has finished.
            signal.raise_signal(signal.SIGTERM)
            signal.raise_signal(signal.SIGHUP)
            completed.append(True)

        arguments = [
            "bridge-cycle",
            "--manifest",
            str(self.manifest),
            "--worktree-root",
            str(self.root),
            "--state-dir",
            str(self.state),
            "--locus-bin",
            str(self.binary),
            "--locus-sha256",
            self.digest,
            "--mcp-bin",
            str(self.binary),
            "--mcp-sha256",
            self.digest,
        ]
        with (
            patch.object(sys, "argv", arguments),
            patch.object(
                bridge,
                "dispatch_codex",
                side_effect=lambda *_args, **_kwargs: signal.raise_signal(
                    signal.SIGTERM
                ),
            ),
            patch.object(bridge, "run_cycle", side_effect=cleanup),
        ):
            with self.assertRaises(SystemExit) as raised:
                bridge.main()
        self.assertEqual(raised.exception.code, 143)
        self.assertEqual(completed, [True])
        self.assertEqual({sig: signal.getsignal(sig) for sig in original}, original)


if __name__ == "__main__":
    unittest.main()
