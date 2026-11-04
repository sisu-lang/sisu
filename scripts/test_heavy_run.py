"""Tests for scripts/heavy-run.sh: one heavy run at a time for all the worktrees of a clone."""

import fcntl
import os
import shutil
import signal
import subprocess
import tempfile
import time
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'heavy-run.sh')
# A pre-commit hook runs these tests with GIT_DIR and its kin set to the outer repository.
# SISU_HEAVY_LOCK=skip in the shell would let the tests that wait for the lock run at once.
ENV = {k: v for k, v in os.environ.items()
       if not k.startswith('GIT_') and k != 'SISU_HEAVY_LOCK'}


def wait_for(condition, what):
    deadline = time.monotonic() + 20
    while not condition():
        if time.monotonic() > deadline:
            raise AssertionError(f'gave up waiting for {what}')
        time.sleep(0.02)


class HeavyRun(unittest.TestCase):
    def setUp(self):
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        subprocess.run(['git', 'init', '-q', self.repo], check=True, env=ENV)
        self.lock = os.path.join(self.repo, '.git', 'sisu-heavy-run.lock')
        self.err = os.path.join(self.repo, 'err')

    def path(self, name):
        return os.path.join(self.repo, name)

    def start(self, *command, cwd=None, env=None):
        with open(self.err, 'w', encoding='utf-8') as err:
            run = subprocess.Popen(['bash', SCRIPT, *command], cwd=cwd or self.repo,
                                   env={**ENV, **(env or {})},
                                   stderr=err, start_new_session=True)
        self.addCleanup(self.stop, run)
        return run

    @staticmethod
    def stop(run):
        if run.poll() is None:
            os.killpg(run.pid, signal.SIGKILL)
        run.wait()

    def stderr(self):
        with open(self.err, encoding='utf-8') as f:
            return f.read()

    def test_a_run_should_wait_for_the_run_that_holds_the_lock_and_name_it(self):
        with open(self.lock, 'w', encoding='utf-8') as held:
            held.write('make test in /elsewhere (pid 1)\n')
            held.flush()
            fcntl.flock(held, fcntl.LOCK_EX)
            run = self.start('touch', 'ran')
            # The script writes the two lines with two `echo`s, so wait for the second one.
            wait_for(lambda: 'when the lock is free' in self.stderr(), 'the waiting message')
            self.assertIn('heavy-run: waiting for this run to end: make test in /elsewhere', self.stderr())
            self.assertIn('heavy-run: the run starts when the lock is free', self.stderr())
            self.assertIsNone(run.poll())
            self.assertFalse(os.path.exists(self.path('ran')))
        self.assertEqual(run.wait(timeout=20), 0)
        self.assertTrue(os.path.exists(self.path('ran')))

    def test_a_run_in_a_linked_worktree_should_wait_for_the_lock_of_the_main_worktree(self):
        git = ['git', '-C', self.repo, '-c', 'user.name=t', '-c', 'user.email=t@t']
        subprocess.run(git + ['commit', '-q', '--allow-empty', '-m', 'base'], check=True, env=ENV)
        linked = self.path('linked')
        subprocess.run(git + ['worktree', 'add', '-q', '--detach', linked], check=True, env=ENV)
        with open(self.lock, 'w', encoding='utf-8') as held:
            held.write('make test in the main worktree (pid 1)\n')
            held.flush()
            fcntl.flock(held, fcntl.LOCK_EX)
            run = self.start('touch', 'ran', cwd=linked)
            wait_for(lambda: 'heavy-run: waiting' in self.stderr(), 'the waiting message')
            self.assertFalse(os.path.exists(os.path.join(linked, 'ran')))
        self.assertEqual(run.wait(timeout=20), 0)
        self.assertTrue(os.path.exists(os.path.join(linked, 'ran')))

    def test_a_run_should_hold_the_lock_until_its_command_ends(self):
        os.mkfifo(self.path('go'))
        self.start('sh', '-c', 'touch ready; read line < go')
        wait_for(lambda: os.path.exists(self.path('ready')), 'the command to start')
        with open(self.lock, encoding='utf-8') as other:
            self.assertIn('sh -c touch ready; read line < go in ', other.read())
            with self.assertRaises(BlockingIOError):
                fcntl.flock(other, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_a_run_with_the_skip_variable_should_run_at_once_while_another_holds_the_lock(self):
        with open(self.lock, 'w+', encoding='utf-8') as held:
            held.write('make mutation-test in /elsewhere (pid 1)\n')
            held.flush()
            fcntl.flock(held, fcntl.LOCK_EX)
            run = self.start('touch', 'ran', env={'SISU_HEAVY_LOCK': 'skip'})
            self.assertEqual(run.wait(timeout=20), 0)
            self.assertTrue(os.path.exists(self.path('ran')))
            self.assertIn('heavy-run: SISU_HEAVY_LOCK=skip; running with no lock', self.stderr())
            held.seek(0)
            self.assertEqual(held.read(), 'make mutation-test in /elsewhere (pid 1)\n')

    def test_a_run_with_no_git_directory_should_warn_and_run_with_no_lock(self):
        shutil.rmtree(os.path.join(self.repo, '.git'))
        run = self.start('touch', 'ran')
        self.assertEqual(run.wait(timeout=20), 0)
        self.assertIn("heavy-run: no lock in the clone's .git; running with no lock", self.stderr())
        self.assertTrue(os.path.exists(self.path('ran')))


if __name__ == '__main__':
    unittest.main()
