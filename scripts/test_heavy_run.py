"""Tests for scripts/heavy-run.sh: one heavy run at a time for all the worktrees of a clone."""

import fcntl
import os
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'heavy-run.sh')
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}


class HeavyRun(unittest.TestCase):
    def setUp(self):
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        subprocess.run(['git', 'init', '-q', self.repo], check=True, env=ENV)
        self.lock = os.path.join(self.repo, '.git', 'sisu-heavy-run.lock')

    def start(self):
        return subprocess.Popen(['bash', SCRIPT, 'touch', 'ran'], cwd=self.repo, env=ENV,
                                stderr=subprocess.PIPE, text=True)

    def test_a_run_should_wait_for_the_run_that_holds_the_lock_and_name_it(self):
        with open(self.lock, 'w', encoding='utf-8') as held:
            held.write('make test in /elsewhere (pid 1)\n')
            held.flush()
            fcntl.flock(held, fcntl.LOCK_EX)
            run = self.start()
            with self.assertRaises(subprocess.TimeoutExpired):
                run.wait(timeout=0.5)
            self.assertFalse(os.path.exists(os.path.join(self.repo, 'ran')))
        _, err = run.communicate(timeout=10)
        self.assertEqual(run.returncode, 0)
        self.assertIn('heavy-run: waiting for this run to end: make test in /elsewhere', err)
        self.assertTrue(os.path.exists(os.path.join(self.repo, 'ran')))

    def test_a_run_should_hold_the_lock_until_its_command_ends(self):
        run = subprocess.Popen(['bash', SCRIPT, 'sleep', '2'], cwd=self.repo, env=ENV)
        self.addCleanup(run.wait)
        # The script takes the lock at once; 0.5 s leaves it time to reach the command.
        subprocess.run(['sleep', '0.5'], check=True)
        with open(self.lock, encoding='utf-8') as other:
            self.assertIn('sleep 2 in ', other.read())
            with self.assertRaises(BlockingIOError):
                fcntl.flock(other, fcntl.LOCK_EX | fcntl.LOCK_NB)


if __name__ == '__main__':
    unittest.main()
