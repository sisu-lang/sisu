"""Unit tests for review-sisu's review_round.py.

Each test runs the script on a small repository whose origin/main holds fake build and brief
scripts. The fakes write their arguments to files in the run directory.
"""

import json
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

    def commit(self, message: str) -> None:
        (self.repo / message).write_text(message)
        self.git("add", ".")
        self.git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", message)

    def moved_main(self) -> None:
        """Make branch feat from the first commit, then move main past it."""
        self.git("switch", "-q", "-c", "feat")
        self.commit("feature")
        self.git("switch", "-q", "main")
        self.commit("later")
        self.git("switch", "-q", "feat")

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

    def test_a_diff_from_main_should_stop_when_main_moved(self) -> None:
        self.moved_main()
        result = self.script("prepare", "main~1..feat")
        self.assertEqual(result.returncode, 1)
        self.assertIn("main moved", result.stderr)
        self.assertFalse((self.run_dir / "build-args").exists())

    def test_old_main_should_let_the_diff_of_one_task_through(self) -> None:
        self.moved_main()
        result = self.script("prepare", "main~1..feat", "--old-main")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("--old-main", (self.run_dir / "brief.md").read_text())

    def test_a_diff_that_starts_on_the_branch_should_not_check_main(self) -> None:
        self.moved_main()
        self.commit("second")
        result = self.script("prepare", "feat~1..feat")
        self.assertEqual(result.returncode, 0, result.stderr)

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

    def test_verify_should_name_the_commits_when_only_head_moved(self) -> None:
        self.script("prepare", "main..main")
        (self.run_dir / "opus.md").write_text("report")
        self.commit("a fix")
        result = self.script("verify", "--report", "opus")
        self.assertEqual(result.returncode, 1)
        self.assertIn("only HEAD moved", result.stdout)
        self.assertIn("t a fix", result.stdout)
        self.assertNotIn("a reviewer changed", result.stdout)

    def test_verify_should_fail_on_an_empty_report(self) -> None:
        self.script("prepare", "main..main")
        (self.run_dir / "opus.md").write_text("")
        result = self.script("verify", "--report", "opus")
        self.assertEqual(result.returncode, 1)
        self.assertIn(f"no report: {self.run_dir / 'opus.md'}", result.stdout)

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


class SaveReport(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)
        self.agents = self.root / "projects/p/session/subagents"
        self.agents.mkdir(parents=True)

    def transcript(self, *blocks: object) -> None:
        lines = [json.dumps({"type": "assistant", "message": {"content": [b]}}) for b in blocks]
        lines.append(json.dumps({"type": "user", "message": {"content": [{"type": "text"}]}}))
        (self.agents / "agent-abc.jsonl").write_text("\n".join(lines) + "\n")

    def save(self, *args: str) -> subprocess.CompletedProcess[str]:
        transcripts = ["--transcripts", str(self.root / "projects")]
        return subprocess.run(
            ["python3", SCRIPT, "save-report", self.root, "opus", *args, *transcripts],
            capture_output=True,
            text=True,
            check=False,
            env=ENV,
        )

    def test_save_report_should_copy_the_handback_word_for_word(self) -> None:
        handback = {
            "type": "tool_use",
            "name": "SubagentHandback",
            "input": {"message": "Verdict: x"},
        }
        self.transcript({"type": "text", "text": "thinking aloud"}, handback)
        result = self.save("--agent", "abc", "--round", "2")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual((self.root / "opus-2.md").read_text(), "Verdict: x\n")

    def test_save_report_should_take_the_last_text_when_there_is_no_handback(self) -> None:
        self.transcript({"type": "text", "text": "first"}, {"type": "text", "text": "Verdict: y"})
        result = self.save("--agent", "agent-abc")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual((self.root / "opus.md").read_text(), "Verdict: y\n")

    def test_save_report_should_fail_when_no_transcript_has_the_id(self) -> None:
        result = self.save("--agent", "nope")
        self.assertEqual(result.returncode, 1)
        self.assertIn("0 transcripts", result.stdout)
        self.assertFalse((self.root / "opus.md").exists())


if __name__ == "__main__":
    unittest.main()
