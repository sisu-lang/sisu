"""Unit tests for merge_pr.py: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script with a fake `gh` on PATH that answers from a table and logs each call,
on a small repository with one worktree.
"""
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / 'merge_pr.py'
# A pre-commit hook runs these tests with GIT_DIR and the other GIT_ variables set to the outer repository.
ENV = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
# The fake gh finds the answer for the first argument that names an endpoint. A list answers in
# order, one item per call, and then repeats its last item.
FAKE_GH = f"""#!{sys.executable}
import json, os, sys
args = sys.argv[2:]
with open(os.environ['GH_LOG'], 'a') as f:
    f.write(json.dumps(args) + '\\n')
table = json.load(open(os.environ['GH_TABLE']))
endpoint = next(a for a in args if a.startswith('repos/')).split('?')[0]
answer = table[endpoint]
if isinstance(answer, list):
    calls = sum(endpoint in line for line in open(os.environ['GH_LOG']))
    answer = answer[min(calls, len(answer)) - 1]
print(json.dumps(answer))
"""


class MergePr(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.dir)
        bin_dir = self.dir / 'bin'
        bin_dir.mkdir()
        fake = bin_dir / 'gh'
        fake.write_text(FAKE_GH)
        fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
        self.repo, self.tree = self.dir / 'repo', self.dir / 'tree'
        self.git = ['git', '-c', 'user.name=t', '-c', 'user.email=t@t', '-C']
        subprocess.run(['git', 'init', '-q', str(self.repo)], check=True, env=ENV)
        subprocess.run(self.git + [str(self.repo), 'commit', '-q', '--allow-empty', '-m', 'one'],
                       check=True, env=ENV)
        subprocess.run(self.git + [str(self.repo), 'worktree', 'add', '-q', '-b', 'fix/x',
                                   str(self.tree)], check=True, env=ENV)
        self.sha = subprocess.run(['git', '-C', str(self.tree), 'rev-parse', 'HEAD'],
                                  capture_output=True, text=True, check=True, env=ENV).stdout.strip()
        self.log = self.dir / 'log'
        self.env = dict(ENV, PATH=f'{bin_dir}:{ENV["PATH"]}', GH_LOG=str(self.log),
                        GH_TABLE=str(self.dir / 'table.json'), MERGE_PR_POLL='0')

    def run_script(self, **changes):
        r = 'repos/sisu-lang/sisu'
        done = {'name': 'Test', 'status': 'completed', 'conclusion': 'success'}
        table = {
            f'{r}/pulls/7': {'state': 'open', 'title': 'Fix x', 'body': 'Text\n\nCloses #3\n',
                             'head': {'sha': self.sha, 'ref': 'fix/x'}, 'base': {'ref': 'main'}},
            f'{r}/compare/main...{self.sha}': {'behind_by': 0},
            f'{r}/commits/{self.sha}/check-runs': {'total_count': 2, 'check_runs': [
                done, {'name': 'Test (ARM64)', 'status': 'completed', 'conclusion': 'skipped'}]},
            f'{r}/pulls/7/merge': {'merged': True, 'sha': 'abcdef0123'},
            f'{r}/issues/3': {'state': 'closed'},
        }
        table.update({f'{r}/{k}': v for k, v in changes.items()})
        (self.dir / 'table.json').write_text(json.dumps(table))
        return subprocess.run([sys.executable, str(SCRIPT), '7', str(self.repo)], capture_output=True,
                              text=True, env=self.env)

    def checks(self, *runs, total=None):
        return {'total_count': len(runs) if total is None else total, 'check_runs': list(runs)}

    def merges(self):
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        return [c for c in calls if 'PUT' in c]

    def test_a_green_pull_request_should_merge_at_its_checked_head_and_remove_its_worktree(self):
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        [merge] = self.merges()
        self.assertIn(f'sha={self.sha}', merge)
        self.assertIn('commit_title=Fix x (#7)', merge)
        self.assertIn('issues: #3 closed', result.stdout)
        self.assertFalse(self.tree.exists())
        branches = subprocess.run(['git', '-C', str(self.repo), 'branch', '--list', 'fix/x'],
                                  capture_output=True, text=True, check=True, env=ENV).stdout
        self.assertEqual(branches, '')

    def test_a_failed_check_should_stop_before_the_merge(self):
        failed = {'name': 'Test', 'status': 'completed', 'conclusion': 'failure'}
        result = self.run_script(**{f'commits/{self.sha}/check-runs': self.checks(failed)})
        self.assertEqual(result.returncode, 1)
        self.assertIn('Test: failure', result.stdout)
        self.assertEqual(self.merges(), [])

    def test_a_head_behind_main_should_stop_before_the_merge(self):
        result = self.run_script(**{f'compare/main...{self.sha}': {'behind_by': 2}})
        self.assertEqual(result.returncode, 1)
        self.assertIn('does not contain main', result.stdout)
        self.assertEqual(self.merges(), [])

    def test_a_worktree_with_changes_should_stay_after_the_merge(self):
        (self.tree / 'note').write_text('draft\n')
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('it has uncommitted changes', result.stdout)
        self.assertTrue((self.tree / 'note').exists())

    def test_a_worktree_with_a_commit_after_the_merged_head_should_stay(self):
        subprocess.run(self.git + [str(self.tree), 'commit', '-q', '--allow-empty', '-m', 'two'],
                       check=True, env=ENV)
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('its head is not the merged head', result.stdout)
        self.assertTrue(self.tree.exists())

    def test_a_running_check_should_be_waited_for_before_the_merge(self):
        running = {'name': 'Test', 'status': 'in_progress', 'conclusion': None}
        done = dict(running, status='completed', conclusion='success')
        result = self.run_script(**{f'commits/{self.sha}/check-runs': [self.checks(running), self.checks(done)]})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len(self.merges()), 1)

    def test_a_head_with_no_checks_should_stop_before_the_merge(self):
        self.env['MERGE_PR_CHECKS_LIMIT'] = '0'
        result = self.run_script(**{f'commits/{self.sha}/check-runs': self.checks()})
        self.assertEqual(result.returncode, 1)
        self.assertIn('the head has no checks', result.stdout)
        self.assertEqual(self.merges(), [])

    def test_checks_on_a_second_page_should_stop_before_the_merge(self):
        done = {'name': 'Test', 'status': 'completed', 'conclusion': 'success'}
        result = self.run_script(**{f'commits/{self.sha}/check-runs': self.checks(done, total=101)})
        self.assertEqual(result.returncode, 1)
        self.assertIn('more than one page', result.stdout)
        self.assertEqual(self.merges(), [])

    def test_a_closed_pull_request_should_not_merge(self):
        result = self.run_script(**{'pulls/7': {'state': 'closed'}})
        self.assertEqual(result.returncode, 1)
        self.assertIn('#7 is closed', result.stdout)
        self.assertEqual(self.merges(), [])

    def test_main_that_moves_during_the_wait_should_stop_the_merge(self):
        result = self.run_script(**{f'compare/main...{self.sha}': [{'behind_by': 0}, {'behind_by': 1}]})
        self.assertEqual(result.returncode, 1)
        self.assertIn('does not contain main', result.stdout)
        self.assertEqual(self.merges(), [])


if __name__ == '__main__':
    unittest.main()
