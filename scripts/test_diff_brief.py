"""Unit tests for review-sisu's diff_brief.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import importlib.util
import os
import shutil
import subprocess
import sys
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


class Repo(unittest.TestCase):
    """A repository of three commits, base, mid and head, and a side commit off base."""

    def setUp(self):
        self.repo = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.repo)
        self.git = ['git', '-C', self.repo, '-c', 'user.name=t', '-c', 'user.email=t@t']
        subprocess.run(self.git[:3] + ['init', '-q'], check=True, env=ENV)
        for message in ('base', 'mid', 'head'):
            subprocess.run(self.git + ['commit', '-q', '--allow-empty', '-m', message], check=True, env=ENV)
        self.head, self.mid, self.base = (self.sha(r) for r in ('HEAD', 'HEAD~1', 'HEAD~2'))
        self.side = subprocess.run(self.git + ['commit-tree', 'HEAD~2^{tree}', '-p', self.base, '-m', 'side'],
                                   capture_output=True, text=True, check=True, env=ENV).stdout.strip()

    def sha(self, ref):
        return subprocess.run(self.git[:3] + ['rev-parse', ref], capture_output=True,
                              text=True, check=True, env=ENV).stdout.strip()


class LogMismatch(Repo):
    """log_mismatch on logs of the range base..HEAD."""

    def mismatch(self, text):
        log = Path(self.repo) / 'mutation.log'
        log.write_text(text)
        with mock.patch.dict(os.environ, ENV, clear=True):
            return diff_brief.log_mismatch(log, self.repo, self.base, 'HEAD')

    def summary(self, sha):
        return f'mutation-test: PASS: 2 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {sha})\n'

    def test_a_summary_on_the_head_should_pass(self):
        self.assertIsNone(self.mismatch(f'mutants\nmutation-test: FAIL: 0 caught, 0 unviable, 2 missed, 0 timeouts (HEAD {self.head})\n'))

    def test_a_summary_on_an_earlier_commit_of_the_range_should_pass(self):
        self.assertIsNone(self.mismatch(self.summary(self.mid)))

    def test_a_summary_before_a_merge_of_main_should_pass(self):
        main = subprocess.run(self.git + ['commit-tree', 'HEAD~2^{tree}', '-p', self.base, '-m', 'main'],
                              capture_output=True, text=True, check=True, env=ENV).stdout.strip()
        subprocess.run(self.git + ['merge', '-q', '--no-edit', main], check=True, env=ENV)
        self.base = main
        self.assertIsNone(self.mismatch(self.summary(self.mid)))

    def test_a_summary_on_the_base_should_fail(self):
        self.assertIn(f'measured {self.base[:8]}, which is not in', self.mismatch(self.summary(self.base)))

    def test_a_summary_off_the_range_should_fail(self):
        self.assertIn(f'measured {self.side[:8]}, which is not in', self.mismatch(self.summary(self.side)))

    def test_a_summary_on_uncommitted_changes_should_fail(self):
        why = self.mismatch(f'mutation-test: PASS: 2 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.head} with uncommitted changes)\n')
        self.assertIn('measured uncommitted changes', why)

    def test_a_log_without_a_summary_should_fail(self):
        self.assertIn('no summary line', self.mismatch('coverage\nmutants still running\n'))


class Brief(Repo):
    """The brief that main() writes; the test repository has no origin/main, so the script's own template is used."""

    def brief(self, *args):
        self.run_dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.run_dir)
        out = self.run_dir / 'brief.md'
        with mock.patch.dict(os.environ, ENV, clear=True), \
             mock.patch.object(sys, 'argv', ['diff_brief.py', self.repo, *args[:1], str(out), *args[1:]]), \
             mock.patch('sys.stderr'), mock.patch('sys.stdout'):
            diff_brief.main()
        return out.read_text()

    def test_a_diff_brief_should_name_its_range_by_full_shas(self):
        text = self.brief('HEAD~2..HEAD')
        self.assertIn(f'Target: {self.base}..{self.head}.', text)
        self.assertNotIn('# Check, for a plan', text)
        self.assertNotIn('# Re-review', text)

    def test_a_mutation_log_on_an_earlier_commit_should_list_the_later_commits(self):
        log = Path(self.repo) / 'mutation.log'
        log.write_text(f'mutation-test: PASS: 1 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {self.mid})\n')
        text = self.brief('HEAD~2..HEAD', '--mutation-log', str(log))
        self.assertIn(f'measured at {self.mid[:8]}. Commits after it, not mutated: {self.head[:8]}', text)

    def test_a_coverage_result_should_be_named_in_focus(self):
        coverage = Path(self.repo) / 'coverage.txt'
        coverage.write_text('uncovered: none\n')
        self.assertIn(f'Coverage result: {coverage}', self.brief('HEAD~2..HEAD', '--coverage', str(coverage)))

    def test_a_missing_plan_should_fail_before_writing(self):
        with self.assertRaises(SystemExit):
            self.brief(str(Path(self.repo) / 'no-plan.md'), '--kind', 'plan')
        self.assertEqual(list(self.run_dir.iterdir()), [])

    def test_a_second_plan_round_should_name_the_previous_version(self):
        plan, prior = Path(self.repo) / 'plan.md', Path(self.repo) / 'sol.md'
        plan.write_text('# Plan v1\n')
        prior.write_text('Verdict: changes needed\n')
        run_dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, run_dir)
        for args in ([], ['--round', '2', '--prior', str(prior), '--rulings', str(prior)]):
            out = run_dir / ('brief-2.md' if args else 'brief.md')
            with mock.patch.dict(os.environ, ENV, clear=True), \
                 mock.patch.object(sys, 'argv', ['diff_brief.py', self.repo, str(plan), str(out), '--kind', 'plan', *args]), \
                 mock.patch('sys.stderr'), mock.patch('sys.stdout'):
                diff_brief.main()
            plan.write_text('# Plan v2\n')
        text = (run_dir / 'brief-2.md').read_text()
        self.assertIn(f'- The previous version: {run_dir / "target-1.md"}.', text)
        self.assertIn(f'The first brief, {run_dir / "brief.md"}, names', ' '.join(text.split()))
        self.assertEqual((run_dir / 'target-2.md').read_text(), '# Plan v2\n')

    def test_a_second_plan_round_without_a_first_snapshot_should_drop_the_previous_version(self):
        plan = Path(self.repo) / 'plan.md'
        plan.write_text('# Plan\n')
        text = self.brief(str(plan), '--kind', 'plan', '--round', '2', '--prior', str(plan), '--rulings', str(plan))
        self.assertNotIn('The previous version', text)

    def test_a_plan_brief_should_read_a_snapshot_and_drop_the_diff_lines(self):
        plan = Path(self.repo) / 'plan.md'
        plan.write_text('# Plan\n')
        text = self.brief(str(plan), '--kind', 'plan')
        snapshot = self.run_dir / 'target-1.md'
        self.assertEqual(snapshot.read_text(), '# Plan\n')
        self.assertIn(f'Target: {snapshot}.', text)
        self.assertIn('# Check, for a plan', text)
        self.assertNotIn('# Check, for a diff', text)
        self.assertNotIn('For a diff,', text.split('# Values')[0])

    def test_a_second_round_should_keep_the_re_review_section(self):
        prior = Path(self.repo) / 'sol.md'
        prior.write_text('Verdict: changes needed\n')
        text = self.brief('HEAD~1..HEAD', '--round', '2', '--prior', str(prior), '--rulings', str(prior))
        section = text.split('# Re-review', 1)[1].split('\n# ', 1)[0]
        self.assertIn('This is round 2.', section)
        self.assertNotIn('The previous version', section)

    def test_a_second_round_without_rulings_should_fail(self):
        with self.assertRaises(SystemExit):
            self.brief('HEAD~1..HEAD', '--round', '2')

if __name__ == '__main__':
    unittest.main()
