"""Unit tests for review-sisu's diff_brief.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import override
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / ".agents/skills/review-sisu/scripts/diff_brief.py"
spec = importlib.util.spec_from_file_location("diff_brief", SCRIPT)
# pyrefly: ignore[bad-argument-type]  the script is in the repository, so the spec is found
diff_brief = importlib.util.module_from_spec(spec)
# pyrefly: ignore[missing-attribute]  as above, and a file spec has a loader
spec.loader.exec_module(diff_brief)
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}


class Repo(unittest.TestCase):
    """A repository of three commits, base, mid and head, and a side commit off base."""

    @override
    def setUp(self) -> None:
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        self.git = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run([*self.git[:3], "init", "-q"], check=True, env=ENV)
        for message in ("base", "mid", "head"):
            subprocess.run(
                [*self.git, "commit", "-q", "--allow-empty", "-m", message], check=True, env=ENV
            )
        self.head, self.mid, self.base = (self.sha(r) for r in ("HEAD", "HEAD~1", "HEAD~2"))
        self.side = subprocess.run(
            [*self.git, "commit-tree", "HEAD~2^{tree}", "-p", self.base, "-m", "side"],
            capture_output=True,
            text=True,
            check=True,
            env=ENV,
        ).stdout.strip()

    def sha(self, ref: str) -> str:
        return subprocess.run(
            [*self.git[:3], "rev-parse", ref], capture_output=True, text=True, check=True, env=ENV
        ).stdout.strip()

    def rebase(self, text: str) -> str:
        """A side commit off base that writes `f`, and its copy on HEAD, as a rebase leaves it."""
        tree = subprocess.run(
            [*self.git[:3], "mktree"],
            input=f"100644 blob {self.blob(text)}\tf\n",
            capture_output=True,
            text=True,
            check=True,
            env=ENV,
        ).stdout.strip()
        side = subprocess.run(
            [*self.git, "commit-tree", tree, "-p", self.base, "-m", "side"],
            capture_output=True,
            text=True,
            check=True,
            env=ENV,
        ).stdout.strip()
        subprocess.run([*self.git, "cherry-pick", side], capture_output=True, check=True, env=ENV)
        return side

    def blob(self, text: str) -> str:
        return subprocess.run(
            [*self.git[:3], "hash-object", "-w", "--stdin"],
            input=text,
            capture_output=True,
            text=True,
            check=True,
            env=ENV,
        ).stdout.strip()


class LogMismatch(Repo):
    """log_mismatch on logs of the range base..HEAD."""

    def mismatch(self, text: str) -> str | None:
        log = Path(self.repo) / "mutation.log"
        log.write_text(text)
        with mock.patch.dict(os.environ, ENV, clear=True):
            return diff_brief.log_mismatch(log, self.repo, self.base, "HEAD")

    def summary(self, sha: str) -> str:
        return f"mutation-test: PASS: 2 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {sha})\n"

    def test_a_summary_on_the_head_should_pass(self) -> None:
        self.assertIsNone(
            self.mismatch(
                f"mutants\nmutation-test: FAIL: 0 caught, 0 unviable, 2 missed, 0 timeouts (HEAD {self.head})\n"
            )
        )

    def test_a_summary_on_an_earlier_commit_of_the_range_should_pass(self) -> None:
        self.assertIsNone(self.mismatch(self.summary(self.mid)))

    def test_a_summary_before_a_merge_of_main_should_pass(self) -> None:
        main = subprocess.run(
            [*self.git, "commit-tree", "HEAD~2^{tree}", "-p", self.base, "-m", "main"],
            capture_output=True,
            text=True,
            check=True,
            env=ENV,
        ).stdout.strip()
        subprocess.run([*self.git, "merge", "-q", "--no-edit", main], check=True, env=ENV)
        self.base = main
        self.assertIsNone(self.mismatch(self.summary(self.mid)))

    def test_a_summary_on_the_base_should_fail(self) -> None:
        why = self.mismatch(self.summary(self.base))
        assert why is not None
        self.assertIn(f"measured {self.base[:8]}, which is not in", why)

    def test_a_summary_off_the_range_should_fail(self) -> None:
        why = self.mismatch(self.summary(self.side))
        assert why is not None
        self.assertIn(f"measured {self.side[:8]}, which is not in", why)

    def test_a_summary_on_uncommitted_changes_should_fail(self) -> None:
        why = self.mismatch(
            f"mutation-test: PASS: 2 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head} with uncommitted changes)\n"
        )
        assert why is not None
        self.assertIn("no summary line", why)

    def test_a_log_without_a_summary_should_fail(self) -> None:
        why = self.mismatch("coverage\nmutants still running\n")
        assert why is not None
        self.assertIn("no summary line", why)

    def test_a_summary_on_a_commit_that_a_rebase_copied_into_the_range_should_pass(self) -> None:
        self.assertIsNone(self.mismatch(self.summary(self.rebase("x\n"))))

    def test_a_summary_on_a_commit_whose_patch_differs_from_the_range_should_fail(self) -> None:
        side = self.rebase("x\n")
        (Path(self.repo) / "f").write_text("y\n")
        subprocess.run(
            [*self.git, "commit", "-q", "--amend", "-a", "-m", "changed"], check=True, env=ENV
        )
        why = self.mismatch(self.summary(side))
        assert why is not None
        self.assertIn(f"measured {side[:8]}, which is not in", why)


class CoverageMismatch(Repo):
    """coverage_mismatch on outputs of `make coverage` for the range base..HEAD."""

    def mismatch(self, text: str) -> str | None:
        path = Path(self.repo) / "coverage.txt"
        path.write_text(text)
        with mock.patch.dict(os.environ, ENV, clear=True):
            return diff_brief.coverage_mismatch(path, self.repo, "HEAD")

    def test_coverage_on_the_head_should_pass(self) -> None:
        self.assertIsNone(self.mismatch(f"coverage: measured (HEAD {self.head})\nCoverage: 90%\n"))

    def test_coverage_on_an_earlier_commit_of_the_range_should_fail(self) -> None:
        why = self.mismatch(f"coverage: measured (HEAD {self.mid})\nCoverage: 90%\n")
        assert why is not None
        self.assertIn(f"measured {self.mid[:8]}, not the head {self.head[:8]}", why)

    def test_coverage_on_uncommitted_changes_should_fail(self) -> None:
        why = self.mismatch(
            f"coverage: measured (HEAD {self.head} with uncommitted changes)\nCoverage: 90%\n"
        )
        assert why is not None
        self.assertIn("measured uncommitted changes", why)

    def test_coverage_without_a_coverage_line_should_fail(self) -> None:
        why = self.mismatch("Coverage: 90%\n")
        assert why is not None
        self.assertIn("no coverage line", why)


class Brief(Repo):
    """The brief that main() writes; the test repository has no origin/main, so the script's own template is used."""

    def brief(self, *args: str) -> str:
        self.run_dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.run_dir)
        out = self.run_dir / "brief.md"
        with (
            mock.patch.dict(os.environ, ENV, clear=True),
            mock.patch.object(
                sys, "argv", ["diff_brief.py", self.repo, *args[:1], str(out), *args[1:]]
            ),
            mock.patch("sys.stderr"),
            mock.patch("sys.stdout"),
        ):
            diff_brief.main()
        return out.read_text()

    def test_a_diff_brief_should_name_its_range_by_full_shas(self) -> None:
        text = self.brief("HEAD~2..HEAD")
        self.assertIn(f"Target: {self.base}..{self.head}.", text)
        self.assertNotIn("# Check, for a plan", text)
        self.assertNotIn("# Re-review", text)

    def test_a_mutation_log_on_an_earlier_commit_should_list_the_later_commits(self) -> None:
        log = Path(self.repo) / "mutation.log"
        log.write_text(
            f"mutation-test: PASS: 1 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.mid})\n"
        )
        text = self.brief("HEAD~2..HEAD", "--mutation-log", str(log))
        self.assertIn(
            f"measured at {self.mid[:8]}. Commits after it, not mutated: {self.head[:8]}", text
        )

    def test_a_mutation_log_before_a_rebase_should_name_the_rebased_commit(self) -> None:
        side = self.rebase("x\n")
        twin = self.sha("HEAD")
        log = Path(self.repo) / "mutation.log"
        log.write_text(
            f"mutation-test: PASS: 1 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {side})\n"
        )
        text = self.brief("HEAD~3..HEAD", "--mutation-log", str(log))
        self.assertIn(f"measured at {side[:8]}, which a rebase made {twin[:8]}\n", text)

    def test_each_of_two_mutation_logs_should_get_a_focus_line_with_its_own_note(self) -> None:
        first, second = Path(self.repo) / "mutation-1.log", Path(self.repo) / "mutation-2.log"
        first.write_text(
            f"mutation-test: PASS: 1 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.mid})\n"
        )
        second.write_text(
            f"mutation-test: PASS: 2 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head})\n"
        )
        text = self.brief(
            "HEAD~2..HEAD", "--mutation-log", str(first), "--mutation-log", str(second)
        )
        self.assertIn(
            f"Mutation-test log: {first}, measured at {self.mid[:8]}. Commits after it, not mutated: "
            f"{self.head[:8]}\n",
            text,
        )
        self.assertIn(f"Mutation-test log: {second}\n", text)

    def test_a_second_mutation_log_that_names_a_commit_outside_the_range_should_fail(self) -> None:
        good, bad = Path(self.repo) / "good.log", Path(self.repo) / "bad.log"
        good.write_text(
            f"mutation-test: PASS: 1 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head})\n"
        )
        bad.write_text(
            f"mutation-test: PASS: 1 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {'0' * 40})\n"
        )
        with self.assertRaises(SystemExit) as stop:
            self.brief("HEAD~2..HEAD", "--mutation-log", str(good), "--mutation-log", str(bad))
        self.assertIn(str(bad), str(stop.exception))

    def test_a_missing_second_mutation_log_should_fail_before_writing(self) -> None:
        good = Path(self.repo) / "good.log"
        good.write_text(
            f"mutation-test: PASS: 1 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head})\n"
        )
        with self.assertRaises(SystemExit):
            self.brief(
                "HEAD~2..HEAD", "--mutation-log", str(good), "--mutation-log", str(good) + ".none"
            )
        self.assertEqual(list(self.run_dir.iterdir()), [])

    def test_a_coverage_result_should_be_named_in_focus(self) -> None:
        coverage = Path(self.repo) / "coverage.txt"
        coverage.write_text(f"coverage: measured (HEAD {self.head})\nuncovered: none\n")
        self.assertIn(
            f"Coverage result: {coverage}", self.brief("HEAD~2..HEAD", "--coverage", str(coverage))
        )

    def test_a_missing_plan_should_fail_before_writing(self) -> None:
        with self.assertRaises(SystemExit):
            self.brief(str(Path(self.repo) / "no-plan.md"), "--kind", "plan")
        self.assertEqual(list(self.run_dir.iterdir()), [])

    def test_a_second_plan_round_should_name_the_previous_version(self) -> None:
        plan, prior = Path(self.repo) / "plan.md", Path(self.repo) / "sol.md"
        plan.write_text("# Plan v1\n")
        prior.write_text("Verdict: changes needed\n")
        run_dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, run_dir)
        args: list[str]
        for args in ([], ["--round", "2", "--prior", str(prior), "--rulings", str(prior)]):
            out = run_dir / ("brief-2.md" if args else "brief.md")
            with (
                mock.patch.dict(os.environ, ENV, clear=True),
                mock.patch.object(
                    sys,
                    "argv",
                    ["diff_brief.py", self.repo, str(plan), str(out), "--kind", "plan", *args],
                ),
                mock.patch("sys.stderr"),
                mock.patch("sys.stdout"),
            ):
                diff_brief.main()
            plan.write_text("# Plan v2\n")
        text = (run_dir / "brief-2.md").read_text()
        self.assertIn(f"- The previous version: {run_dir / 'target-1.md'}.", text)
        self.assertIn(f"The first brief, {run_dir / 'brief.md'}, names", " ".join(text.split()))
        self.assertEqual((run_dir / "target-2.md").read_text(), "# Plan v2\n")

    def test_a_second_plan_round_without_a_first_snapshot_should_drop_the_previous_version(
        self,
    ) -> None:
        plan = Path(self.repo) / "plan.md"
        plan.write_text("# Plan\n")
        text = self.brief(
            str(plan),
            "--kind",
            "plan",
            "--round",
            "2",
            "--prior",
            str(plan),
            "--rulings",
            str(plan),
        )
        self.assertNotIn("The previous version", text)

    def test_a_plan_brief_should_read_a_snapshot_and_drop_the_diff_lines(self) -> None:
        plan = Path(self.repo) / "plan.md"
        plan.write_text("# Plan\n")
        text = self.brief(str(plan), "--kind", "plan")
        snapshot = self.run_dir / "target-1.md"
        self.assertEqual(snapshot.read_text(), "# Plan\n")
        self.assertIn(f"Target: {snapshot}.", text)
        self.assertIn("# Check, for a plan", text)
        self.assertNotIn("# Check, for a diff", text)
        self.assertNotIn("For a diff,", text.split("# Values")[0])

    def test_a_second_round_should_keep_the_re_review_section(self) -> None:
        prior = Path(self.repo) / "sol.md"
        prior.write_text("Verdict: changes needed\n")
        text = self.brief(
            "HEAD~1..HEAD", "--round", "2", "--prior", str(prior), "--rulings", str(prior)
        )
        section = text.split("# Re-review", 1)[1].split("\n# ", 1)[0]
        self.assertIn("This is round 2.", section)
        self.assertNotIn("The previous version", section)

    def test_an_earlier_rounds_mutation_log_should_pass_in_a_later_round(self) -> None:
        prior = Path(self.repo) / "sol.md"
        prior.write_text("Verdict: changes needed\n")
        log = Path(self.repo) / "mutation.log"
        log.write_text(
            f"mutation-test: PASS: 1 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.base})\n"
        )
        text = self.brief(
            "HEAD~1..HEAD",
            "--round",
            "2",
            "--prior",
            str(prior),
            "--rulings",
            str(prior),
            "--mutation-log",
            str(log),
        )
        self.assertIn(
            f"Mutation-test log: {log}, measured at {self.base[:8]}, before this round", text
        )

    def test_a_first_round_mutation_log_on_the_base_should_fail(self) -> None:
        log = Path(self.repo) / "mutation.log"
        log.write_text(
            f"mutation-test: PASS: 1 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.base})\n"
        )
        with self.assertRaises(SystemExit):
            self.brief("HEAD~1..HEAD", "--mutation-log", str(log))

    def test_a_second_round_without_rulings_should_fail(self) -> None:
        with self.assertRaises(SystemExit):
            self.brief("HEAD~1..HEAD", "--round", "2")


class RemovedTests(unittest.TestCase):
    """removed_tests and the Focus lists of a diff brief, on a repository whose head edits its tests."""

    @override
    def setUp(self) -> None:
        self.repo = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.repo)
        self.git = ["git", "-C", str(self.repo), "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run([*self.git[:3], "init", "-q"], check=True, env=ENV)
        self.write(
            {
                "src/tests.rs": 'fn kept() {}\n// old note\n(1, "row one"),\n(2, "row two"),\n}\n',
                "tests/gone.sisu": "fn main() {}\n",
                "tests/a.err": "error[E0032]\n",
                "src/lib.rs": "fn dropped() {}\n",
            }
        )
        self.base = self.commit("base")
        # Row two goes, row one moves below the brace, the note changes; none of the rest is in a test.
        self.write(
            {
                "src/tests.rs": 'fn kept() {}\n// new note\n}\n(1, "row one"),\n',
                "tests/a.err": "error[E0050]\n",
                "src/lib.rs": "",
            }
        )
        (self.repo / "tests/gone.sisu").unlink()
        self.head = self.commit("head")

    def write(self, files: dict[str, str]) -> None:
        for name, text in files.items():
            (self.repo / name).parent.mkdir(parents=True, exist_ok=True)
            (self.repo / name).write_text(text)

    def commit(self, message: str) -> str:
        subprocess.run([*self.git, "add", "-A"], check=True, env=ENV)
        subprocess.run([*self.git, "commit", "-q", "-m", message], check=True, env=ENV)
        return subprocess.run(
            [*self.git[:3], "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=True,
            env=ENV,
        ).stdout.strip()

    def test_removed_tests_should_list_a_dropped_row_and_a_deleted_file_only(self) -> None:
        with mock.patch.dict(os.environ, ENV, clear=True):
            removed = diff_brief.removed_tests(self.repo, self.base, self.head)
        self.assertEqual(removed, ['src/tests.rs:4: (2, "row two"),', "tests/gone.sisu: deleted"])

    def test_removed_tests_should_read_a_byte_that_is_not_utf_8(self) -> None:
        # E0072.sisu and wc.in hold such bytes; the diff decoded strictly and stopped the brief.
        (self.repo / "tests/raw.in").write_bytes(b"\xff kept\n\xfe gone\n")
        base = self.commit("raw")
        (self.repo / "tests/raw.in").write_bytes(b"\xff kept\n")
        head = self.commit("raw edit")
        with mock.patch.dict(os.environ, ENV, clear=True):
            removed = diff_brief.removed_tests(self.repo, base, head)
        self.assertEqual(removed, ["tests/raw.in:2: \ufffd gone"])

    def test_removed_tests_should_keep_their_line_numbers_under_git_diff_opts(self) -> None:
        self.write({"tests/opts.in": "a\nb\nc\n"})
        base = self.commit("opts")
        self.write({"tests/opts.in": "a\nc\n"})
        head = self.commit("opts edit")
        with mock.patch.dict(os.environ, {**ENV, "GIT_DIFF_OPTS": "--unified=3"}, clear=True):
            removed = diff_brief.removed_tests(self.repo, base, head)
        self.assertEqual(removed, ["tests/opts.in:2: b"])

    def test_removed_tests_should_name_a_path_that_is_not_ascii_as_written(self) -> None:
        # git quotes such a path as "tests/\303\244.in" unless core.quotepath is off.
        self.write({"tests/ä.in": "kept\ngone\n"})
        base = self.commit("a umlaut")
        self.write({"tests/ä.in": "kept\n"})
        head = self.commit("a umlaut edit")
        with mock.patch.dict(os.environ, ENV, clear=True):
            removed = diff_brief.removed_tests(self.repo, base, head)
        self.assertEqual(removed, ["tests/ä.in:2: gone"])

    def test_a_diff_brief_should_list_removed_tests_and_prebuilt_sisu(self) -> None:
        run_dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, run_dir)
        build = run_dir / "bin" / self.head / "sisu"
        build.parent.mkdir(parents=True)
        build.write_text("")
        out = run_dir / "brief.md"
        with (
            mock.patch.dict(os.environ, ENV, clear=True),
            mock.patch.object(
                sys, "argv", ["diff_brief.py", str(self.repo), "HEAD~1..HEAD", str(out)]
            ),
            mock.patch("sys.stderr"),
            mock.patch("sys.stdout"),
        ):
            diff_brief.main()
        focus = out.read_text().split("# Focus", 1)[1]
        self.assertIn('- src/tests.rs:4: (2, "row two"),\n- tests/gone.sisu: deleted', focus)
        self.assertIn(f"- {self.head[:8]} head: {build}", focus)

    def test_a_diff_brief_should_skip_a_build_folder_that_is_not_a_sha(self) -> None:
        run_dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, run_dir)
        for name in (self.head, "fix1"):
            build = run_dir / "bin" / name / "sisu"
            build.parent.mkdir(parents=True)
            build.write_text("")
        out = run_dir / "brief.md"
        with (
            mock.patch.dict(os.environ, ENV, clear=True),
            mock.patch.object(
                sys, "argv", ["diff_brief.py", str(self.repo), "HEAD~1..HEAD", str(out)]
            ),
            mock.patch("sys.stderr"),
            mock.patch("sys.stdout"),
        ):
            diff_brief.main()
        focus = out.read_text().split("# Focus", 1)[1]
        self.assertIn(f"- {self.head[:8]} head: {run_dir / 'bin' / self.head / 'sisu'}", focus)
        self.assertNotIn("fix1", focus)


class GitFailures(Repo):
    """A git command that fails stops the script, unless the failure is an answer."""

    @override
    def setUp(self) -> None:
        super().setUp()
        env = mock.patch.dict(os.environ, ENV, clear=True)
        env.start()
        self.addCleanup(env.stop)

    def test_a_failed_git_should_stop_with_its_error(self) -> None:
        with self.assertRaises(SystemExit) as stop:
            diff_brief.git(self.repo, "rev-list", "no-such-ref")
        self.assertIn("diff_brief: git rev-list no-such-ref: fatal:", str(stop.exception))

    def test_a_failed_diff_should_not_read_as_no_removed_tests(self) -> None:
        with self.assertRaises(SystemExit):
            diff_brief.removed_tests(self.repo, "no-such-ref", self.head)

    def test_a_patch_id_of_an_unknown_commit_should_stop_with_its_error(self) -> None:
        with self.assertRaises(SystemExit) as stop:
            diff_brief.patch_id(self.repo, "0" * 40)
        self.assertIn(f"diff_brief: git show {'0' * 40}:", str(stop.exception))

    def test_a_patch_id_of_an_empty_commit_should_be_empty(self) -> None:
        self.assertEqual(diff_brief.patch_id(self.repo, self.head), "")

    def test_a_ref_that_is_not_a_commit_should_read_as_no_commit(self) -> None:
        self.assertEqual(diff_brief.commit(self.repo, "no-such-ref"), "")
        self.assertEqual(diff_brief.commit(self.repo, "0" * 40), "")

    def test_a_commit_lookup_without_a_repository_should_stop_with_its_error(self) -> None:
        with (
            mock.patch.dict(os.environ, {"GIT_DIR": "/no-such-git-dir"}),
            self.assertRaises(SystemExit) as stop,
        ):
            diff_brief.commit(self.repo, "HEAD")
        self.assertIn("not a git repository", str(stop.exception))

    def test_is_ancestor_should_answer_for_two_commits(self) -> None:
        self.assertTrue(diff_brief.is_ancestor(self.repo, self.base, self.head))
        self.assertFalse(diff_brief.is_ancestor(self.repo, self.head, self.base))

    def test_is_ancestor_of_an_unknown_commit_should_stop_with_its_error(self) -> None:
        with self.assertRaises(SystemExit) as stop:
            diff_brief.is_ancestor(self.repo, "0" * 40, self.head)
        self.assertIn("diff_brief: git merge-base --is-ancestor", str(stop.exception))

    def test_a_log_on_an_unknown_commit_should_be_outside_the_range(self) -> None:
        log = Path(self.repo) / "mutation.log"
        log.write_text(f"mutation-test: PASS: 1 caught (HEAD {'0' * 40})\n")
        for earlier in (False, True):
            with self.subTest(earlier=earlier):
                why = diff_brief.log_mismatch(log, self.repo, self.base, "HEAD", earlier=earlier)
                assert why is not None
                self.assertIn(f"measured {'0' * 8}, which is not in", why)


if __name__ == "__main__":
    unittest.main()
