"""Unit tests for the diff and baseline checks of test_names.py:
python3 -m unittest discover -s scripts -p 'test_*.py'.
"""

import contextlib
import io
import os
import subprocess
import sys
import unittest
from pathlib import Path

import git_fixture
import test_names

ROOT = Path(__file__).absolute().parent.parent
BASELINE_TEST = (
    "#[test]\nfn old_name() {\n    one();\n}\n\n#[test]\nfn other_name() {\n    two();\n}\n"
)


def run(argv: list[str], baseline: Path) -> tuple[int, str]:
    """The exit code and output of `test_names.py ARGV` with `baseline` as its baseline file."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = test_names.cli(argv, baseline)
    return code, out.getvalue()


write = git_fixture.write


class TestNamesDiffAndBaseline(unittest.TestCase):
    def test_a_changed_baseline_test_should_fail(self) -> None:
        with git_fixture.repo({"crates/a/src/f.rs": BASELINE_TEST}) as tmp:
            baseline = Path(tmp, "baseline.txt")
            write(
                tmp, "baseline.txt", "crates/a/src/f.rs::old_name\ncrates/a/src/f.rs::other_name\n"
            )
            cwd = Path.cwd()
            os.chdir(tmp)
            self.addCleanup(os.chdir, cwd)
            self.assertEqual(run(["--diff", "HEAD", "crates/a/src/f.rs"], baseline), (0, ""))
            write(tmp, "crates/a/src/f.rs", BASELINE_TEST.replace("one", "uno"))
            code, out = run(["--diff", "HEAD", "crates/a/src/f.rs"], baseline)
        self.assertEqual(code, 1)
        self.assertEqual(
            out.splitlines(),
            [
                (
                    "crates/a/src/f.rs:2: test `old_name` is in test_names_baseline.txt and its body "
                    "changed: rename it to <subject>_should_<result> and remove its baseline line"
                )
            ],
        )

    def test_a_baseline_line_with_no_test_should_fail(self) -> None:
        with git_fixture.repo({"f.rs": BASELINE_TEST}) as tmp:
            baseline = Path(tmp, "scripts", "test_names_baseline.txt")
            baseline.parent.mkdir(parents=True)
            write(
                baseline.parent,
                "test_names_baseline.txt",
                "# a comment\nf.rs::old_name\ngone.rs::old_name\nf.rs::no_name\nf.rs::old_name\n",
            )
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                code = test_names.check_baseline(baseline, Path(tmp))
        self.assertEqual(code, 1)
        self.assertEqual(
            out.getvalue().splitlines(),
            [
                "scripts/test_names_baseline.txt:3: `gone.rs::old_name` names no test",
                "scripts/test_names_baseline.txt:4: `f.rs::no_name` names no test",
                "scripts/test_names_baseline.txt:5: `f.rs::old_name` names no test",
            ],
        )

    def test_a_bad_ref_should_print_gits_error_and_fail(self) -> None:
        with git_fixture.repo({"f.rs": BASELINE_TEST}) as tmp:
            baseline = Path(tmp, "baseline.txt")
            write(tmp, "baseline.txt", "")
            cwd = Path.cwd()
            os.chdir(tmp)
            self.addCleanup(os.chdir, cwd)
            code, out = run(["--diff", "no-such-ref", "f.rs"], baseline)
        self.assertEqual(code, 1)
        self.assertRegex(out, r"^test_names: `git diff no-such-ref` failed: .*no-such-ref")

    def test_the_baseline_should_name_only_tests_on_the_tree(self) -> None:
        run = subprocess.run(
            [sys.executable, ROOT / "scripts" / "test_names.py", "--baseline"],
            capture_output=True,
            text=True,
            cwd=ROOT,
        )
        self.assertEqual((run.returncode, run.stdout), (0, ""))


if __name__ == "__main__":
    unittest.main()
