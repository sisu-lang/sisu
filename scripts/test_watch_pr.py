"""Unit tests for watch_pr.py: python3 -m unittest discover -s scripts -p 'test_*.py'.

Each test runs the script with a fake `gh` on PATH that answers from a table.
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

SCRIPT = Path(__file__).resolve().parent / 'watch_pr.py'
FAKE_GH = f"""#!{sys.executable}
import json, os, sys
table = json.load(open(os.environ['GH_TABLE']))
print(json.dumps(table[sys.argv[2].split('?')[0]]))
"""
P = 'repos/sisu-lang/sisu'


def comment(id, login, body, updated='t1'):
    return {'id': id, 'user': {'login': login}, 'body': body, 'updated_at': updated,
            'path': 'a.rs', 'line': 3, 'html_url': f'u{id}'}


class WatchPr(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.dir)
        gh = self.dir / 'gh'
        gh.write_text(FAKE_GH)
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
        self.state = self.dir / 'state.json'
        self.table = {
            'user': {'login': 'me'},
            f'{P}/pulls/7': {'head': {'sha': 'abcdef0123'}},
            f'{P}/pulls/7/reviews': [],
            f'{P}/pulls/7/comments': [],
            f'{P}/issues/7/comments': [],
            f'{P}/commits/abcdef0123/check-runs': {'check_runs': [{'status': 'in_progress'}]},
            f'{P}/commits/abcdef0123/check-suites': {'check_suites': [
                {'app': {'slug': 'github-actions'}, 'status': 'completed'},
                {'app': {'slug': 'greptile-apps'}, 'status': 'queued'}]},
        }

    def watch(self):
        (self.dir / 'table.json').write_text(json.dumps(self.table))
        env = dict(os.environ, PATH=f"{self.dir}:{os.environ['PATH']}",
                   GH_TABLE=str(self.dir / 'table.json'), WATCH_PR_POLL='0', WATCH_PR_LIMIT='0')
        return subprocess.run([sys.executable, SCRIPT, '7', self.state], env=env,
                              capture_output=True, text=True)

    def test_watch_should_print_a_new_comment_once_and_skip_my_own(self):
        self.table[f'{P}/pulls/7/comments'] = [comment(1, 'bot', '<b>Fix</b> the line'),
                                               comment(2, 'me', 'done')]
        out = self.watch()
        self.assertEqual((out.returncode, out.stdout), (0, 'inline 1 bot a.rs:3: Fix the line u1\n'))
        self.assertEqual(self.watch().returncode, 1)

    def test_watch_should_skip_the_in_progress_note_and_wake_on_its_edit(self):
        note = '<!-- This is an auto-generated comment: review in progress by coderabbit.ai -->'
        self.table[f'{P}/issues/7/comments'] = [comment(5, 'coderabbitai[bot]', note)]
        self.assertEqual(self.watch().returncode, 1)
        self.table[f'{P}/issues/7/comments'] = [comment(5, 'coderabbitai[bot]', 'Summary', 't2')]
        self.assertEqual(self.watch().stdout, 'comment 5 coderabbitai[bot]: Summary u5\n')

    def test_watch_should_wake_on_an_edited_review(self):
        review = {'id': 9, 'user': {'login': 'bot'}, 'state': 'COMMENTED', 'html_url': 'r9'}
        self.table[f'{P}/pulls/7/reviews'] = [dict(review, body='One')]
        self.assertEqual(self.watch().stdout, 'review 9 COMMENTED bot: One r9\n')
        self.table[f'{P}/pulls/7/reviews'] = [dict(review, body='Two')]
        self.assertEqual(self.watch().stdout, 'review 9 COMMENTED bot: Two r9\n')

    def test_watch_should_wait_for_a_workflow_that_has_not_started(self):
        self.table[f'{P}/commits/abcdef0123/check-runs'] = {'check_runs': [
            {'status': 'completed', 'conclusion': 'skipped', 'name': 'plan', 'html_url': 'u'}]}
        self.table[f'{P}/commits/abcdef0123/check-suites']['check_suites'].append(
            {'app': {'slug': 'github-actions'}, 'status': 'queued'})
        self.assertEqual(self.watch().returncode, 1)

    def test_watch_should_report_a_failed_check_and_then_the_end_of_checks_once(self):
        runs = f'{P}/commits/abcdef0123/check-runs'
        self.table[runs] = {'check_runs': [
            {'status': 'completed', 'conclusion': 'failure', 'name': 'test', 'html_url': 'u'},
            {'status': 'in_progress'}]}
        self.assertEqual(self.watch().stdout, 'checks failed at abcdef01: test u\n')
        self.table[runs] = {'check_runs': [
            {'status': 'completed', 'conclusion': 'success', 'name': 'test', 'html_url': 'u'}]}
        self.assertEqual(self.watch().stdout, 'checks passed at abcdef01: 1 checks\n')
        self.assertEqual(self.watch().returncode, 1)


if __name__ == '__main__':
    unittest.main()
