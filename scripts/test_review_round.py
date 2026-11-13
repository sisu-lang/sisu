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
TEXT_LIMITS = Path(__file__).resolve().parent / "text_limits.py"
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
FAKE_BUILD = 'echo "$@" > "$2/build-args"\n'
REPORT = "Model: m\nVerdict: approve\n"
LONG = " ".join(["a"] * 26) + "."
ROW = "| `Body` holds, in {} fields, all the state. | `check::Body` | Closure checking |\n"
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
        (self.repo / "scripts").mkdir()
        shutil.copy(TEXT_LIMITS, self.repo / "scripts")
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

    def register(self, fields: int) -> None:
        """Commit a register whose `check::Body` row has FIELDS fields."""
        (self.repo / "docs").mkdir(exist_ok=True)
        (self.repo / "docs/register.md").write_text(ROW.format(fields))
        self.git("add", ".")
        self.git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "register")

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

    def test_a_later_round_should_take_the_report_of_every_reviewer_and_no_other_file(
        self,
    ) -> None:
        names = ("rulings-1.md", "fable.md", "sol.md", "opus.md", "brief.md", "focus.md")
        for name in (*names, "target-1.md", "diagnostic-issues.md", "focus-2.md", "retro.md"):
            (self.run_dir / name).write_text(name)
        result = self.script("prepare", "main..main", "--round", "2")
        self.assertEqual(result.returncode, 0, result.stderr)
        words = (self.run_dir / "brief-2.md").read_text().split()
        prior = [words[i + 1] for i, word in enumerate(words) if word == "--prior"]
        reports = [str(self.run_dir / name) for name in ("fable.md", "opus.md", "sol.md")]
        self.assertEqual(prior, reports)

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

    def test_a_head_that_names_no_commit_should_fail_with_the_error_of_git(self) -> None:
        result = self.script("prepare", "main..nope")
        self.assertEqual(result.returncode, 1)
        self.assertIn("nope", result.stderr)
        self.assertNotIn("main moved", result.stderr)

    def test_a_pr_body_should_go_to_the_brief_as_a_doc(self) -> None:
        body = self.root / "body.md"
        body.write_text("Title: Fix it\n\n## Summary\n\nA short sentence.\n")
        result = self.script("prepare", "main..main", "--pr-body", str(body))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"--doc {body}", (self.run_dir / "brief.md").read_text())

    def test_a_pr_body_over_a_text_limit_should_stop_before_the_build(self) -> None:
        body = self.root / "body.md"
        body.write_text(f"Short.\n\n{LONG}\n")
        result = self.script("prepare", "main..main", "--pr-body", str(body))
        self.assertEqual(result.returncode, 1)
        self.assertIn(f"{body}:3: a sentence has 26 words", result.stderr)
        self.assertFalse((self.run_dir / "build-args").exists())

    def test_a_pr_body_that_is_not_markdown_should_stop(self) -> None:
        body = self.root / "body.txt"
        body.write_text(f"{LONG}\n")
        result = self.script("prepare", "main..main", "--pr-body", str(body))
        self.assertEqual(result.returncode, 1)
        self.assertIn(".md file", result.stderr)
        self.assertFalse((self.run_dir / "build-args").exists())

    def test_a_grown_field_count_should_stop_until_the_focus_or_the_body_names_the_row(
        self,
    ) -> None:
        self.register(54)
        self.register(55)
        result = self.script("prepare", "main~1..main")
        self.assertEqual(result.returncode, 1)
        self.assertIn("`check::Body` row of docs/register.md from 54 to 55", result.stderr)
        self.assertFalse((self.run_dir / "build-args").exists())
        body = self.root / "body.md"
        body.write_text("It grows the `check::Body` row from 54 to 55 fields.\n")
        result = self.script("prepare", "main~1..main", "--pr-body", str(body))
        self.assertEqual(result.returncode, 0, result.stderr)
        body.unlink()
        (self.run_dir / "focus.md").write_text("The diff grows the `check::Body` row.\n")
        result = self.script("prepare", "main~1..main")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_smaller_or_a_new_field_count_should_not_stop(self) -> None:
        self.register(55)
        self.register(54)
        result = self.script("prepare", "main~2..main~1", "--old-main")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.script("prepare", "main~1..main").returncode, 0)

    def test_verify_without_a_record_should_say_to_prepare(self) -> None:
        result = self.script("verify", "--report", "opus")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("run prepare first", result.stderr)

    def test_record_should_let_verify_check_a_checkout_without_prepare(self) -> None:
        self.script("record")
        (self.run_dir / "cross-check-report.md").write_text(REPORT)
        result = self.script("verify", "--report", "cross-check-report")
        self.assertEqual(result.returncode, 0, result.stdout)
        (self.repo / "new").write_text("x")
        result = self.script("verify", "--report", "cross-check-report")
        self.assertEqual(result.returncode, 1, result.stdout)

    def test_verify_should_pass_on_an_unchanged_checkout_with_each_report(self) -> None:
        self.script("prepare", "main..main")
        (self.run_dir / "opus.md").write_text(REPORT)
        (self.run_dir / "sol.md").write_text(REPORT)
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
        (self.run_dir / "sol.md").write_text(REPORT)
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

    def test_verify_should_stop_on_the_path_of_another_round(self) -> None:
        self.script("prepare", "main..main")
        (self.run_dir / "opus.md").write_text(REPORT)
        (self.run_dir / "opus-2.md").write_text(REPORT)
        result = self.script("verify", "--report", str(self.run_dir / "opus-2.md"))
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("is not the report of round 1", result.stderr)

    def test_verify_should_fail_on_a_report_without_a_model_line(self) -> None:
        self.script("prepare", "main..main")
        (self.run_dir / "opus.md").write_text("Verdict: approve\n")
        result = self.script("verify", "--report", "opus")
        self.assertEqual(result.returncode, 1)
        self.assertIn(f"no Model line: {self.run_dir / 'opus.md'}", result.stdout)

    def test_verify_with_no_report_named_should_fail(self) -> None:
        self.script("prepare", "main..main")
        result = self.script("verify")
        self.assertEqual(result.returncode, 2)
        self.assertIn("--report", result.stderr)


class Rulings(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.run_dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.run_dir)
        finding = "### {n}. A title\nSeverity: nit\n"
        (self.run_dir / "sol.md").write_text(REPORT + finding.format(n=1) + finding.format(n=2))
        (self.run_dir / "opus.md").write_text(REPORT + finding.format(n=1))

    def rulings(self, lines: str) -> subprocess.CompletedProcess[str]:
        (self.run_dir / "rulings-1.md").write_text(lines)
        return subprocess.run(
            ["python3", SCRIPT, "rulings", self.run_dir, "--report", "sol", "--report", "opus"],
            capture_output=True,
            text=True,
            check=False,
            env=ENV,
        )

    def test_rulings_should_take_the_path_of_a_report_as_its_stem(self) -> None:
        (self.run_dir / "rulings-2.md").write_text("sol 1: Accept: x.\nsol 2: Reject: y.\n")
        (self.run_dir / "sol-2.md").write_bytes((self.run_dir / "sol.md").read_bytes())
        result = subprocess.run(
            [
                "python3",
                SCRIPT,
                "rulings",
                self.run_dir,
                "--round",
                "2",
                "--report",
                str(self.run_dir / "sol-2.md"),
            ],
            capture_output=True,
            text=True,
            check=False,
            env=ENV,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("sol: 2 findings: 1 Accept, 1 Reject", result.stdout)

    def test_rulings_should_stop_on_the_path_of_another_round(self) -> None:
        result = subprocess.run(
            [
                "python3",
                SCRIPT,
                "rulings",
                self.run_dir,
                "--report",
                str(self.run_dir / "sol-2.md"),
            ],
            capture_output=True,
            text=True,
            check=False,
            env=ENV,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("is not the report of round 1", result.stderr)

    def test_rulings_should_stop_on_a_report_path_outside_the_run(self) -> None:
        result = subprocess.run(
            ["python3", SCRIPT, "rulings", self.run_dir, "--report", "/elsewhere/sol.md"],
            capture_output=True,
            text=True,
            check=False,
            env=ENV,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("is not the report of round 1", result.stderr)

    def test_rulings_should_count_each_ruling_of_each_reviewer(self) -> None:
        result = self.rulings(
            "sol 1: Accept: same as opus 1.\nsol 2: Defer to #12: later.\nopus 1: Accept: x.\n"
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("sol: 2 findings: 1 Accept, 1 Defer", result.stdout)
        self.assertIn("opus: 1 findings: 1 Accept", result.stdout)

    def test_rulings_should_fail_and_name_a_finding_without_a_ruling(self) -> None:
        result = self.rulings("sol 1: Reject: the code at a.rs:3 does it.\nopus 1: Accept: x.\n")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no ruling: sol 2", result.stdout)
        self.assertNotIn("no ruling: sol 1", result.stdout)

    def test_rulings_should_fail_on_a_word_other_than_accept_reject_or_defer(self) -> None:
        result = self.rulings("sol 1: Accept: x.\nsol 2: Pending: later.\nopus 1: Accept: x.\n")
        self.assertEqual(result.returncode, 1)
        self.assertIn("not Accept, Reject or Defer: sol 2: Pending", result.stdout)

    def test_rulings_should_fail_on_two_words_that_only_start_with_a_ruling(self) -> None:
        result = self.rulings(
            "sol 1: Accept: x.\nsol 2: Accept/Reject: later.\nopus 1: Accept: x.\n"
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("not Accept, Reject or Defer: sol 2: Accept/Reject", result.stdout)

    def test_rulings_should_fail_on_a_deferral_with_no_destination(self) -> None:
        result = self.rulings("sol 1: Accept: x.\nsol 2: Defer to : later.\nopus 1: Accept: x.\n")
        self.assertEqual(result.returncode, 1)
        self.assertIn("not Accept, Reject or Defer: sol 2: Defer to ", result.stdout)

    def test_rulings_without_a_rulings_file_should_name_each_finding_without_a_ruling(self) -> None:
        result = subprocess.run(
            ["python3", SCRIPT, "rulings", self.run_dir, "--report", "opus"],
            capture_output=True,
            text=True,
            check=False,
            env=ENV,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("no ruling: opus 1", result.stdout)
        self.assertNotIn("Traceback", result.stderr)

    def test_rulings_should_read_the_report_and_the_rulings_of_the_given_round(self) -> None:
        (self.run_dir / "opus-2.md").write_text(REPORT + "### 1. A\n### 2. B\n")
        (self.run_dir / "rulings-2.md").write_text("opus 1: Accept: x.\nopus 2: Reject: y.\n")
        result = subprocess.run(
            ["python3", SCRIPT, "rulings", self.run_dir, "--report", "opus", "--round", "2"],
            capture_output=True,
            text=True,
            check=False,
            env=ENV,
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("opus: 2 findings: 1 Accept, 1 Reject", result.stdout)


class Stats(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)

    def run_dir(self, name: str, files: dict[str, str]) -> Path:
        path = self.root / name
        path.mkdir()
        for file, text in files.items():
            (path / file).write_text(text)
        return path

    def stats(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["python3", SCRIPT, "stats", *sorted(self.root.iterdir())],
            capture_output=True,
            text=True,
            check=False,
            env=ENV,
        )

    def two_rounds(self, files: dict[str, str]) -> None:
        """Make a run of two rounds, with one blocking finding and one nit in round 1."""
        report = REPORT + "### 1. A\nSeverity: blocking\n### 2. B\nSeverity: nit\n"
        rulings = "sol 1: Accept (same as opus 1): x.\nsol 2: Defer to #12: later.\n"
        later = {"brief.md": "", "brief-2.md": "", "sol.md": report, "rulings-1.md": rulings}
        self.run_dir("two-rounds", later | files)

    def test_stats_should_count_rounds_and_rulings_by_round_and_severity(self) -> None:
        self.two_rounds({"rulings-final.md": "sol 1: Reject: not a round.\n"})
        self.run_dir("one-round", {"brief.md": ""})
        self.run_dir("not-a-review", {"notes.md": ""})
        result = self.stats()
        self.assertEqual(result.returncode, 0, result.stderr)
        for line in (
            "runs: 2; last brief: 1: 1, 2: 1",
            "runs with no rulings file: 1",
            "round 1, blocking: 1: Accept: 1",
            "round 1, nit: 1: Defer: 1",
            "reviewer sol: 2: Accept: 1, Defer: 1",
        ):
            self.assertIn(line, result.stdout.splitlines())

    def test_stats_should_count_the_origin_of_each_later_blocking_finding(self) -> None:
        finding = "### {n}. C\nSeverity: blocking\n{origin}"
        origins = ("Origin: fix\n", "Origin: `missed`\n", "Origin: unfixed\n", "")
        report = REPORT + "".join(finding.format(n=n, origin=o) for n, o in enumerate(origins))
        rulings = "".join(f"opus {n}: Accept: x.\n" for n in range(len(origins)))
        self.two_rounds({"opus-2.md": report, "rulings-2.md": rulings})
        result = self.stats()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("later rounds, blocking: 4: Accept: 4", result.stdout.splitlines())
        line = "origin of blocking findings after round 1: fix: 1, missed: 1, none: 1, unfixed: 1"
        self.assertIn(line, result.stdout.splitlines())

    def test_stats_should_count_retro_classes_without_case(self) -> None:
        retro = "sol 1: Incomplete fix: a neighbour row.\nopus-2 1: incomplete fix: x.\nno class\n"
        self.two_rounds({"retro.md": retro})
        result = self.stats()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("retro classes: incomplete fix: 2", result.stdout.splitlines())

    def test_stats_should_stop_on_a_run_that_is_not_a_directory(self) -> None:
        result = subprocess.run(
            ["python3", SCRIPT, "stats", self.root / "nope"],
            capture_output=True,
            text=True,
            check=False,
            env=ENV,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("is not a directory", result.stderr)


class SaveReport(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)
        self.agents = self.root / "projects/p/session/subagents"
        self.agents.mkdir(parents=True)

    def transcript(self, *blocks: object) -> None:
        entries = [{"type": "assistant", "message": {"content": [b]}} for b in blocks]
        entries.append({"type": "user", "message": {"content": [{"type": "text"}]}})
        lines = [json.dumps(entry, ensure_ascii=False) for entry in entries]
        (self.agents / "agent-abc.jsonl").write_text("\n".join(lines) + "\n\n", encoding="utf-8")

    def save(self, *args: str, stem: str = "opus") -> subprocess.CompletedProcess[str]:
        transcripts = ["--transcripts", str(self.root / "projects")]
        return subprocess.run(
            ["python3", SCRIPT, "save-report", self.root, stem, *args, *transcripts],
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

    def test_save_report_should_take_the_path_of_the_report_as_its_stem(self) -> None:
        self.transcript({"type": "text", "text": "Verdict: x"})
        result = self.save("--agent", "abc", "--round", "2", stem=str(self.root / "opus-2.md"))
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual((self.root / "opus-2.md").read_text(), "Verdict: x\n")

    def test_save_report_should_stop_on_the_path_of_another_round(self) -> None:
        self.transcript({"type": "text", "text": "Verdict: x"})
        result = self.save("--agent", "abc", stem=str(self.root / "opus-2.md"))
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("is not the report of round 1", result.stderr)
        self.assertEqual(list(self.root.glob("*.md")), [])

    def test_save_report_should_keep_a_line_separator_in_the_reply(self) -> None:
        reply = "Verdict: x\u2028two\u0085three\n"
        handback = {"type": "tool_use", "name": "SubagentHandback", "input": {"message": reply}}
        self.transcript(handback)
        result = self.save("--agent", "abc")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "opus.md").read_text(encoding="utf-8"), reply)

    def test_save_report_should_fail_when_the_agent_stopped_during_its_work(self) -> None:
        self.transcript(
            {"type": "text", "text": "thinking aloud"}, {"type": "tool_use", "name": "Read"}
        )
        result = self.save("--agent", "abc")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no final reply", result.stdout)
        self.assertFalse((self.root / "opus.md").exists())

    def test_save_report_should_fail_on_a_line_that_is_not_json(self) -> None:
        self.transcript({"type": "text", "text": "Verdict: y"})
        with (self.agents / "agent-abc.jsonl").open("a", encoding="utf-8") as f:
            f.write('{"type": "assistant", "mess')
        result = self.save("--agent", "abc")
        self.assertEqual(result.returncode, 1)
        self.assertIn("not JSON", result.stderr)
        self.assertFalse((self.root / "opus.md").exists())

    def test_save_report_should_fail_on_a_blank_reply(self) -> None:
        self.transcript({"type": "tool_use", "name": "SubagentHandback", "input": {"message": " "}})
        result = self.save("--agent", "abc")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no final reply", result.stdout)

    def test_save_report_should_fail_when_two_transcripts_have_the_id(self) -> None:
        self.transcript({"type": "text", "text": "Verdict: y"})
        other = self.root / "projects/q/session/subagents"
        other.mkdir(parents=True)
        shutil.copy(self.agents / "agent-abc.jsonl", other)
        result = self.save("--agent", "abc")
        self.assertEqual(result.returncode, 1)
        self.assertIn("2 transcripts", result.stdout)

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
