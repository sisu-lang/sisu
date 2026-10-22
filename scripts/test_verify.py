"""Unit tests for verify.sh's lock and log: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import fcntl
import os
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'verify.sh')
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}


class Verify(unittest.TestCase):
    """verify.sh in a one-commit repository with no Rust changes, so it returns at once."""

    def setUp(self):
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        os.mkdir(os.path.join(self.repo, 'scripts'))
        shutil.copy(SCRIPT, os.path.join(self.repo, 'scripts'))
        git = ['git', '-C', self.repo, '-c', 'user.name=t', '-c', 'user.email=t@t']
        subprocess.run(git[:3] + ['init', '-q'], check=True, env=ENV)
        subprocess.run(git + ['commit', '-q', '--allow-empty', '-m', 'base'], check=True, env=ENV)
        self.log = os.path.join(self.repo, 'verify.log')

    def verify(self):
        return subprocess.run(['bash', 'scripts/verify.sh', 'HEAD', self.log], cwd=self.repo,
                              capture_output=True, text=True, env=ENV)

    def test_the_verdict_goes_to_the_log(self):
        run = self.verify()
        self.assertEqual(run.returncode, 0)
        self.assertEqual(run.stdout, '')
        with open(self.log, encoding='utf-8') as f:
            self.assertIn('verify: PASS: no Rust changes since HEAD', f.read())

    def test_a_second_run_leaves_the_first_runs_log_alone(self):
        with open(self.log, 'w', encoding='utf-8') as f:
            f.write('first run\n')
        os.mkdir(os.path.join(self.repo, 'target'))
        with open(os.path.join(self.repo, 'target', 'verify.lock'), 'w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            run = self.verify()
        self.assertEqual(run.returncode, 1)
        self.assertIn('another make verify is running in this worktree', run.stdout)
        with open(self.log, encoding='utf-8') as f:
            self.assertEqual(f.read(), 'first run\n')


if __name__ == '__main__':
    unittest.main()
