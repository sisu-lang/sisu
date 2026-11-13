"""Unit tests for coverage.sh: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script in a small repository with a fake `cargo` on PATH and a stub
coverage_report.py, so no instrumented run happens.
"""

import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import override

SCRIPT = Path(__file__).absolute().parent / "coverage.sh"
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
# The tests set NEXTEST_PROFILE themselves.
ENV = {k: v for k, v in os.environ.items() if not k.startswith("GIT_") and k != "NEXTEST_PROFILE"}
# The fake cargo does nothing, except that `nextest` writes its NEXTEST_PROFILE to PROFILE_LOG
# and runs the action that the test names in FAKE_ACTION: it edits the tracked file or it makes
# a commit.
FAKE_CARGO = """#!/usr/bin/env bash
[[ $2 == nextest ]] || exit 0
printf '%s' "${NEXTEST_PROFILE-unset}" >"$PROFILE_LOG"
case ${FAKE_ACTION:-} in
  edit) echo changed >a.txt ;;
  commit) echo changed >a.txt && git -c user.name=t -c user.email=t@t commit -q -am during ;;
esac
"""
STUB_REPORT = "print('Coverage: stub')\n"


class CoverageSh(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        scripts = Path(self.repo, "scripts")
        scripts.mkdir()
        shutil.copy(SCRIPT, scripts)
        with (scripts / "coverage_report.py").open("w", encoding="utf-8") as f:
            f.write(STUB_REPORT)
        bin_dir = Path(self.repo, "bin")
        bin_dir.mkdir()
        cargo = bin_dir / "cargo"
        with cargo.open("w", encoding="utf-8") as f:
            f.write(FAKE_CARGO)
        cargo.chmod(cargo.stat().st_mode | stat.S_IXUSR)
        # Untracked, so the write does not change the marker.
        self.profile_log = bin_dir / "profile"
        self.env = dict(ENV, PATH=f"{bin_dir}:{ENV['PATH']}", PROFILE_LOG=str(self.profile_log))
        self.git = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run([*self.git[:3], "init", "-q"], check=True, env=ENV)
        self.write_tracked("base\n")
        subprocess.run([*self.git, "add", "-A"], check=True, env=ENV)
        subprocess.run([*self.git, "commit", "-q", "-m", "base"], check=True, env=ENV)
        self.head = self.rev_parse()

    def write_tracked(self, text: str) -> None:
        with Path(self.repo, "a.txt").open("w", encoding="utf-8") as f:
            f.write(text)

    def rev_parse(self) -> str:
        return subprocess.run(
            [*self.git, "rev-parse", "HEAD"], capture_output=True, text=True, check=True, env=ENV
        ).stdout.strip()

    def measure(self, action: str = "", extra: dict[str, str] | None = None) -> str:
        run = subprocess.run(
            ["bash", "scripts/coverage.sh", "HEAD"],
            cwd=self.repo,
            capture_output=True,
            text=True,
            env=dict(self.env, FAKE_ACTION=action, **(extra or {})),
            check=False,
        )
        self.assertEqual(run.returncode, 0, run.stderr)
        # The marker line comes before the report, which CI copies from `Coverage:` on.
        marker, report = run.stdout.splitlines()
        self.assertEqual(report, "Coverage: stub")
        return marker

    def test_a_clean_tree_should_give_the_clean_marker(self) -> None:
        self.assertEqual(self.measure(), f"coverage: measured (HEAD {self.head})")

    def test_a_change_before_the_run_should_give_the_dirty_marker(self) -> None:
        self.write_tracked("changed\n")
        self.assertEqual(
            self.measure(), f"coverage: measured (HEAD {self.head} with uncommitted changes)"
        )

    def test_a_change_during_the_run_should_give_the_dirty_marker(self) -> None:
        self.assertEqual(
            self.measure("edit"), f"coverage: measured (HEAD {self.head} with uncommitted changes)"
        )

    def test_a_commit_during_the_run_should_give_the_dirty_marker_with_the_first_head(self) -> None:
        marker = self.measure("commit")
        self.assertNotEqual(self.rev_parse(), self.head)
        self.assertEqual(marker, f"coverage: measured (HEAD {self.head} with uncommitted changes)")

    def test_an_unset_profile_should_run_the_ci_profile(self) -> None:
        self.measure()
        self.assertEqual(self.profile_log.read_text(encoding="utf-8"), "ci")

    def test_a_set_profile_should_run_that_profile(self) -> None:
        self.measure(extra={"NEXTEST_PROFILE": "default"})
        self.assertEqual(self.profile_log.read_text(encoding="utf-8"), "default")


if __name__ == "__main__":
    unittest.main()
