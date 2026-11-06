"""Tests for nightly_build.py: python3 -m unittest discover -s scripts -p 'test_nightly_build.py'.

Each test runs the script with a fake `gh` on PATH, so nothing reaches GitHub. The fake answers
each query from a JSON state file and records each call. It supplies replies; it never computes
which commit to take.
"""
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from typing import NamedTuple

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'nightly_build.py')
# A is the newest commit. Each is a distinct 40-character string.
A, B, C = ('a' * 40, 'b' * 40, 'c' * 40)
HEAD = 'd' * 40
NO_NEW = 'no new green commit'
DIFFER = 'the newest green commit has other workflow files than `main`'

FAKE_GH = """#!%s
import json, os, sys
args = sys.argv[1:]
state_path = os.environ['FAKE_STATE']
with open(state_path, encoding='utf-8') as f:
    state = json.load(f)
with open(state_path + '.calls', 'a', encoding='utf-8') as f:
    f.write(json.dumps(args) + '\\n')
path = next(a for a in args[1:] if not a.startswith('--'))
if state['fail_on'] and state['fail_on'] in path:
    print('gh: HTTP 500', file=sys.stderr)
    sys.exit(1)
if '/commits?' in path:
    reply = [{'sha': sha} for sha in state['commits']]
elif '/actions/workflows/ci.yml/runs?' in path:
    reply = {'workflow_runs': [{'head_sha': sha} for sha in state['green']]}
elif '/releases?' in path:
    reply = state['releases'] if '--paginate' in args else state['releases'][:100]
elif '/contents/.github?ref=' in path:
    reply = [{'name': 'ISSUE_TEMPLATE', 'sha': 'other'},
             {'name': 'workflows', 'sha': state['trees'][path.split('ref=')[1]]}]
else:
    print('fake gh: unknown query ' + path, file=sys.stderr)
    sys.exit(2)
print(json.dumps(reply))
""" % sys.executable


class Run(NamedTuple):
    code: int
    out: dict
    summary: str
    calls: list
    stdout: str
    stderr: str


def release(tag, published_at='2026-11-01T00:00:00Z', draft=False):
    return {'id': 1, 'tag_name': tag, 'draft': draft, 'published_at': published_at}


def nightly(sha, published_at='2026-11-01T00:00:00Z'):
    return release('nightly-2026-11-01-' + sha[:7], published_at)


def state(commits, green=(), releases=(), trees=None, fail_on=None):
    ids = [*commits, HEAD]
    return {'commits': list(commits), 'green': list(green), 'releases': list(releases),
            'trees': trees or {sha: 't1' for sha in ['main', *ids]}, 'fail_on': fail_on}


class Pick(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.dir)
        bin_dir = os.path.join(self.dir, 'bin')
        os.mkdir(bin_dir)
        gh = os.path.join(bin_dir, 'gh')
        with open(gh, 'w', encoding='utf-8') as f:
            f.write(FAKE_GH)
        os.chmod(gh, os.stat(gh).st_mode | stat.S_IXUSR)
        self.bin = bin_dir

    def run_pick(self, fake, ref='refs/heads/main', head=HEAD):
        path = os.path.join(self.dir, 'state.json')
        with open(path, 'w', encoding='utf-8') as f:
            json.dump(fake, f)
        output, summary = (os.path.join(self.dir, n) for n in ('output', 'summary'))
        env = dict(os.environ, PATH=f'{self.bin}:{os.environ["PATH"]}', FAKE_STATE=path,
                   GITHUB_REPOSITORY='o/r', GITHUB_REF=ref, HEAD_SHA=head, GH_TOKEN='t',
                   GITHUB_OUTPUT=output, GITHUB_STEP_SUMMARY=summary)
        run = subprocess.run([sys.executable, SCRIPT, 'pick'], cwd=self.dir, capture_output=True,
                             text=True, env=env, timeout=30)
        return Run(run.returncode, self.read_outputs(output), self.read(summary),
                   [json.loads(line) for line in self.read(path + '.calls').splitlines()],
                   run.stdout, run.stderr)

    @staticmethod
    def read(path):
        if not os.path.exists(path):
            return ''
        with open(path, encoding='utf-8') as f:
            return f.read()

    def read_outputs(self, path):
        return dict(line.split('=', 1) for line in self.read(path).splitlines())

    def assert_answer(self, run, build, publish, sha=None):
        self.assertEqual(run.code, 0, run.stderr)
        self.assertEqual((run.out.get('build'), run.out.get('publish')), (build, publish), run)
        if sha is not None:
            self.assertEqual(run.out.get('sha'), sha)

    def test_pick_should_take_the_newest_commit_with_a_green_run(self):
        run = self.run_pick(state([A, B, C], [A, C], [nightly(C)]))
        self.assert_answer(run, 'true', 'true', A)

    def test_pick_should_take_the_commit_before_a_red_head(self):
        run = self.run_pick(state([A, B, C], [B], [nightly(C)]))
        self.assert_answer(run, 'true', 'true', B)

    def test_pick_should_not_build_when_the_release_has_the_newest_green_commit(self):
        run = self.run_pick(state([A, B], [B], [nightly(B)]))
        self.assert_answer(run, 'false', 'false')
        self.assertIn(NO_NEW, run.summary)

    def test_pick_should_not_go_back_after_a_ci_rerun_on_an_old_commit(self):
        run = self.run_pick(state([A, B, C], [C, B], [nightly(B)]))
        self.assert_answer(run, 'false', 'false')
        self.assertIn(NO_NEW, run.summary)

    def test_pick_should_build_when_no_nightly_build_exists(self):
        run = self.run_pick(state([A], [A]))
        self.assert_answer(run, 'true', 'true', A)

    def test_pick_should_not_build_when_the_workflow_files_differ(self):
        fake = state([A, B], [B], trees={'main': 't1', A: 't1', B: 't2', HEAD: 't1'})
        run = self.run_pick(fake)
        self.assert_answer(run, 'false', 'false')
        self.assertIn(DIFFER, run.summary)
        self.assertNotIn(NO_NEW, run.summary)

    def test_pick_should_not_build_when_no_commit_is_green(self):
        run = self.run_pick(state([A, B]))
        self.assert_answer(run, 'false', 'false')
        self.assertIn(NO_NEW, run.summary)

    def test_pick_should_build_the_head_and_not_publish_in_a_dry_run(self):
        for ref in ('refs/pull/1/merge', 'refs/heads/x'):
            with self.subTest(ref):
                run = self.run_pick(state([A, B], [A], [nightly(B)]), ref=ref)
                self.assert_answer(run, 'true', 'false', HEAD)
                self.assertIn('main would: ', run.stdout)
                self.assertIn(A, run.stdout)
                self.assertIn('main would: ', run.summary)
                self.assertTrue(any('commits?sha=main' in call[-1] for call in run.calls), run.calls)

    def test_pick_should_ignore_other_releases_and_drafts(self):
        draft = release('nightly-2026-11-02-' + A[:7], published_at=None, draft=True)
        run = self.run_pick(state([A, B], [A], [draft, nightly(B)]))
        self.assert_answer(run, 'true', 'true', A)
        other = release('v1.0.0', published_at='2026-11-09T00:00:00Z')
        run = self.run_pick(state([A, B], [B], [nightly(B), other]))
        self.assert_answer(run, 'false', 'false')

    def test_pick_should_take_the_largest_published_at(self):
        releases = [nightly(C, '2026-11-01T00:00:00Z'), nightly(B, '2026-11-02T00:00:00Z')]
        run = self.run_pick(state([A, B, C], [B], releases))
        self.assert_answer(run, 'false', 'false')

    def test_pick_should_send_no_write(self):
        for ref in ('refs/heads/main', 'refs/heads/x'):
            with self.subTest(ref):
                run = self.run_pick(state([A, B], [A], [nightly(B)]), ref=ref)
                self.assertTrue(run.calls)
                for call in run.calls:
                    self.assertEqual(call[0], 'api')
                    self.assertNotIn('--method', call)

    def test_pick_should_fail_when_a_query_fails(self):
        for query in ('commits', 'runs', 'releases', 'contents'):
            with self.subTest(query):
                run = self.run_pick(state([A], [A], fail_on=query))
                self.assertNotEqual(run.code, 0)


if __name__ == '__main__':
    unittest.main()
