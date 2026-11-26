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

ROOT = Path(__file__).absolute().parent / ".."
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
            check=False,
        )
        self.assertEqual((run.returncode, run.stdout), (0, ""))

    def test_a_new_sisu_fixture_without_given_when_then_should_fail(self) -> None:
        old_fixture = "crates/sisuc/tests/diagnostics/old.sisu"
        new_bad = "crates/sisuc/tests/diagnostics/new_bad.sisu"
        new_good = "crates/sisuc/tests/run/new_good.sisu"
        with git_fixture.repo({old_fixture: "fn main() {}\n"}) as tmp:
            baseline = Path(tmp, "baseline.txt")
            write(tmp, "baseline.txt", "")
            cwd = Path.cwd()
            os.chdir(tmp)
            self.addCleanup(os.chdir, cwd)
            # An existing fixture at HEAD is skipped even without GIVEN/WHEN/THEN.
            self.assertEqual(run(["--diff", "HEAD", old_fixture], baseline), (0, ""))
            write(tmp, new_bad, "// just a note\nfn main() {}\n")
            Path(tmp, new_good).parent.mkdir(parents=True, exist_ok=True)
            write(
                tmp,
                new_good,
                "// args: one\n// GIVEN a program\n// WHEN it runs\n// THEN it prints 1\n"
                "fn main() { print(1) }\n",
            )
            code, out = run(["--diff", "HEAD", new_bad, new_good], baseline)
        self.assertEqual(code, 1)
        self.assertEqual(
            out.splitlines(),
            [
                (
                    f"{new_bad}:1: new fixture `new_bad.sisu` needs leading "
                    "// GIVEN, // WHEN and // THEN lines"
                )
            ],
        )

    def test_a_new_run_fail_fixture_with_non_utf8_bytes_should_be_checked(self) -> None:
        fixture = "crates/sisuc/tests/run-fail/bad_bytes.sisu"
        with git_fixture.repo({"f.rs": ""}) as tmp:
            baseline = Path(tmp, "baseline.txt")
            write(tmp, "baseline.txt", "")
            target = Path(tmp, fixture)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(b"fn main() {}\n// \xff\n")
            cwd = Path.cwd()
            os.chdir(tmp)
            self.addCleanup(os.chdir, cwd)
            code, out = run(["--diff", "HEAD", fixture], baseline)
        self.assertEqual(code, 1)
        self.assertIn("new fixture `bad_bytes.sisu` needs leading", out)

    def test_a_bom_or_trailing_comment_fixture_should_pass(self) -> None:
        bom = "crates/sisuc/tests/diagnostics/bom_case.sisu"
        trailing = "crates/sisuc/tests/diagnostics/byte0_case.sisu"
        with git_fixture.repo({"f.rs": ""}) as tmp:
            baseline = Path(tmp, "baseline.txt")
            write(tmp, "baseline.txt", "")
            for rel, content in (
                (
                    bom,
                    "\ufeff// GIVEN a BOM\n// WHEN checked\n// THEN E0010\nfn main() {}\n",
                ),
                (
                    trailing,
                    "@\nfn main() {}\n// GIVEN bad byte 0\n// WHEN checked\n// THEN E0010\n",
                ),
            ):
                target = Path(tmp, rel)
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content, encoding="utf-8")
            cwd = Path.cwd()
            os.chdir(tmp)
            self.addCleanup(os.chdir, cwd)
            self.assertEqual(run(["--diff", "HEAD", bom, trailing], baseline), (0, ""))

    def test_an_existing_non_ascii_sisu_fixture_should_be_skipped(self) -> None:
        non_ascii = "crates/sisuc/tests/run/ä.sisu"
        with git_fixture.repo({non_ascii: "fn main() {}\n"}) as tmp:
            baseline = Path(tmp, "baseline.txt")
            write(tmp, "baseline.txt", "")
            cwd = Path.cwd()
            os.chdir(tmp)
            self.addCleanup(os.chdir, cwd)
            self.assertEqual(run(["--diff", "HEAD", non_ascii], baseline), (0, ""))


if __name__ == "__main__":
    unittest.main()
