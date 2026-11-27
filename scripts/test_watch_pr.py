"""Unit tests for watch_pr.py: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script with a fake `gh` on PATH that answers from a table.
"""

import base64
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

SCRIPT = Path(__file__).resolve().parent / "watch_pr.py"
FAKE_GH = f"""#!{sys.executable}
import base64, json, os, sys, time
table = json.load(open(os.environ['GH_TABLE']))
key = sys.argv[-1].split('?')[0]
if key not in table or table[key] is None:
    # Real `gh api` writes the JSON error body to stdout before exiting 1.
    print('{{"message":"Not Found"}}')
    sys.exit(1)
answer = table[key]
if isinstance(answer, dict) and '__sleep__' in answer:
    time.sleep(answer['__sleep__'])
    sys.exit(0)
if isinstance(answer, dict) and '__b64__' in answer and '--allow-escape-sequences' in sys.argv:
    sys.stdout.buffer.write(base64.b64decode(answer['__b64__']))
    sys.exit(0)
if isinstance(answer, str) and '--allow-escape-sequences' in sys.argv:
    sys.stdout.write(answer)
    sys.exit(0)
# --paginate --slurp: a list of pages. A list over 2 items comes as two pages.
if '--slurp' in sys.argv:
    answer = [answer[:2], answer[2:]] if isinstance(answer, list) and len(answer) > 2 else [answer]
print(json.dumps(answer))
"""
P = "repos/sisu-lang/sisu"


def comment(comment_id: int, login: str, body: str) -> dict[str, Any]:
    return {
        "id": comment_id,
        "user": {"login": login},
        "body": body,
        "path": "a.rs",
        "line": 3,
        "html_url": f"u{comment_id}",
    }


class WatchPr(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.dir)
        gh = self.dir / "gh"
        gh.write_text(FAKE_GH)
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
        self.state = self.dir / "state.json"
        self.table: dict[str, Any] = {
            "user": {"login": "me"},
            f"{P}/pulls/7": {"head": {"sha": "abcdef0123"}},
            f"{P}/pulls/7/reviews": [],
            f"{P}/pulls/7/comments": [],
            f"{P}/issues/7/comments": [],
            f"{P}/commits/abcdef0123/check-runs": {"check_runs": [{"status": "in_progress"}]},
            f"{P}/commits/abcdef0123/check-suites": {
                "check_suites": [
                    {"app": {"slug": "github-actions"}, "status": "completed"},
                    {"app": {"slug": "greptile-apps"}, "status": "queued"},
                ]
            },
        }

    def watch(self, **extra_env: str) -> subprocess.CompletedProcess[str]:
        (self.dir / "table.json").write_text(json.dumps(self.table))
        env = dict(
            os.environ,
            PATH=f"{self.dir}:{os.environ['PATH']}",
            GH_TABLE=str(self.dir / "table.json"),
            WATCH_PR_POLL="0",
            WATCH_PR_LIMIT="0",
            **extra_env,
        )
        return subprocess.run(
            [sys.executable, SCRIPT, "7", self.state],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_watch_should_print_a_new_comment_once_and_skip_my_own(self) -> None:
        self.table[f"{P}/pulls/7/comments"] = [
            comment(1, "bot", "<b>Fix</b> the line"),
            comment(2, "me", "done"),
        ]
        out = self.watch()
        self.assertEqual(
            (out.returncode, out.stdout), (0, "inline 1 bot a.rs:3: Fix the line u1\n")
        )
        self.assertEqual(self.watch().returncode, 1)

    def test_watch_should_read_every_page(self) -> None:
        self.table[f"{P}/issues/7/comments"] = [comment(i, "bot", f"c{i}") for i in range(3)]
        self.assertEqual(self.watch().stdout.count("comment 2 bot: c2"), 1)

    def test_watch_should_skip_the_in_progress_note_and_wake_on_its_edit(self) -> None:
        note = "<!-- This is an auto-generated comment: review in progress by coderabbit.ai -->"
        self.table[f"{P}/issues/7/comments"] = [comment(5, "coderabbitai[bot]", note)]
        self.assertEqual(self.watch().returncode, 1)
        self.table[f"{P}/issues/7/comments"] = [comment(5, "coderabbitai[bot]", "Summary")]
        self.assertEqual(self.watch().stdout, "comment 5 coderabbitai[bot]: Summary u5\n")

    def test_watch_should_skip_a_bot_comment_that_asks_for_no_action(self) -> None:
        cov = "<!-- sisu-coverage -->\nCoverage: lines 99.0%, regions 98.9%, changed lines"
        for login, body, wakes in (
            ("coderabbitai[bot]", "<a><img></a> No actionable comments were generated.", False),
            ("coderabbitai[bot]", "Actionable comments posted: 2", True),
            ("coderabbitai[bot]", "Fix this. No actionable comments were generated.", True),
            ("github-actions[bot]", f"{cov} 100.0% (44 of 44).", False),
            ("github-actions[bot]", f"{cov}: none.", False),
            ("github-actions[bot]", f"{cov} 97.7% (43 of 44).", True),
            ("github-actions[bot]", f"{cov} 100.0% (1999 of 2000).", True),
            ("someone", "No actionable comments were generated.", True),
        ):
            with self.subTest(body=body):
                self.state.unlink(missing_ok=True)
                self.table[f"{P}/issues/7/comments"] = [comment(5, login, body)]
                self.assertEqual(self.watch().returncode, 0 if wakes else 1)

    def test_watch_should_judge_each_edit_of_a_bot_comment_by_its_new_text(self) -> None:
        path = f"{P}/issues/7/comments"
        self.table[path] = [
            comment(5, "coderabbitai[bot]", "No actionable comments were generated.")
        ]
        self.assertEqual(self.watch().returncode, 1)
        self.table[path] = [comment(5, "coderabbitai[bot]", "Actionable comments posted: 1")]
        self.assertEqual(self.watch().returncode, 0)
        self.table[path] = [
            comment(5, "coderabbitai[bot]", "No actionable comments were generated.")
        ]
        self.assertEqual(self.watch().returncode, 1)

    def test_watch_should_stay_asleep_on_an_edit_of_hidden_text_only(self) -> None:
        for path in (f"{P}/pulls/7/comments", f"{P}/issues/7/comments"):
            with self.subTest(path=path):
                self.state.unlink(missing_ok=True)
                self.table[path] = [comment(5, "bot", "Summary <!-- state a -->")]
                self.assertEqual(self.watch().returncode, 0)
                self.table[path] = [comment(5, "bot", "Summary <!-- state b -->")]
                self.assertEqual(self.watch().returncode, 1)
                self.table[path] = []

    def test_watch_should_wake_on_an_edit_past_the_printed_text(self) -> None:
        long = "a" * 400
        for path in (f"{P}/pulls/7/comments", f"{P}/issues/7/comments"):
            with self.subTest(path=path):
                self.state.unlink(missing_ok=True)
                self.table[path] = [comment(5, "bot", long)]
                self.assertEqual(self.watch().returncode, 0)
                self.table[path] = [comment(5, "bot", long + " more")]
                self.assertEqual(self.watch().returncode, 0)
                self.table[path] = []

    def test_watch_should_wake_on_an_edit_between_angle_brackets(self) -> None:
        for path in (f"{P}/pulls/7/comments", f"{P}/issues/7/comments"):
            with self.subTest(path=path):
                self.state.unlink(missing_ok=True)
                self.table[path] = [comment(5, "bot", "Use Vec<String> here")]
                self.assertEqual(self.watch().returncode, 0)
                self.table[path] = [comment(5, "bot", "Use Vec<u8> here")]
                self.assertEqual(self.watch().returncode, 0)
                self.table[path] = []

    def test_watch_should_judge_a_review_edit_by_the_same_rule_as_a_comment(self) -> None:
        review = {"id": 9, "user": {"login": "bot"}, "state": "COMMENTED", "html_url": "r9"}
        self.table[f"{P}/pulls/7/reviews"] = [dict(review, body="Vec<u8> <!-- state a -->")]
        self.assertEqual(self.watch().returncode, 0)
        self.table[f"{P}/pulls/7/reviews"] = [dict(review, body="Vec<u8> <!-- state b -->")]
        self.assertEqual(self.watch().returncode, 1)
        self.table[f"{P}/pulls/7/reviews"] = [dict(review, body="Vec<i8> <!-- state b -->")]
        self.assertEqual(self.watch().returncode, 0)

    def test_watch_should_wake_on_an_edited_review(self) -> None:
        review = {"id": 9, "user": {"login": "bot"}, "state": "COMMENTED", "html_url": "r9"}
        self.table[f"{P}/pulls/7/reviews"] = [dict(review, body="One")]
        self.assertEqual(self.watch().stdout, "review 9 COMMENTED bot: One r9\n")
        self.table[f"{P}/pulls/7/reviews"] = [dict(review, body="Two")]
        self.assertEqual(self.watch().stdout, "review 9 COMMENTED bot: Two r9\n")

    def test_watch_should_wait_for_a_workflow_that_has_not_started(self) -> None:
        self.table[f"{P}/commits/abcdef0123/check-runs"] = {
            "check_runs": [
                {"status": "completed", "conclusion": "skipped", "name": "plan", "html_url": "u"}
            ]
        }
        self.table[f"{P}/commits/abcdef0123/check-suites"]["check_suites"].append(
            {"app": {"slug": "github-actions"}, "status": "queued"}
        )
        self.assertEqual(self.watch().returncode, 1)

    def test_watch_should_report_a_failed_check_and_then_the_end_of_checks_once(self) -> None:
        runs = f"{P}/commits/abcdef0123/check-runs"
        self.table[runs] = {
            "check_runs": [
                {"status": "completed", "conclusion": "failure", "name": "test", "html_url": "u"},
                {"status": "in_progress"},
            ]
        }
        self.assertEqual(self.watch().stdout, "checks failed at abcdef01: test u\n")
        self.table[runs] = {
            "check_runs": [
                {"status": "completed", "conclusion": "success", "name": "test", "html_url": "u"}
            ]
        }
        self.assertEqual(self.watch().stdout, "checks passed at abcdef01: 1 checks\n")
        self.assertEqual(self.watch().returncode, 1)

    def test_watch_should_print_stripped_error_lines_of_a_failed_actions_job(self) -> None:
        runs = f"{P}/commits/abcdef0123/check-runs"
        url = "https://github.com/sisu-lang/sisu/actions/runs/99/job/42"
        self.table[runs] = {
            "check_runs": [
                {"status": "completed", "conclusion": "failure", "name": "Lint", "html_url": url}
            ]
        }
        long_err = "error[E0308]: " + ("x" * 160)
        cases: list[tuple[str, Any, str]] = [
            (
                "bom, ansi, generic exit ignored, cleanup dropped",
                (
                    "\ufeff2026-11-26T20:17:24Z \x1b[31mE: Failed to fetch mirror 403 Forbidden\x1b[0m\n"
                    "2026-11-26T20:17:32Z ##[error]Process completed with exit code 100.\n"
                    "2026-11-26T20:17:32Z Post job cleanup.\n"
                    "2026-11-26T20:17:32Z ##[error]cleanup noise\n"
                ),
                "E: Failed to fetch mirror 403 Forbidden",
            ),
            (
                "tail of 3 matching lines with 300-char cap and blank/bare timestamps skipped",
                (
                    "2026-11-26T20:17:20Z error: first ignored\n"
                    "2026-11-26T20:17:21Z\n"
                    "2026-11-26T20:17:22Z   \n"
                    "2026-11-26T20:17:23Z ERROR second\n"
                    "2026-11-26T20:17:24Z FAIL third\n"
                    f"2026-11-26T20:17:25Z {long_err}\n"
                ),
                f"ERROR second | FAIL third | {long_err}"[:300],
            ),
            (
                "clippy error: matches even before generic exit footer",
                (
                    "2026-11-26T20:17:24Z error: item foo is never used\n"
                    "2026-11-26T20:17:25Z   --> crates/sisuc/src/lib.rs:4:4\n"
                    "2026-11-26T20:17:26Z ##[error]Process completed with exit code 101.\n"
                ),
                "error: item foo is never used",
            ),
            (
                "fallback to cleaned tail when only generic exit matches",
                (
                    "2026-11-26T20:17:24Z plain step one\n"
                    "2026-11-26T20:17:25Z plain step two\n"
                    "2026-11-26T20:17:26Z\n"
                    "2026-11-26T20:17:27Z ##[error]Process completed with exit code 1.\n"
                    "2026-11-26T20:17:28Z Post job cleanup.\n"
                ),
                "plain step one | plain step two | ##[error]Process completed with exit code 1.",
            ),
            (
                "non-utf8 bytes replaced instead of crashing",
                {
                    "__b64__": base64.b64encode(
                        b"2026-11-26T20:17:31Z FAILED bad byte \xff here\n"
                    ).decode("ascii")
                },
                "FAILED bad byte \ufffd here",
            ),
        ]
        for label, payload, expected in cases:
            with self.subTest(label=label):
                self.state.unlink(missing_ok=True)
                self.table[f"{P}/actions/jobs/42/logs"] = payload
                out = self.watch()
                self.assertEqual(out.returncode, 0, out.stderr)
                self.assertEqual(
                    out.stdout,
                    f"checks failed at abcdef01: Lint {url}\n  Lint: {expected}\n",
                )

    def test_watch_should_still_report_a_failed_job_when_log_fetch_fails_or_times_out(self) -> None:
        runs = f"{P}/commits/abcdef0123/check-runs"
        url = "https://github.com/sisu-lang/sisu/actions/runs/99/job/43"
        self.table[runs] = {
            "check_runs": [
                {"status": "completed", "conclusion": "failure", "name": "Lint", "html_url": url}
            ]
        }
        failures: tuple[tuple[str, Any, dict[str, str]], ...] = (
            ("http error with json stdout", None, {}),
            ("timeout", {"__sleep__": 0.5}, {"WATCH_PR_LOG_TIMEOUT": "0.05"}),
        )
        for label, payload, extra_env in failures:
            with self.subTest(label=label):
                self.state.unlink(missing_ok=True)
                self.table[f"{P}/actions/jobs/43/logs"] = payload
                out = self.watch(**extra_env)
                self.assertEqual(out.returncode, 0, out.stderr)
                self.assertEqual(out.stdout, f"checks failed at abcdef01: Lint {url}\n")


if __name__ == "__main__":
    unittest.main()
