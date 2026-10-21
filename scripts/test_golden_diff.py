"""Unit tests for golden_diff.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import contextlib
import io
import os
import subprocess
import tempfile
import unittest
from unittest import mock

import golden_diff as gd


def commit(repo, files):
    for name, text in files.items():
        os.makedirs(os.path.join(repo, os.path.dirname(name)), exist_ok=True)
        with open(os.path.join(repo, name), 'w', encoding='utf-8') as f:
            f.write(text)
    subprocess.run(['git', '-C', repo, 'add', '.'], check=True)
    subprocess.run(['git', '-C', repo, '-c', 'user.name=t', '-c', 'user.email=t@t',
                    'commit', '-qm', 'x'], check=True)
    return subprocess.run(['git', '-C', repo, 'rev-parse', 'HEAD'], capture_output=True,
                          text=True, check=True).stdout.strip()


class GoldenDiff(unittest.TestCase):
    def setUp(self):
        # A pre-commit hook sets GIT_DIR and GIT_INDEX_FILE, which would aim every git call
        # here at the repository being committed to.
        clean = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
        patch = mock.patch.dict(os.environ, clean, clear=True)
        patch.start()
        self.addCleanup(patch.stop)

    def run_in(self, files_before, files_after, subs):
        with tempfile.TemporaryDirectory() as repo:
            subprocess.run(['git', 'init', '-q', repo], check=True)
            base = commit(repo, files_before)
            head = commit(repo, files_after)
            out = io.StringIO()
            cwd = os.getcwd()
            os.chdir(repo)
            try:
                with contextlib.redirect_stdout(out):
                    status = gd.main(f'{base}..{head}', subs)
            finally:
                os.chdir(cwd)
        return status, out.getvalue().splitlines()

    def test_passes_when_every_change_is_a_substitution(self):
        status, lines = self.run_in(
            {'crates/c/tests/a.err': 'old: x\nsame\n', 'crates/c/tests/b.sisu': '1'},
            {'crates/c/tests/a.err': 'new: x\nsame\n', 'crates/c/tests/b.sisu': '2'},
            [('old: ', 'new: ')])
        self.assertEqual(status, 0)
        self.assertEqual(lines, ['golden_diff: 1 files, 1 changed lines, 0 problems'])

    def test_fails_on_any_other_change_to_a_case(self):
        status, lines = self.run_in(
            {'crates/c/tests/a.err': 'old: x\n', 'crates/c/tests/b.out': '1\n', 'docs/d.json': '1'},
            {'crates/c/tests/a.err': 'new: y\n', 'crates/c/tests/b.out': '1\n2\n',
             'docs/d.json': '2'},
            [('old: ', 'new: ')])
        self.assertEqual(status, 1)
        self.assertEqual(lines, ["crates/c/tests/a.err: 'old: x' became 'new: y'",
                                 'crates/c/tests/b.out: 2 lines became 3',
                                 'golden_diff: 2 files, 1 changed lines, 2 problems'])


if __name__ == '__main__':
    unittest.main()
