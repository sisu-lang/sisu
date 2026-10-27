"""Unit tests for the diff and baseline checks of test_names.py:
python3 -m unittest discover -s scripts -p 'test_*.py'."""
import contextlib
import io
import os
import subprocess
import sys
import unittest

import git_fixture
import test_names

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASELINE_TEST = '#[test]\nfn old_name() {\n    one();\n}\n\n#[test]\nfn other_name() {\n    two();\n}\n'


def run(argv, baseline):
    """The exit code and output of `test_names.py ARGV` with `baseline` as its baseline file."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = test_names.cli(argv, baseline)
    return code, out.getvalue()


def write(path, text):
    with open(path, 'w', encoding='utf-8') as file:
        file.write(text)


class TestNamesDiffAndBaseline(unittest.TestCase):
    def test_a_changed_baseline_test_should_fail(self):
        with git_fixture.repo({'crates/a/src/f.rs': BASELINE_TEST}) as tmp:
            baseline = os.path.join(tmp, 'baseline.txt')
            write(baseline, 'crates/a/src/f.rs::old_name\ncrates/a/src/f.rs::other_name\n')
            cwd = os.getcwd()
            os.chdir(tmp)
            self.addCleanup(os.chdir, cwd)
            self.assertEqual(run(['--diff', 'HEAD', 'crates/a/src/f.rs'], baseline), (0, ''))
            write('crates/a/src/f.rs', BASELINE_TEST.replace('one', 'uno'))
            code, out = run(['--diff', 'HEAD', 'crates/a/src/f.rs'], baseline)
        self.assertEqual(code, 1)
        self.assertEqual(out.splitlines(), [
            'crates/a/src/f.rs:2: test `old_name` is in test_names_baseline.txt and its body '
            'changed: rename it to <subject>_should_<result> and remove its baseline line'])

    def test_a_baseline_line_with_no_test_should_fail(self):
        with git_fixture.repo({'f.rs': BASELINE_TEST}) as tmp:
            baseline = os.path.join(tmp, 'scripts', 'test_names_baseline.txt')
            os.makedirs(os.path.dirname(baseline))
            write(baseline, '# a comment\nf.rs::old_name\ngone.rs::old_name\nf.rs::no_name\n'
                            'f.rs::old_name\n')
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                code = test_names.check_baseline(baseline, tmp)
        self.assertEqual(code, 1)
        self.assertEqual(out.getvalue().splitlines(), [
            'scripts/test_names_baseline.txt:3: `gone.rs::old_name` names no test',
            'scripts/test_names_baseline.txt:4: `f.rs::no_name` names no test',
            'scripts/test_names_baseline.txt:5: `f.rs::old_name` names no test'])

    def test_the_baseline_should_name_only_tests_on_the_tree(self):
        run = subprocess.run([sys.executable, os.path.join(ROOT, 'scripts', 'test_names.py'), '--baseline'],
                             capture_output=True, text=True, cwd=ROOT)
        self.assertEqual((run.returncode, run.stdout), (0, ''))


if __name__ == '__main__':
    unittest.main()
