"""Unit tests for review-sisu's review_round.py.

Each test runs the script on a small repository whose origin/main holds fake build and brief
scripts. The fakes write their arguments to files in the run directory.
"""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import override

SCRIPT = Path(__file__).resolve().parents[1] / ".agents/skills/review-sisu/scripts/review_round.py"
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
FAKE_BUILD = 'echo "$@" > "$2/build-args"\n'
FAKE_BRIEF = """import sys
from pathlib import Path
out = Path(sys.argv[3])
out.write_text(" ".join(sys.argv[1:]))
"""


class ReviewRound(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)
        self.repo = self.root / "repo"
        self.run_dir = self.root / "run"
        self.run_dir.mkdir()
        scripts = self.repo / ".agents/skills/review-sisu/scripts"
        scripts.mkdir(parents=True)
        (scripts / "build_sisu.sh").write_text(FAKE_BUILD)
        (scripts / "diff_brief.py").write_text(FAKE_BRIEF)
        self.git("init", "-q", "-b", "main")
        self.git("add", ".")
        self.git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "first")
        self.git("remote", "add", "origin", str(self.repo))

    def git(self, *args: str) -> None:
        subprocess.run(["git", "-C", self.repo, *args], check=True, env=ENV)

    def script(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["python3", SCRIPT, args[0], self.repo, self.run_dir, *args[1:]],
            capture_output=True,
            text=True,
            check=False,
            env=ENV,
        )

    def test_a_first_round_of_a_diff_should_build_both_ends_and_record_the_checkout(self) -> None:
        (self.run_dir / "focus.md").write_text("focus")
        result = self.script("prepare", "main..main", "--doc", "spec.md")
        self.assertEqual(result.returncode, 0, result.stderr)
        built = (self.run_dir / "build-args").read_text().split()
        self.assertEqual(built[2:], ["main", "main"])
        brief = (self.run_dir / "brief.md").read_text()
        self.assertIn(f"--focus {self.run_dir / 'focus.md'}", brief)
        self.assertIn("--doc spec.md", brief)
        self.assertNotIn("--rulings", brief)
        self.assertTrue((self.run_dir / "tree-1.txt").exists())

    def test_a_later_round_should_take_the_rulings_and_the_reports(self) -> None:
        for name in ("rulings-1.md", "sol.md", "opus.md", "focus-2.md"):
            (self.run_dir / name).write_text(name)
        result = self.script("prepare", "main..main", "--round", "2")
        self.assertEqual(result.returncode, 0, result.stderr)
        brief = (self.run_dir / "brief-2.md").read_text()
        self.assertIn(f"--rulings {self.run_dir / 'rulings-1.md'}", brief)
        self.assertIn(f"--prior {self.run_dir / 'sol.md'}", brief)
        self.assertIn(f"--prior {self.run_dir / 'opus.md'}", brief)
        self.assertIn(f"--focus {self.run_dir / 'focus-2.md'}", brief)

    def test_a_later_round_without_rulings_should_fail(self) -> None:
        result = self.script("prepare", "main..main", "--round", "2")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("rulings-1.md", result.stderr)
        self.assertFalse((self.run_dir / "brief-2.md").exists())

    def test_a_spec_should_not_build(self) -> None:
        spec = self.root / "spec.md"
        spec.write_text("spec")
        result = self.script("prepare", str(spec), "--kind", "spec")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.run_dir / "build-args").exists())

    def test_an_option_before_the_target_should_fail(self) -> None:
        result = self.script("prepare", "--doc", "spec.md", "main..main")
        self.assertEqual(result.returncode, 2)
        self.assertIn("TARGET", result.stderr)
        self.assertFalse((self.run_dir / "brief.md").exists())

    def test_verify_without_a_record_should_say_to_prepare(self) -> None:
        result = self.script("verify", "--report", "opus")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("run prepare first", result.stderr)

    def test_verify_should_pass_on_an_unchanged_checkout_with_each_report(self) -> None:
        self.script("prepare", "main..main")
        (self.run_dir / "opus.md").write_text("report")
        (self.run_dir / "sol.md").write_text("report")
        result = self.script("verify", "--report", "opus", "--report", "sol")
        self.assertEqual(result.returncode, 0, result.stdout)

    def test_verify_should_fail_when_a_reviewer_wrote_in_the_checkout(self) -> None:
        self.script("prepare", "main..main")
        (self.run_dir / "opus.md").write_text("report")
        (self.repo / "stray").write_text("x")
        result = self.script("verify", "--report", "opus")
        self.assertEqual(result.returncode, 1)
        self.assertIn("changed the checkout", result.stdout)

    def test_verify_should_fail_and_name_a_missing_report(self) -> None:
        self.script("prepare", "main..main")
        (self.run_dir / "sol.md").write_text("report")
        result = self.script("verify", "--report", "sol", "--report", "opus")
        self.assertEqual(result.returncode, 1)
        self.assertIn(f"no report: {self.run_dir / 'opus.md'}", result.stdout)
        self.assertNotIn("sol.md", result.stdout)

    def test_verify_of_a_later_round_should_ask_for_the_numbered_report(self) -> None:
        for name in ("rulings-1.md", "opus.md"):
            (self.run_dir / name).write_text(name)
        self.script("prepare", "main..main", "--round", "2")
        result = self.script("verify", "--round", "2", "--report", "opus")
        self.assertEqual(result.returncode, 1)
        self.assertIn("opus-2.md", result.stdout)

    def test_verify_with_no_report_named_should_fail(self) -> None:
        self.script("prepare", "main..main")
        result = self.script("verify")
        self.assertEqual(result.returncode, 2)
        self.assertIn("--report", result.stderr)


if __name__ == "__main__":
    unittest.main()
