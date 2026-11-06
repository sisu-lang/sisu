"""Unit tests for fuzz_issue.sh: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script with a fake `gh` on PATH, so nothing reaches GitHub.
"""

import json
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fuzz_issue.sh")
# The fake gh answers a list request with the JSON in $FAKE_ISSUES, and records the path of
# a request that sends a body, with that body, to `posts`.
FAKE_GH = """#!/usr/bin/env bash
if [[ $* == *--input* ]]; then
  echo "$2" >>posts
  cat >>posts
  echo >>posts
  exit 0
fi
echo "$*" >list
echo "$FAKE_ISSUES"
"""


class FuzzIssue(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.dir)
        bin_dir = os.path.join(self.dir, "bin")
        os.mkdir(bin_dir)
        gh = os.path.join(bin_dir, "gh")
        with open(gh, "w", encoding="utf-8") as f:
            f.write(FAKE_GH)
        os.chmod(gh, os.stat(gh).st_mode | stat.S_IXUSR)
        self.bin = bin_dir
        self.summary = os.path.join(self.dir, "summary.md")

    def run_script(self, issues, summary=b"# Fuzzing: front_end\n", target="front_end"):
        if summary is not None:
            with open(self.summary, "wb") as f:
                f.write(summary)
        env = dict(
            os.environ,
            PATH=f"{self.bin}:{os.environ['PATH']}",
            FAKE_ISSUES=json.dumps(issues),
            GITHUB_REPOSITORY="o/r",
            RUN_URL="https://example.test/run/1",
            GH_TOKEN="t",
        )
        run = subprocess.run(
            ["bash", SCRIPT, target, self.summary],
            cwd=self.dir,
            capture_output=True,
            env=env,
            timeout=30,
        )
        self.assertEqual(run.returncode, 0, run.stderr)
        posts = os.path.join(self.dir, "posts")
        if not os.path.exists(posts):
            return None, None
        with open(posts, encoding="utf-8") as f:
            path, _, body = f.read().partition("\n")
        return path, json.loads(body)

    def test_a_failure_with_no_open_issue_should_open_one_with_the_summary_and_the_run_url(
        self,
    ) -> None:
        path, body = self.run_script([])
        self.assertEqual(path, "repos/o/r/issues")
        self.assertEqual(body["title"], "Nightly fuzz failure: front_end")
        self.assertEqual(body["labels"], ["needs-triage", "bug", "fuzz"])
        self.assertIn("# Fuzzing: front_end", body["body"])
        self.assertIn("https://example.test/run/1", body["body"])
        self.assertIn("severity label", body["body"])
        with open(os.path.join(self.dir, "list"), encoding="utf-8") as f:
            self.assertIn("labels=fuzz", f.read())

    def test_a_failure_with_an_open_issue_should_comment_on_it(self) -> None:
        issues = [
            {"number": 5, "title": "Nightly fuzz failure: program"},
            {"number": 7, "title": "Nightly fuzz failure: front_end"},
        ]
        path, body = self.run_script(issues)
        self.assertEqual(path, "repos/o/r/issues/7/comments")
        self.assertEqual(list(body), ["body"])

    def test_a_pull_request_with_the_title_should_not_count_as_the_open_issue(self) -> None:
        issues = [{"number": 7, "title": "Nightly fuzz failure: front_end", "pull_request": {}}]
        path, _ = self.run_script(issues)
        self.assertEqual(path, "repos/o/r/issues")

    def test_a_summary_with_shell_and_json_characters_should_arrive_unchanged(self) -> None:
        text = "- **crash**: `$(touch pwned)` \"q\" \\ 's' ${HOME}\n"
        _, body = self.run_script([], summary=text.encode())
        self.assertIn(text, body["body"])
        self.assertFalse(os.path.exists(os.path.join(self.dir, "pwned")))

    def test_a_missing_summary_should_still_report_the_run(self) -> None:
        _, body = self.run_script([], summary=None)
        self.assertIn("wrote no summary", body["body"])
        self.assertIn("https://example.test/run/1", body["body"])

    def test_a_long_summary_should_be_cut_below_the_api_limit(self) -> None:
        _, body = self.run_script([], summary=b"x" * 70000)
        self.assertLess(len(body["body"]), 65536)


if __name__ == "__main__":
    unittest.main()
