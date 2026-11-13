"""Unit tests for open_pr.py: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script with a fake `gh` on PATH that keeps its arguments and its stdin.
"""

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import override

SCRIPT = Path(__file__).resolve().parent / "open_pr.py"
# A pre-commit hook runs these tests with the GIT_ variables of the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
FAKE_GH = f"""#!{sys.executable}
import json, os, sys
with open(os.environ['GH_LOG'], 'w') as f:
    json.dump({{'args': sys.argv[1:], 'stdin': sys.stdin.read()}}, f)
if os.environ.get('GH_FAIL'):
    print('HTTP 422: a pull request already exists', file=sys.stderr)
    sys.exit(1)
print('https://github.com/sisu-lang/sisu/pull/7')
"""


class OpenPr(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.dir)
        bin_dir = self.dir / "bin"
        bin_dir.mkdir()
        fake = bin_dir / "gh"
        fake.write_text(FAKE_GH)
        fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
        self.repo = self.dir / "repo"
        subprocess.run(["git", "init", "-q", "-b", "feat/x", str(self.repo)], check=True, env=ENV)
        self.body = self.dir / "body.md"
        self.body.write_text("Use `struct` and $HOME as they are.\n")
        self.log = self.dir / "log"
        self.env = dict(ENV, PATH=f"{bin_dir}:{ENV['PATH']}", GH_LOG=str(self.log))

    def open_pr(self, *extra: str, **env: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["python3", SCRIPT, "--title", "Add `x`", "--body-file", str(self.body), *extra],
            cwd=self.repo,
            capture_output=True,
            text=True,
            check=False,
            env=dict(self.env, **env),
        )

    def test_a_body_with_backticks_should_reach_the_request_as_it_is(self) -> None:
        result = self.open_pr()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.strip(), "https://github.com/sisu-lang/sisu/pull/7")
        call = json.loads(self.log.read_text())
        self.assertEqual(call["args"][:3], ["api", "repos/sisu-lang/sisu/pulls", "--input"])
        self.assertEqual(
            json.loads(call["stdin"]),
            {
                "title": "Add `x`",
                "head": "feat/x",
                "base": "main",
                "body": "Use `struct` and $HOME as they are.\n",
            },
        )

    def test_a_head_option_should_replace_the_checked_out_branch(self) -> None:
        self.open_pr("--head", "fix/y")
        self.assertEqual(json.loads(json.loads(self.log.read_text())["stdin"])["head"], "fix/y")

    def test_a_refused_request_should_fail_with_the_reason_of_gh(self) -> None:
        result = self.open_pr(GH_FAIL="1")
        self.assertEqual(result.returncode, 1)
        self.assertIn("open_pr: FAIL: HTTP 422: a pull request already exists", result.stdout)

    def test_a_missing_body_file_should_fail_with_its_reason(self) -> None:
        self.body.unlink()
        result = self.open_pr()
        self.assertEqual(result.returncode, 1)
        self.assertIn("open_pr: FAIL:", result.stdout)
        self.assertIn("body.md", result.stdout)
        self.assertFalse(self.log.exists())

    def test_a_detached_head_should_fail_and_ask_for_a_branch(self) -> None:
        git = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run([*git, "commit", "-q", "--allow-empty", "-m", "one"], check=True, env=ENV)
        subprocess.run([*git, "switch", "-q", "--detach"], check=True, env=ENV)
        result = self.open_pr()
        self.assertEqual(result.returncode, 1)
        self.assertIn("open_pr: FAIL: no branch is checked out", result.stdout)
        self.assertFalse(self.log.exists())


if __name__ == "__main__":
    unittest.main()
