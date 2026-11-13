"""Unit tests for pr_threads.py: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script with a fake `gh` on PATH that answers from a table.
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
from typing import Any, override

SCRIPT = Path(__file__).resolve().parent / "pr_threads.py"
FAKE_GH = f"""#!{sys.executable}
import json, os, sys
table = json.load(open(os.environ['GH_TABLE']))
answer = table[sys.argv[-1].split('?')[0]]
# --paginate --slurp: a list of pages. A list over 2 items comes as two pages.
if '--slurp' in sys.argv:
    answer = [answer[:2], answer[2:]] if isinstance(answer, list) and len(answer) > 2 else [answer]
print(json.dumps(answer))
"""
P = "repos/sisu-lang/sisu"
BOT = "coderabbitai[bot]"
# The head of a real CodeRabbit finding, read from #694.
SEVERITY = (
    "<!-- coderabbit-ai-generated:v1 -->\n\n"
    '**<picture><img src="amber-16.svg" alt=""></picture> Minor** · '
    "**Functional Correctness** · **Quick win**\n\n"
)


def inline(comment_id: int, login: str, body: str, reply_to: int | None = None) -> dict[str, Any]:
    c: dict[str, Any] = {
        "id": comment_id,
        "user": {"login": login},
        "body": body,
        "path": "a.py",
        "line": 3,
        "html_url": f"u{comment_id}",
    }
    if reply_to is not None:
        c["in_reply_to_id"] = reply_to
    return c


class PrThreads(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.dir)
        gh = self.dir / "gh"
        gh.write_text(FAKE_GH)
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
        self.table: dict[str, Any] = {
            "user": {"login": "me"},
            f"{P}/pulls/7/comments": [],
            f"{P}/pulls/7/reviews": [],
        }

    def run_script(self) -> subprocess.CompletedProcess[str]:
        (self.dir / "table.json").write_text(json.dumps(self.table))
        env = dict(
            os.environ,
            PATH=f"{self.dir}:{os.environ['PATH']}",
            GH_TABLE=str(self.dir / "table.json"),
        )
        return subprocess.run(
            [sys.executable, SCRIPT, "7"], env=env, capture_output=True, text=True, check=False
        )

    def test_threads_should_list_open_ones_first_with_the_bold_title(self) -> None:
        self.table[f"{P}/pulls/7/comments"] = [
            inline(1, BOT, SEVERITY + "**Fix the walk.**\n\nLong text"),
            inline(2, "me", "Fixed in abc.", reply_to=1),
            inline(3, BOT, "<b>Plain</b> finding with no title"),
        ]
        out = self.run_script()
        self.assertEqual(out.returncode, 0)
        self.assertEqual(
            out.stdout.splitlines(),
            [
                f"open 3 {BOT} a.py:3: Plain finding with no title u3",
                f"replied 1 {BOT} a.py:3: Minor: Fix the walk. u1",
            ],
        )

    def test_threads_should_skip_a_thread_that_i_started(self) -> None:
        self.table[f"{P}/pulls/7/comments"] = [
            inline(1, "me", "**Note** to self"),
            inline(2, BOT, "**Answer**", reply_to=1),
        ]
        self.assertEqual(self.run_script().stdout, "")

    def test_threads_should_count_a_bot_answer_as_no_reply_of_mine(self) -> None:
        self.table[f"{P}/pulls/7/comments"] = [
            inline(1, BOT, "**First**"),
            inline(2, BOT, "**More**", reply_to=1),
            inline(3, "someone", "agree", reply_to=1),
        ]
        self.assertEqual(self.run_script().stdout, f"open 1 {BOT} a.py:3: First u1\n")

    def test_threads_should_name_the_reviews_with_findings_outside_the_diff(self) -> None:
        self.table[f"{P}/pulls/7/reviews"] = [
            {
                "id": 9,
                "user": {"login": BOT},
                "body": "<summary>⚠️ Outside diff range comments (2)</summary> ...",
                "html_url": "r9",
            },
            {"id": 10, "user": {"login": BOT}, "body": "Actionable comments: 1", "html_url": "r10"},
        ]
        self.assertEqual(self.run_script().stdout, f"outside 9 {BOT}: 2 findings r9\n")

    def test_threads_should_print_usage_without_a_pull_request(self) -> None:
        out = subprocess.run([sys.executable, SCRIPT], capture_output=True, text=True, check=False)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("Usage", out.stderr)


if __name__ == "__main__":
    unittest.main()
