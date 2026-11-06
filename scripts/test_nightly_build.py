"""Tests for nightly_build.py: python3 -m unittest discover -s scripts -p 'test_nightly_build.py'.

Each test runs the script with a fake `gh` on PATH, so nothing reaches GitHub. The fake answers
each query from a JSON state file and records each call. It supplies replies; it never computes
which commit to take.
"""
import itertools
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from typing import NamedTuple
from unittest import mock

import nightly_build

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'nightly_build.py')
# A is the newest commit. Each is a distinct 40-character string.
A, B, C = ('a' * 40, 'b' * 40, 'c' * 40)
HEAD = 'd' * 40
NO_NEW = 'no new green commit'
DIFFER = 'the newest green commit has other workflow files than `main`'
NEWER = 'the commit is not newer than the newest nightly build'
TITLE = 'Nightly build failure'

FAKE_GH = """#!%s
import io, json, os, sys, zipfile
from datetime import datetime, timezone
from urllib.parse import parse_qs
args = sys.argv[1:]
state_path = os.environ['FAKE_STATE']
with open(state_path, encoding='utf-8') as f:
    state = json.load(f)
with open(state_path + '.calls', 'a', encoding='utf-8') as f:
    f.write(json.dumps(args) + '\\n')


def save():
    with open(state_path, 'w', encoding='utf-8') as f:
        json.dump(state, f)


def now():
    return datetime.now(timezone.utc).isoformat(timespec='seconds')


def draft_of(tag):
    found = [r for r in state['releases'] if r['tag_name'] == tag and r['draft']]
    if not found:
        print('gh: release not found: ' + tag, file=sys.stderr)
        sys.exit(1)
    return found[-1]


def release_of(tag):
    found = [r for r in state['releases'] if r['tag_name'] == tag]
    if not found:
        print('gh: release not found: ' + tag, file=sys.stderr)
        sys.exit(1)
    return found[-1]


is_release = args[0] == 'release'
path = '' if is_release else next(a for a in args if a.startswith('repos/'))
key = ' '.join(args[:2]) if is_release else path
partial = state['fail_after'] and key == 'release upload'
if state['fail_on'] and state['fail_on'] in key and not partial:
    state['fail_on'] = None  # one call fails
    save()
    print('gh: HTTP 500', file=sys.stderr)
    sys.exit(1)
if is_release:
    command, tag = args[1], args[2]
    rest = [a for i, a in enumerate(args[3:]) if not a.startswith('--') and args[2 + i] != '--repo']
    if command == 'create':
        state['next_id'] += 1
        state['releases'].append({'id': state['next_id'], 'tag_name': tag, 'draft': True,
                                  'published_at': None, 'assets': {},
                                  'target': args[args.index('--target') + 1]})
    elif command == 'upload':
        release = release_of(tag)  # gh also uploads to a published release
        for name in rest:  # gh compares every name before it uploads any
            asset = os.path.basename(name)
            if asset in release['assets'] and '--clobber' not in args:
                print('gh: asset under the same name already exists: ' + asset, file=sys.stderr)
                sys.exit(1)
        for count, name in enumerate(rest):
            if state['fail_on'] and partial and count == state['fail_after']:
                state['fail_on'] = None  # one call fails, after some assets are stored
                save()
                print('gh: HTTP 500', file=sys.stderr)
                sys.exit(1)
            with open(name, encoding='utf-8') as f:
                release['assets'][os.path.basename(name)] = f.read()
            save()
    elif command == 'edit':
        release = draft_of(tag)
        release['draft'] = '--draft=false' not in args
        release['published_at'] = now()
        release['latest'] = '--latest' in args
    elif command == 'delete':
        release_of(tag)
        state['releases'] = [r for r in state['releases'] if r['tag_name'] != tag]
        if '--cleanup-tag' in args:
            state['tags_deleted'].append(tag)
    save()
    sys.exit(0)
if '--method' in args and 'DELETE' in args:
    gone = int(path.rsplit('/', 1)[1])
    if gone not in [r['id'] for r in state['releases']]:
        print('gh: HTTP 404', file=sys.stderr)
        sys.exit(1)
    state['releases'] = [r for r in state['releases'] if r['id'] != gone]
    save()
    sys.exit(0)  # the reply is empty
if '--input' in args:
    body = json.load(sys.stdin)
    state['posted'].append({'path': path, 'body': body})
    if path.endswith('/issues'):
        state['issues'].append({'number': 100 + len(state['posted']), 'title': body['title'],
                                'state': 'open', 'pull_request': None,
                                'labels': [{'name': name} for name in body['labels']]})
    save()
    print('{}')
    sys.exit(0)
if '/commits?' in path:
    reply = [{'sha': sha} for sha in state['commits']]
elif '/actions/workflows/ci.yml/runs?' in path:
    reply = {'workflow_runs': [{'head_sha': sha} for sha in state['green']]}
elif '/actions/runs/' in path and '/artifacts?' in path:
    listed = [{k: a[k] for k in ('id', 'name', 'created_at')} for a in state['artifacts']]
    pages = [listed[i:i + 100] for i in range(0, len(listed), 100)] or [[]]
    pages = [{'total_count': len(listed), 'artifacts': page} for page in pages]
    if '--paginate' not in args:
        pages = pages[:1]
    if '--slurp' in args:
        reply = pages
    else:
        for page in pages:
            print(json.dumps(page))
        sys.exit(0)
elif '/actions/artifacts/' in path and path.endswith('/zip'):
    wanted = int(path.split('/')[-2])
    files = next(a['files'] for a in state['artifacts'] if a['id'] == wanted)
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, 'w') as z:
        for name, text in files.items():
            z.writestr(name, text)
    sys.stdout.buffer.write(buffer.getvalue())
    sys.exit(0)
elif '/releases?' in path:
    reply = state['releases'] if '--paginate' in args else state['releases'][:100]
elif '/issues?' in path:
    query = parse_qs(path.split('?')[1])
    want = query.get('state', ['open'])[0]  # the API's default is open
    reply = [i for i in state['issues'] if want in (i['state'], 'all')
             and ('labels' not in query
                  or query['labels'][0] in [label['name'] for label in i['labels']])]
    if '--paginate' not in args:
        reply = reply[:100]
elif '/contents/.github?ref=' in path:
    ref = path.split('ref=')[1]
    if ref.endswith('^'):
        ref = state['parents'][ref[:-1]]
    reply = [{'name': 'ISSUE_TEMPLATE', 'sha': 'other'},
             {'name': 'workflows', 'sha': state['trees'][ref]}]
elif '/commits/' in path:
    sha = path.rsplit('/', 1)[1]
    reply = {'sha': sha, 'parents': [{'sha': state['parents'][sha]}]}
elif path.endswith('/empty'):
    sys.exit(0)
else:
    print('fake gh: unknown query ' + path, file=sys.stderr)
    sys.exit(2)
print(json.dumps(reply))
""" % sys.executable


def is_write_flag(arg):
    """A `gh api` flag that sends a method or a body, alone or with its value attached."""
    if arg.startswith('--'):
        return arg.split('=')[0] in ('--method', '--field', '--raw-field', '--input')
    return arg.startswith('-') and len(arg) > 1 and arg[1] in 'XfF'


class Run(NamedTuple):
    code: int
    out: dict
    summary: str
    calls: list
    stdout: str
    stderr: str
    state: dict


_ids = itertools.count(1)


def release(tag, published_at='2026-11-01T00:00:00Z', draft=False):
    return {'id': next(_ids), 'tag_name': tag, 'draft': draft, 'published_at': published_at}


def nightly(sha, published_at='2026-11-01T00:00:00Z'):
    return release('nightly-2026-11-01-' + sha[:7], published_at)


def state(commits, green=(), releases=(), trees=None, parents=None, fail_on=None, artifacts=(),
          issues=(), fail_after=0):
    parents = parents or dict(zip(commits, [*commits[1:], 'e' * 40]))
    ids = ['main', HEAD, *commits, *parents.values()]
    return {'commits': list(commits), 'green': list(green), 'releases': list(releases),
            'trees': trees or {sha: 't1' for sha in ids}, 'parents': parents, 'fail_on': fail_on,
            'fail_after': fail_after,
            'artifacts': list(artifacts), 'issues': list(issues), 'posted': [],
            'tags_deleted': [], 'next_id': 10**6}


class Script(unittest.TestCase):
    """Runs `nightly_build.py <command>` against the fake `gh`."""

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

    def run_script(self, args, fake, **env):
        path = os.path.join(self.dir, 'state.json')
        with open(path, 'w', encoding='utf-8') as f:
            json.dump(fake, f)
        output, summary = (os.path.join(self.dir, n) for n in ('output', 'summary'))
        for stale in (output, summary, path + '.calls'):
            if os.path.exists(stale):
                os.remove(stale)
        env = dict(os.environ, PATH=f'{self.bin}:{os.environ["PATH"]}', FAKE_STATE=path,
                   GITHUB_REPOSITORY='o/r', GH_TOKEN='t', GITHUB_OUTPUT=output,
                   GITHUB_STEP_SUMMARY=summary, **env)
        run = subprocess.run([sys.executable, SCRIPT, *args], cwd=self.dir, capture_output=True,
                             text=True, env=env, timeout=60)
        return Run(run.returncode, self.read_outputs(output), self.read(summary),
                   [json.loads(line) for line in self.read(path + '.calls').splitlines()],
                   run.stdout, run.stderr, json.loads(self.read(path)))

    @staticmethod
    def read(path):
        if not os.path.exists(path):
            return ''
        with open(path, encoding='utf-8') as f:
            return f.read()

    def read_outputs(self, path):
        return dict(line.split('=', 1) for line in self.read(path).splitlines())


class Pick(Script):
    def run_pick(self, fake, ref='refs/heads/main', head=HEAD):
        return self.run_script(['pick'], fake, GITHUB_REF=ref, HEAD_SHA=head)

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
        # B and its parent C have equal trees: only a compare with `main` sees the difference.
        fake = state([A, B], [B], parents={A: B, B: C},
                     trees={'main': 't1', A: 't1', B: 't2', C: 't2', HEAD: 't1'})
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
                queried = any('commits?sha=main' in call[-1] for call in run.calls)
                self.assertTrue(queried, run.calls)

    def test_pick_should_fail_on_a_nightly_tag_without_a_short_sha(self):
        for tag in ('nightly-test', 'nightly-a', 'nightly-2026-11-01-',
                    'nightly-2026-11-01-ABCDEF0', 'nightly-2026-11-01-abcdef01'):
            with self.subTest(tag):
                # Each of these tags can hide the release's commit, or stop the walk at a commit.
                run = self.run_pick(state([A, B], [A], [release(tag)]))
                self.assertEqual(run.code, 1, run.stderr)
                self.assertIn(tag, run.stderr)

    def test_pick_should_ignore_other_releases_and_drafts(self):
        tag = 'nightly-2026-11-02-' + A[:7]
        draft = release(tag, published_at=None, draft=True)
        newer_draft = release(tag, published_at='2026-11-09T00:00:00Z', draft=True)
        other = release('v1.0.0', published_at='2026-11-09T00:00:00Z')
        for name, fake, build, sha in (
                ('draft without published_at', state([A, B], [A], [draft, nightly(B)]), 'true', A),
                ('draft with a newer published_at',
                 state([A, B], [A], [newer_draft, nightly(B)]), 'true', A),
                ('other release', state([A, B], [B], [nightly(B), other]), 'false', '')):
            with self.subTest(name):
                self.assert_answer(self.run_pick(fake), build, build, sha)

    def test_pick_should_take_the_largest_published_at(self):
        releases = [nightly(C, '2026-11-01T00:00:00Z'), nightly(B, '2026-11-02T00:00:00Z')]
        run = self.run_pick(state([A, B, C], [B], releases))
        self.assert_answer(run, 'false', 'false')

    def test_pick_should_read_every_page_of_releases(self):
        older = [release(f'nightly-2026-09-01-{i:07x}', f'2026-09-01T00:00:{i % 60:02d}Z')
                 for i in range(100)]
        run = self.run_pick(state([A, B], [B], [*older, nightly(B)]))
        self.assert_answer(run, 'false', 'false')

    def test_gh_should_return_none_for_an_empty_reply(self):
        self.run_pick(state([A]))
        env = dict(PATH=f'{self.bin}:{os.environ["PATH"]}',
                   FAKE_STATE=os.path.join(self.dir, 'state.json'))
        with mock.patch.dict(os.environ, env):
            self.assertIsNone(nightly_build.gh('repos/o/r/empty'))

    def test_pick_should_send_no_write(self):
        for ref in ('refs/heads/main', 'refs/heads/x'):
            with self.subTest(ref):
                run = self.run_pick(state([A, B], [A], [nightly(B)]), ref=ref)
                self.assertTrue(run.calls)
                self.assertEqual(run.code, 0, run.stderr)
                for call in run.calls:
                    self.assertEqual(call[0], 'api')
                    self.assertEqual([arg for arg in call if is_write_flag(arg)], [], call)

    def test_pick_should_fail_when_a_query_fails(self):
        for query in ('commits', 'runs', 'releases', 'contents'):
            with self.subTest(query):
                run = self.run_pick(state([A], [A], fail_on=query))
                self.assertNotEqual(run.code, 0)
                self.assertIn('HTTP 500', run.stderr)
                self.assertNotIn('Traceback', run.stderr)
                self.assertEqual(run.out, {})


ASSETS = ['sisu-x86_64-linux.tar.gz', 'sisu-aarch64-linux.tar.gz', 'sisu-aarch64-macos.tar.gz',
          'install.sh', 'orders.sisu', 'SHA256SUMS']
# The artifacts of the workflow: one per tarball, and `release-files` holds the other three.
ARTIFACTS = {'tarball-x86_64-linux': ASSETS[:1], 'tarball-aarch64-linux': ASSETS[1:2],
             'tarball-aarch64-macos': ASSETS[2:3], 'release-files': ASSETS[3:]}
UTC = timezone.utc


def artifact(id_, name, text, created_at='2026-11-06T01:00:00Z', skip=()):
    files = {f: f'{text}:{f}' for f in ARTIFACTS[name] if f not in skip}
    return {'id': id_, 'name': name, 'created_at': created_at, 'files': files}


def all_artifacts(text='v', first_id=1, skip=()):
    return [artifact(first_id + i, name, text, skip=skip) for i, name in enumerate(ARTIFACTS)]


def numbered(i):
    """The i-th published nightly build; a larger i is newer."""
    when = datetime(2026, 9, 1, tzinfo=UTC) + timedelta(hours=i)
    return release(f'nightly-2026-09-01-{i:07x}', when.strftime('%Y-%m-%dT%H:%M:%SZ'))


def release_tags(fake_state, draft=False):
    return [r['tag_name'] for r in fake_state['releases'] if r['draft'] == draft]


def first(calls, *words):
    """The index of the first call that has every word as one of its arguments."""
    for i, call in enumerate(calls):
        if all(w in call for w in words):
            return i
    raise AssertionError(f'no call has {words}: {calls}')


def with_arg(calls, text):
    return [c for c in calls if any(text in a for a in c)]


def sent(calls, *words):
    return [c for c in calls if c[:len(words)] == list(words)]


class Publish(Script):
    def run_publish(self, fake, sha=A, **env):
        return self.run_script(['publish', '--sha', sha], fake, GITHUB_RUN_ID='42', **env)

    def fake(self, releases=(), artifacts=None, **extra):
        artifacts = all_artifacts() if artifacts is None else artifacts
        return state([A, B], [A], releases, artifacts=artifacts, **extra)

    def assert_stopped(self, run, reason):
        self.assertEqual(run.code, 0, run.stderr)
        self.assertEqual(run.out.get('published'), 'false')
        self.assertIn(reason, run.summary)
        self.assertEqual([c for c in run.calls if 'artifacts' in ' '.join(c) or c[0] == 'release'],
                         [])

    def test_publish_should_send_the_calls_in_order(self):
        for name, releases in (('older nightly build', [nightly(B)]), ('no nightly build', [])):
            with self.subTest(name):
                before = datetime.now(UTC).strftime('%Y-%m-%d')
                run = self.run_publish(self.fake(releases))
                after = datetime.now(UTC).strftime('%Y-%m-%d')
                self.assertEqual(run.code, 0, run.stderr)
                calls = run.calls
                listing = first(calls, 'repos/o/r/actions/runs/42/artifacts?per_page=100')
                create, upload, edit = (first(calls, 'release', c) for c in
                                        ('create', 'upload', 'edit'))
                self.assertLess(listing, create)
                self.assertLess(create, upload)
                self.assertLess(upload, edit)
                tag = calls[create][2]
                self.assertIn(tag, {f'nightly-{d}-{A[:7]}' for d in (before, after)})
                self.assertIn('--draft', calls[create])
                self.assertEqual(calls[create][calls[create].index('--target') + 1], A)
                self.assertIn('--latest=false', calls[create])
                self.assertIn(A, calls[create][calls[create].index('--notes') + 1])
                self.assertEqual(calls[upload][2], tag)
                files = [a for a in calls[upload][3:] if not a.startswith('--') and a != 'o/r']
                self.assertEqual(sorted(os.path.basename(f) for f in files), sorted(ASSETS))
                self.assertNotIn('--clobber', calls[upload])
                self.assertEqual(calls[edit][2], tag)
                self.assertIn('--draft=false', calls[edit])
                self.assertIn('--latest', calls[edit])
                self.assertEqual(with_arg(calls, '--prerelease'), [])
                self.assertEqual(run.out.get('published'), 'true')

    def test_publish_should_delete_a_draft_first(self):
        for name, others in (('no other release', []), ('100 other releases',
                                                         [release(f'v0.{i}') for i in range(100)])):
            with self.subTest(name):
                draft = release('nightly-2026-11-02-1234567', None, draft=True)
                other = release('other', None, draft=True)
                run = self.run_publish(self.fake([*others, draft, other]))
                self.assertEqual(run.code, 0, run.stderr)
                deletes = [i for i, c in enumerate(run.calls) if 'DELETE' in c]
                self.assertEqual([run.calls[i][-1] for i in deletes],
                                 [f'repos/o/r/releases/{draft["id"]}'])
                self.assertLess(deletes[0], first(run.calls, 'release', 'create'))
                self.assertIn('other', release_tags(run.state, draft=True))
                self.assertNotIn(draft['tag_name'], release_tags(run.state, draft=True))

    def test_failed_upload_should_not_publish_the_draft(self):
        for name, stored in (('no asset stored', 0), ('two assets stored', 2)):
            with self.subTest(name):
                fake = self.fake([nightly(B)], fail_on='release upload', fail_after=stored)
                run = self.run_publish(fake)
                self.assertNotEqual(run.code, 0)
                self.assertEqual(sent(run.calls, 'release', 'edit'), [])
                self.assertNotEqual(run.out.get('published'), 'true')
                self.assertEqual(release_tags(run.state), release_tags(fake))
                [draft] = [r for r in run.state['releases'] if r['draft']]
                self.assertEqual(len(draft['assets']), stored)

    def test_failed_create_should_not_publish(self):
        run = self.run_publish(self.fake([nightly(B)], fail_on='release create'))
        self.assertNotEqual(run.code, 0)
        self.assertEqual(len(sent(run.calls, 'release', 'create')), 1)
        self.assertEqual(sent(run.calls, 'release', 'upload'), [])
        self.assertEqual(sent(run.calls, 'release', 'edit'), [])

    def test_next_publish_should_clear_a_failed_upload(self):
        failed = self.run_publish(self.fake([nightly(B)], fail_on='release upload', fail_after=2))
        self.assertNotEqual(failed.code, 0)
        drafts = [r for r in failed.state['releases'] if r['draft']]
        self.assertEqual(len(drafts), 1)
        draft = drafts[0]
        self.assertEqual(len(draft['assets']), 2)
        run = self.run_publish(dict(failed.state, fail_on=None, fail_after=0))
        self.assertEqual(run.code, 0, run.stderr)
        deleted = [c[-1] for c in run.calls if 'DELETE' in c]
        self.assertIn(f'repos/o/r/releases/{draft["id"]}', deleted)
        self.assertEqual(run.out.get('published'), 'true')
        self.assertEqual(release_tags(run.state, draft=True), [])
        self.assertIn(draft['tag_name'], release_tags(run.state))
        assets = [r['assets'] for r in run.state['releases'] if r['tag_name'] == draft['tag_name']]
        self.assertEqual(assets, [{f: f'v:{f}' for f in ASSETS}])

    def test_publish_should_upload_the_largest_id_of_each_name(self):
        def pair(old_first, early_new=False):
            old = [artifact(i, n, 'old', created_at='2026-11-06T09:00:00Z')
                   for i, n in enumerate(ARTIFACTS, 1)]
            new = [artifact(i, n, 'new', created_at='2026-11-06T01:00:00Z' if early_new
                            else '2026-11-06T10:00:00Z') for i, n in enumerate(ARTIFACTS, 1000)]
            return old + new if old_first else new + old

        many_old = [artifact(i, n, 'old') for i, n in enumerate(list(ARTIFACTS) * 25, 1)]
        rows = (('smaller id first', pair(True)), ('larger id first', pair(False)),
                ('100 entries first', many_old + all_artifacts('new', 1000)),
                ('larger id, earlier created_at', pair(True, early_new=True)))
        for name, artifacts in rows:
            with self.subTest(name):
                run = self.run_publish(self.fake([], artifacts))
                self.assertEqual(run.code, 0, run.stderr)
                published = [r['assets'] for r in run.state['releases'] if not r['draft']]
                self.assertEqual(published, [{f: f'new:{f}' for f in ASSETS}])

    def test_missing_artifact_should_fail_before_the_draft(self):
        draft = release('nightly-2026-11-02-1234567', None, draft=True)
        run = self.run_publish(self.fake([draft], all_artifacts(skip=['SHA256SUMS'])))
        self.assertNotEqual(run.code, 0)
        self.assertIn('SHA256SUMS', run.stderr)
        self.assertEqual(with_arg(run.calls, 'DELETE'), [])
        self.assertEqual(sent(run.calls, 'release'), [])

    def test_publish_should_stop_on_two_artifacts_with_one_file_name(self):
        def extra(files):
            return {'id': 50, 'name': 'extra', 'created_at': '2026-11-06T01:00:00Z', 'files': files}

        draft = release('nightly-2026-11-02-1234567', None, draft=True)
        rows = (('extra listed last', [*all_artifacts(), extra({'install.sh': 'other'})],
                 ['release-files', 'extra']),
                ('extra listed first', [extra({'install.sh': 'other'}), *all_artifacts()],
                 ['release-files', 'extra']),
                ('one artifact, two folders',
                 [extra({'install.sh': 'a', 'sub/install.sh': 'b'}),
                  *all_artifacts(skip=ASSETS[3:4])], ['extra']))
        for name, artifacts, named in rows:
            with self.subTest(name):
                run = self.run_publish(self.fake([draft], artifacts))
                self.assertNotEqual(run.code, 0)
                self.assertIn('install.sh', run.stderr)
                for artifact_name in named:
                    self.assertIn(artifact_name, run.stderr)
                self.assertEqual(with_arg(run.calls, 'DELETE'), [])
                self.assertEqual(sent(run.calls, 'release'), [])

    def test_publish_should_stay_in_its_temp_directory(self):
        tmp = os.path.join(self.dir, 'tmp')
        os.mkdir(tmp)
        names = ('..', '../../up', 'a/b')
        evil = [{'id': 60 + i, 'name': n, 'created_at': '2026-11-06T01:00:00Z',
                 'files': {'evil.txt': 'x', '../zipslip.txt': 'y'}} for i, n in enumerate(names)]
        run = self.run_publish(self.fake([], [*all_artifacts(), *evil]), TMPDIR=tmp)
        self.assertEqual(run.code, 0, run.stderr)
        self.assertEqual(os.listdir(tmp), [])
        outside = {'bin', 'tmp', 'state.json', 'state.json.calls', 'output', 'summary'}
        self.assertLessEqual(set(os.listdir(self.dir)), outside)

    def test_publish_should_stop_when_a_newer_nightly_build_exists(self):
        run = self.run_publish(state([A, B], [A, B], [nightly(A)], artifacts=all_artifacts()), B)
        self.assert_stopped(run, NEWER)

    def test_publish_should_stop_when_the_commit_is_the_newest_nightly_build(self):
        run = self.run_publish(state([A, B], [A, B], [nightly(B)], artifacts=all_artifacts()), B)
        self.assert_stopped(run, NEWER)

    def test_publish_should_stop_when_the_sha_left_the_listing(self):
        run = self.run_publish(state([A, B], [A, B], [nightly(B)], artifacts=all_artifacts()), C)
        self.assert_stopped(run, NEWER)

    def test_publish_should_stop_when_the_workflow_files_changed(self):
        trees = {'main': 't1', A: 't2'}
        run = self.run_publish(state([A, B], [A], [nightly(B)], trees=trees,
                                     artifacts=all_artifacts()))
        self.assert_stopped(run, DIFFER)

    def test_publish_and_prune_should_leave_14_after_a_run(self):
        built = [numbered(i) for i in range(15)]
        draft = release('nightly-2026-11-02-1234567', None, draft=True)
        newest = '0' * 6 + 'e' + '0' * 33  # the commit of `numbered(14)`
        fake = state([A, newest], [A], [*built, draft], artifacts=all_artifacts())
        published = self.run_publish(fake)
        self.assertEqual(published.code, 0, published.stderr)
        run = self.run_script(['prune'], published.state)
        self.assertEqual(run.code, 0, run.stderr)
        left = release_tags(run.state)
        self.assertEqual(len(left), 14)
        self.assertIn(published.calls[first(published.calls, 'release', 'create')][2], left)
        self.assertEqual(release_tags(run.state, draft=True), [])
        old = [r['tag_name'] for r in built[:2]]
        for tag in old:
            self.assertNotIn(tag, left)
            self.assertIn(tag, run.state['tags_deleted'])
        self.assertEqual(len(run.state['tags_deleted']), len(old))


class Prune(Script):
    def run_prune(self, fake):
        return self.run_script(['prune'], fake)

    @staticmethod
    def deleted(run):
        return [c[2] for c in sent(run.calls, 'release', 'delete')]

    def test_prune_should_keep_the_14_newest_and_touch_nothing_else(self):
        built = [numbered(i) for i in range(16)]
        draft = release('nightly-2026-12-01-ddddddd', None, draft=True)
        fake = state([A], releases=[*built[::2], release('v1.0.0'), draft, *built[1::2]])
        run = self.run_prune(fake)
        self.assertEqual(run.code, 0, run.stderr)
        self.assertEqual(sorted(self.deleted(run)), [built[0]['tag_name'], built[1]['tag_name']])
        for call in sent(run.calls, 'release', 'delete'):
            self.assertIn('--cleanup-tag', call)
        self.assertEqual(with_arg(run.calls, 'v1.0.0') + with_arg(run.calls, 'ddddddd'), [])
        self.assertEqual(len(release_tags(run.state)), 15)

    def test_prune_should_delete_nothing_at_14(self):
        run = self.run_prune(state([A], releases=[numbered(i) for i in range(14)]))
        self.assertEqual(run.code, 0, run.stderr)
        self.assertTrue(with_arg(run.calls, 'releases?'))
        self.assertEqual(self.deleted(run), [])

    def test_prune_should_read_every_page(self):
        built = [numbered(i) for i in range(102)]
        run = self.run_prune(state([A], releases=built[::2] + built[1::2]))
        self.assertEqual(run.code, 0, run.stderr)
        self.assertEqual(sorted(self.deleted(run)), sorted(r['tag_name'] for r in built[:88]))
        self.assertEqual(len(release_tags(run.state)), 14)

    def test_prune_should_try_every_delete_and_then_fail(self):
        run = self.run_prune(state([A], releases=[numbered(i) for i in range(16)],
                                   fail_on='release delete'))
        self.assertNotEqual(run.code, 0)
        self.assertEqual(len(self.deleted(run)), 2)


def issue(number, title=TITLE, state='open', labels=('nightly-build',), pull_request=None):
    return {'number': number, 'title': title, 'state': state, 'pull_request': pull_request,
            'labels': [{'name': name} for name in labels]}


class Report(Script):
    URL = 'https://example.test/o/r/actions/runs/7'

    def run_report(self, results, issues=()):
        needs = json.dumps({name: {'result': result, 'outputs': {}}
                            for name, result in results.items()})
        return self.run_script(['report'], state([A], issues=issues), NEEDS=needs, RUN_URL=self.URL)

    def posted(self, run):
        self.assertEqual(run.code, 0, run.stderr)
        self.assertEqual(len(run.state['posted']), 1, run.state['posted'])
        return run.state['posted'][0]

    def test_report_should_open_one_issue(self):
        post = self.posted(self.run_report({'pick': 'success', 'build': 'failure'}))
        self.assertEqual(post['path'], 'repos/o/r/issues')
        self.assertEqual(post['body']['title'], TITLE)
        self.assertEqual(post['body']['labels'], ['needs-triage', 'bug', 'nightly-build'])
        self.assertIn(Report.URL, post['body']['body'])
        self.assertIn('build: failure', post['body']['body'])

    def test_report_should_comment_on_an_open_issue(self):
        issues = [issue(9)]
        post = self.posted(self.run_report({'build': 'failure'}, issues))
        self.assertEqual(post['path'], 'repos/o/r/issues/9/comments')
        self.assertIn('build: failure', post['body']['body'])

    def test_report_should_report_a_cancelled_job(self):
        post = self.posted(self.run_report({'clean-machine': 'cancelled'}))
        self.assertIn('clean-machine: cancelled', post['body']['body'])

    def test_report_should_name_the_live_check(self):
        post = self.posted(self.run_report({'live-check': 'failure', 'publish': 'success'}))
        self.assertIn('live-check', post['body']['body'])
        self.assertNotIn('publish', post['body']['body'])

    def test_report_should_not_name_a_skipped_job(self):
        results = {'build': 'failure', 'checksums': 'skipped', 'prune': 'skipped'}
        body = self.posted(self.run_report(results))['body']['body']
        self.assertIn('build: failure', body)
        self.assertNotIn('checksums', body)
        self.assertNotIn('prune', body)

    def test_report_should_ignore_a_pull_request_with_the_title(self):
        issues = [issue(3, pull_request={'url': 'x'})]
        self.assertEqual(self.posted(self.run_report({'build': 'failure'}, issues))['path'],
                         'repos/o/r/issues')

    def test_report_should_read_every_page_of_issues(self):
        issues = [issue(n, 'Other') for n in range(100)] + [issue(500)]
        post = self.posted(self.run_report({'build': 'failure'}, issues))
        self.assertEqual(post['path'], 'repos/o/r/issues/500/comments')

    def test_report_should_not_comment_on_a_closed_issue(self):
        issues = [issue(6, state='closed')]
        self.assertEqual(self.posted(self.run_report({'build': 'failure'}, issues))['path'],
                         'repos/o/r/issues')

    def test_report_should_not_comment_on_an_issue_without_the_label(self):
        issues = [issue(7, labels=())]
        self.assertEqual(self.posted(self.run_report({'build': 'failure'}, issues))['path'],
                         'repos/o/r/issues')

    def test_report_should_ignore_another_title(self):
        issues = [issue(4, 'Other')]
        self.assertEqual(self.posted(self.run_report({'build': 'failure'}, issues))['path'],
                         'repos/o/r/issues')


if __name__ == '__main__':
    unittest.main()
