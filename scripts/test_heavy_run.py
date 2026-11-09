"""Tests for scripts/heavy-run.sh: one heavy run at a time for all the worktrees of a clone."""

import fcntl
import os
import shutil
import signal
import subprocess
import tempfile
import time
import unittest
from collections.abc import Callable
from pathlib import Path
from typing import override

SCRIPT = Path(__file__).absolute().parent / "heavy-run.sh"
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
# SISU_HEAVY_LOCK=skip in the shell would let the tests that wait for the lock run at once.
ENV = {k: v for k, v in os.environ.items() if not k.startswith("GIT_") and k != "SISU_HEAVY_LOCK"}


def wait_for(condition: Callable[[], bool], what: str) -> None:
    deadline = time.monotonic() + 20
    while not condition():
        if time.monotonic() > deadline:
            raise AssertionError(f"gave up waiting for {what}")
        time.sleep(0.02)


class HeavyRun(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.repo = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.repo)
        subprocess.run(["git", "init", "-q", self.repo], check=True, env=ENV)
        self.lock = self.repo / ".git" / "sisu-heavy-run.lock"
        self.err = self.repo / "err"

    def path(self, name: str) -> Path:
        return self.repo / name

    def start(
        self, *command: str, cwd: Path | None = None, env: dict[str, str] | None = None
    ) -> subprocess.Popen[bytes]:
        with self.err.open("w", encoding="utf-8") as err:
            run = subprocess.Popen(
                ["bash", SCRIPT, *command],
                cwd=cwd or self.repo,
                env={**ENV, **(env or {})},
                stderr=err,
                start_new_session=True,
            )
        self.addCleanup(self.stop, run)
        return run

    @staticmethod
    def stop(run: subprocess.Popen[bytes]) -> None:
        if run.poll() is None:
            os.killpg(run.pid, signal.SIGKILL)
        run.wait()

    def stderr(self) -> str:
        with self.err.open(encoding="utf-8") as f:
            return f.read()

    def test_a_run_should_wait_for_the_run_that_holds_the_lock_and_name_it(self) -> None:
        with self.lock.open("w", encoding="utf-8") as held:
            held.write("make test in /elsewhere (pid 1)\n")
            held.flush()
            fcntl.flock(held, fcntl.LOCK_EX)
            run = self.start("touch", "ran")
            # The script writes the two lines with two `echo`s, so wait for the second one.
            wait_for(lambda: "when the lock is free" in self.stderr(), "the waiting message")
            self.assertIn(
                "heavy-run: waiting for this run to end: make test in /elsewhere", self.stderr()
            )
            self.assertIn("heavy-run: the run starts when the lock is free", self.stderr())
            self.assertNotIn("SISU_HEAVY_LOCK", self.stderr())
            self.assertIsNone(run.poll())
            self.assertFalse(self.path("ran").exists())
        self.assertEqual(run.wait(timeout=20), 0)
        self.assertTrue(self.path("ran").exists())

    def test_a_run_in_a_linked_worktree_should_wait_for_the_lock_of_the_main_worktree(self) -> None:
        git = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run([*git, "commit", "-q", "--allow-empty", "-m", "base"], check=True, env=ENV)
        linked = self.path("linked")
        subprocess.run([*git, "worktree", "add", "-q", "--detach", linked], check=True, env=ENV)
        with self.lock.open("w", encoding="utf-8") as held:
            held.write("make test in the main worktree (pid 1)\n")
            held.flush()
            fcntl.flock(held, fcntl.LOCK_EX)
            run = self.start("touch", "ran", cwd=linked)
            wait_for(lambda: "heavy-run: waiting" in self.stderr(), "the waiting message")
            self.assertFalse((linked / "ran").exists())
        self.assertEqual(run.wait(timeout=20), 0)
        self.assertTrue((linked / "ran").exists())

    def test_a_run_should_hold_the_lock_until_its_command_ends(self) -> None:
        os.mkfifo(self.path("go"))
        self.start("sh", "-c", "touch ready; read line < go")
        wait_for(lambda: self.path("ready").exists(), "the command to start")
        with self.lock.open(encoding="utf-8") as other:
            self.assertIn("sh -c touch ready; read line < go in ", other.read())
            with self.assertRaises(BlockingIOError):
                fcntl.flock(other, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_a_run_with_the_skip_variable_should_run_at_once_while_another_holds_the_lock(
        self,
    ) -> None:
        with self.lock.open("w+", encoding="utf-8") as held:
            held.write("make mutation-test in /elsewhere (pid 1)\n")
            held.flush()
            fcntl.flock(held, fcntl.LOCK_EX)
            run = self.start("touch", "ran", env={"SISU_HEAVY_LOCK": "skip"})
            self.assertEqual(run.wait(timeout=20), 0)
            self.assertTrue(self.path("ran").exists())
            self.assertIn("heavy-run: SISU_HEAVY_LOCK=skip; running with no lock", self.stderr())
            held.seek(0)
            self.assertEqual(held.read(), "make mutation-test in /elsewhere (pid 1)\n")

    def status(self) -> str:
        return subprocess.run(
            ["bash", SCRIPT, "--status"],
            cwd=self.repo,
            env=ENV,
            capture_output=True,
            text=True,
            check=True,
        ).stdout

    def test_status_should_name_the_run_that_holds_the_lock(self) -> None:
        with self.lock.open("w", encoding="utf-8") as held:
            held.write("make test in /elsewhere (pid 1)\n")
            held.flush()
            fcntl.flock(held, fcntl.LOCK_EX)
            self.assertEqual(self.status(), "held: make test in /elsewhere (pid 1)\n")

    def test_status_should_say_free_when_only_the_text_of_an_ended_run_stays(self) -> None:
        self.lock.write_text("make test in /elsewhere (pid 1)\n", encoding="utf-8")
        self.assertEqual(self.status(), "free\n")

    def test_status_should_say_free_when_no_run_made_the_lock_file(self) -> None:
        self.assertEqual(self.status(), "free\n")

    def test_status_in_a_shell_that_sources_the_script_should_not_keep_the_lock(self) -> None:
        # mutants.sh sources the script, and the shell takes the lock later.
        self.lock.write_text("make test in /elsewhere (pid 1)\n", encoding="utf-8")
        script = 'source "$1"; heavy_status; exec {f}<"$2"; flock -n "$f" && echo locked'
        run = subprocess.run(
            ["bash", "-c", script, "bash", SCRIPT, self.lock],
            cwd=self.repo,
            env=ENV,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(run.stdout, "free\nlocked\n")

    def test_a_run_with_no_git_directory_should_warn_and_run_with_no_lock(self) -> None:
        shutil.rmtree(self.repo / ".git")
        run = self.start("touch", "ran")
        self.assertEqual(run.wait(timeout=20), 0)
        self.assertIn("heavy-run: no lock in the clone's .git; running with no lock", self.stderr())
        self.assertTrue(self.path("ran").exists())


if __name__ == "__main__":
    unittest.main()
