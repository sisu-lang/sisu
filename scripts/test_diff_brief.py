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

    def rebase(self, text):
        """A side commit off base that writes `f`, and its copy on HEAD, as a rebase leaves it."""
        tree = subprocess.run(self.git[:3] + ['mktree'], input=f'100644 blob {self.blob(text)}\tf\n',
                              capture_output=True, text=True, check=True, env=ENV).stdout.strip()
        side = subprocess.run(self.git + ['commit-tree', tree, '-p', self.base, '-m', 'side'],
                              capture_output=True, text=True, check=True, env=ENV).stdout.strip()
        subprocess.run(self.git + ['cherry-pick', side], capture_output=True, check=True, env=ENV)
        return side

    def blob(self, text):
        return subprocess.run(self.git[:3] + ['hash-object', '-w', '--stdin'], input=text,
                              capture_output=True, text=True, check=True, env=ENV).stdout.strip()


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
        self.assertIn('no summary line', why)

    def test_a_log_without_a_summary_should_fail(self):
        self.assertIn('no summary line', self.mismatch('coverage\nmutants still running\n'))

    def test_a_summary_on_a_commit_that_a_rebase_copied_into_the_range_should_pass(self):
        self.assertIsNone(self.mismatch(self.summary(self.rebase('x\n'))))

    def test_a_summary_on_a_commit_whose_patch_differs_from_the_range_should_fail(self):
        side = self.rebase('x\n')
        (Path(self.repo) / 'f').write_text('y\n')
        subprocess.run(self.git + ['commit', '-q', '--amend', '-a', '-m', 'changed'], check=True, env=ENV)
        self.assertIn(f'measured {side[:8]}, which is not in', self.mismatch(self.summary(side)))


class CoverageMismatch(Repo):
    """coverage_mismatch on outputs of `make coverage` for the range base..HEAD."""

    def mismatch(self, text):
        path = Path(self.repo) / 'coverage.txt'
        path.write_text(text)
        with mock.patch.dict(os.environ, ENV, clear=True):
            return diff_brief.coverage_mismatch(path, self.repo, 'HEAD')

    def test_coverage_on_the_head_should_pass(self):
        self.assertIsNone(self.mismatch(f'coverage: measured (HEAD {self.head})\nCoverage: 90%\n'))

    def test_coverage_on_an_earlier_commit_of_the_range_should_fail(self):
        why = self.mismatch(f'coverage: measured (HEAD {self.mid})\nCoverage: 90%\n')
        self.assertIn(f'measured {self.mid[:8]}, not the head {self.head[:8]}', why)

    def test_coverage_on_uncommitted_changes_should_fail(self):
        why = self.mismatch(f'coverage: measured (HEAD {self.head} with uncommitted changes)\nCoverage: 90%\n')
        self.assertIn('measured uncommitted changes', why)

    def test_coverage_without_a_coverage_line_should_fail(self):
        self.assertIn('no coverage line', self.mismatch('Coverage: 90%\n'))


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

    def test_a_mutation_log_before_a_rebase_should_name_the_rebased_commit(self):
        side = self.rebase('x\n')
        twin = self.sha('HEAD')
        log = Path(self.repo) / 'mutation.log'
        log.write_text(f'mutation-test: PASS: 1 caught, 0 unviable, 0 missed, 0 timeouts (HEAD {side})\n')
        text = self.brief('HEAD~3..HEAD', '--mutation-log', str(log))
        self.assertIn(f'measured at {side[:8]}, which a rebase made {twin[:8]}\n', text)

    def test_a_coverage_result_should_be_named_in_focus(self):
        coverage = Path(self.repo) / 'coverage.txt'
        coverage.write_text(f'coverage: measured (HEAD {self.head})\nuncovered: none\n')
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

class RemovedTests(unittest.TestCase):
    """removed_tests and the Focus lists of a diff brief, on a repository whose head edits its tests."""

    def setUp(self):
        self.repo = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.repo)
        self.git = ['git', '-C', str(self.repo), '-c', 'user.name=t', '-c', 'user.email=t@t']
        subprocess.run(self.git[:3] + ['init', '-q'], check=True, env=ENV)
        self.write({'src/tests.rs': 'fn kept() {}\n// old note\n(1, "row one"),\n(2, "row two"),\n}\n',
                    'tests/gone.sisu': 'fn main() {}\n', 'tests/a.err': 'error[E0032]\n',
                    'src/lib.rs': 'fn dropped() {}\n'})
        self.base = self.commit('base')
        # Row two goes, row one moves below the brace, the note changes; none of the rest is in a test.
        self.write({'src/tests.rs': 'fn kept() {}\n// new note\n}\n(1, "row one"),\n', 'tests/a.err': 'error[E0050]\n',
                    'src/lib.rs': ''})
        (self.repo / 'tests/gone.sisu').unlink()
        self.head = self.commit('head')

    def write(self, files):
        for name, text in files.items():
            (self.repo / name).parent.mkdir(parents=True, exist_ok=True)
            (self.repo / name).write_text(text)

    def commit(self, message):
        subprocess.run(self.git + ['add', '-A'], check=True, env=ENV)
        subprocess.run(self.git + ['commit', '-q', '-m', message], check=True, env=ENV)
        return subprocess.run(self.git[:3] + ['rev-parse', 'HEAD'], capture_output=True, text=True,
                              check=True, env=ENV).stdout.strip()

    def test_removed_tests_should_list_a_dropped_row_and_a_deleted_file_only(self):
        with mock.patch.dict(os.environ, ENV, clear=True):
            removed = diff_brief.removed_tests(self.repo, self.base, self.head)
        self.assertEqual(removed, ['src/tests.rs:4: (2, "row two"),', 'tests/gone.sisu: deleted'])

    def test_removed_tests_should_read_a_byte_that_is_not_utf_8(self):
        # E0072.sisu and wc.in hold such bytes; the diff decoded strictly and stopped the brief.
        (self.repo / 'tests/raw.in').write_bytes(b'\xff kept\n\xfe gone\n')
        base = self.commit('raw')
        (self.repo / 'tests/raw.in').write_bytes(b'\xff kept\n')
        head = self.commit('raw edit')
        with mock.patch.dict(os.environ, {**ENV, 'GIT_DIFF_OPTS': '--unified=3'}, clear=True):
            removed = diff_brief.removed_tests(self.repo, base, head)
        self.assertEqual(removed, ['tests/raw.in:2: \ufffd gone'])

    def test_a_diff_brief_should_list_removed_tests_and_prebuilt_sisu(self):
        run_dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, run_dir)
        build = run_dir / 'bin' / self.head / 'sisu'
        build.parent.mkdir(parents=True)
        build.write_text('')
        out = run_dir / 'brief.md'
        with mock.patch.dict(os.environ, ENV, clear=True), \
             mock.patch.object(sys, 'argv', ['diff_brief.py', str(self.repo), 'HEAD~1..HEAD', str(out)]), \
             mock.patch('sys.stderr'), mock.patch('sys.stdout'):
            diff_brief.main()
        focus = out.read_text().split('# Focus', 1)[1]
        self.assertIn('- src/tests.rs:4: (2, "row two"),\n- tests/gone.sisu: deleted', focus)
        self.assertIn(f'- {self.head[:8]} head: {build}', focus)


if __name__ == '__main__':
    unittest.main()
