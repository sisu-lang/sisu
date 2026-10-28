"""Unit tests for review-sisu's diff_brief.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import importlib.util
import os
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / '.agents/skills/review-sisu/scripts/diff_brief.py'
spec = importlib.util.spec_from_file_location('diff_brief', SCRIPT)
diff_brief = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diff_brief)
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}


class LogMismatch(unittest.TestCase):
    """log_mismatch on logs of a two-commit repository, whose range ends at HEAD."""

    def setUp(self):
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        git = ['git', '-C', self.repo, '-c', 'user.name=t', '-c', 'user.email=t@t']
        subprocess.run(git[:3] + ['init', '-q'], check=True, env=ENV)
        for message in ('base', 'head'):
            subprocess.run(git + ['commit', '-q', '--allow-empty', '-m', message], check=True, env=ENV)
        sha = lambda ref: subprocess.run(git[:3] + ['rev-parse', ref], capture_output=True,
                                         text=True, check=True, env=ENV).stdout.strip()
        self.head, self.base = sha('HEAD'), sha('HEAD~1')

    def mismatch(self, text):
        log = Path(self.repo) / 'mutation.log'
        log.write_text(text)
        with mock.patch.dict(os.environ, ENV, clear=True):
            return diff_brief.log_mismatch(log, self.repo, 'HEAD')

    def test_a_summary_on_the_head_should_pass(self):
        self.assertIsNone(self.mismatch(f'mutants\nmutation-test: FAIL: 0 caught, 0 unviable, 2 missed, 0 timeouts (HEAD {self.head})\n'))

    def test_a_summary_on_an_earlier_commit_should_fail(self):
        why = self.mismatch(f'mutation-test: PASS: 2 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.base})\n')
        self.assertIn(f'measured {self.base[:8]}, not HEAD', why)

    def test_a_summary_on_uncommitted_changes_should_fail(self):
        why = self.mismatch(f'mutation-test: PASS: 2 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head} with uncommitted changes)\n')
        self.assertIn('measured uncommitted changes', why)

    def test_a_log_without_a_summary_should_fail(self):
        self.assertIn('no summary line', self.mismatch('coverage\nmutants still running\n'))


if __name__ == '__main__':
    unittest.main()
