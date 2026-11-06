"""Unit tests for diag_diff.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import contextlib
import io
import json
import os
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import diag_diff as dd

E0032 = ('{"kind":"diagnostic","code":"E0032","severity":"error",'
         '"primary":{"line":2,"column":5},"secondary":[{"line":2,"column":9}]}')
W0001 = ('{"kind":"diagnostic","code":"W0001","severity":"warning",'
         '"primary":{"line":1,"column":1},"secondary":[]}')


def fake_sisu(tmp, name, outputs):
    """A sisu stand-in that prints the JSON lines `outputs` gives a file's stem, or crashes."""
    path = os.path.join(tmp, name)
    cases = ''.join(f"  {stem}) printf '%s\\n' '{out}' >&2;;\n" for stem, out in outputs.items())
    with open(path, 'w', encoding='utf-8') as f:
        f.write(f'#!/bin/sh\ncase $(basename "$4" .sisu) in\n{cases}  *) exit 101;;\nesac\n')
    os.chmod(path, 0o755)
    return path


class DiagDiff(unittest.TestCase):
    def test_reports_each_program_whose_diagnostics_differ(self):
        with tempfile.TemporaryDirectory() as tmp:
            for stem in ['same', 'accepted', 'rejected', 'moved', 'crash']:
                open(os.path.join(tmp, f'{stem}.sisu'), 'w').close()
            moved = E0032.replace('"column":9', '"column":10')
            base = fake_sisu(tmp, 'base', {'same': E0032, 'accepted': E0032, 'rejected': W0001,
                                           'moved': E0032, 'crash': E0032})
            head = fake_sisu(tmp, 'head', {'same': E0032, 'accepted': W0001, 'rejected': E0032,
                                           'moved': moved})
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                status = dd.main(base, head, tmp, full=False)
        self.assertEqual(status, 1)
        lines = out.getvalue().splitlines()
        self.assertEqual([line.split(':')[0] for line in lines if not line.startswith(' ')][:-1],
                         ['newly accepted', 'crash', 'changed', 'newly rejected'])
        self.assertIn('  head: crash 101:0', lines)
        self.assertIn('  head: E0032 2:5 +2:10', lines)
        self.assertEqual(lines[-1], 'diag_diff: 5 programs, 1 changed, 1 crash, '
                                    '1 newly accepted, 1 newly rejected')

    def test_reports_the_programs_of_every_directory_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            for sub in ['a', 'b', 'b/c']:
                os.makedirs(os.path.join(tmp, sub), exist_ok=True)
                open(os.path.join(tmp, sub, 'same.sisu'), 'w').close()
            sisu = fake_sisu(tmp, 'sisu', {'same': E0032})
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                status = dd.main(sisu, sisu, os.path.join(tmp, 'a'), os.path.join(tmp, 'b'),
                                 os.path.join(tmp, 'b', 'c'))
        self.assertEqual(status, 0)
        self.assertEqual(out.getvalue(), 'diag_diff: 3 programs, no differences\n')

    def test_reports_a_program_that_crashes_both_builds(self):
        with tempfile.TemporaryDirectory() as tmp:
            open(os.path.join(tmp, 'crash.sisu'), 'w').close()
            sisu = fake_sisu(tmp, 'sisu', {})
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                status = dd.main(sisu, sisu, tmp)
        self.assertEqual(status, 1)
        lines = out.getvalue().splitlines()
        self.assertTrue(lines[0].startswith('crash: '), lines)
        self.assertEqual(lines[-1], 'diag_diff: 1 programs, 1 crash')

    def test_full_should_report_a_program_whose_help_alone_differs(self):
        """
        GIVEN a program whose diagnostic has the same code and position in both builds
        AND a help that differs
        WHEN diag_diff runs without and with `full`
        THEN only the run with `full` reports it, and shows the help of each build
        """
        with tempfile.TemporaryDirectory() as tmp:
            open(os.path.join(tmp, 'help.sisu'), 'w').close()
            base = fake_sisu(tmp, 'base', {'help': E0032.replace('}]}', '}],"help":"use `match`"}')})
            head = fake_sisu(tmp, 'head', {'help': E0032.replace('}]}', '}],"help":"use a plain `let`"}')})
            runs = []
            for full in (False, True):
                out = io.StringIO()
                with contextlib.redirect_stdout(out):
                    runs.append((dd.main(base, head, tmp, full=full), out.getvalue().splitlines()))
        self.assertEqual(runs[0], (0, ['diag_diff: 1 programs, no differences']))
        status, lines = runs[1]
        self.assertEqual(status, 1)
        self.assertTrue(lines[0].startswith('changed: '), lines)
        self.assertIn('"help": "use `match`"', lines[1])
        self.assertIn('"help": "use a plain `let`"', lines[2])
        self.assertEqual(lines[-1], 'diag_diff: 1 programs, 1 changed')
    def test_full_should_report_each_text_field_that_alone_differs(self):
        """
        GIVEN programs whose diagnostic differs in one field each: the message, the label, a
        secondary label, the message of a fix, an edit of a fix and the safe flag of a fix
        WHEN diag_diff runs from the command line with `--codes`, with no flag and with `--full`
        THEN the run with `--codes` reports none of them, and each other run reports each of them
        """
        def diagnostic(message='m', label='l', other='o', fix='f', replacement='', safe=True):
            return json.dumps({
                'kind': 'diagnostic', 'code': 'E0131', 'severity': 'error', 'message': message,
                'primary': {'line': 1, 'column': 1, 'label': label},
                'secondary': [{'line': 2, 'column': 1, 'label': other}],
                'fixes': [{'message': fix, 'safe': safe,
                           'edits': [{'start': 0, 'end': 1, 'replacement': replacement}]}]})
        changed = {'message': {'message': 'n'}, 'label': {'label': 'k'},
                   'secondary': {'other': 'p'}, 'fix': {'fix': 'g'},
                   'edit': {'replacement': 'x'}, 'safe': {'safe': False}}
        script = os.path.join(os.path.dirname(os.path.abspath(dd.__file__)), 'diag_diff.py')
        with tempfile.TemporaryDirectory() as tmp:
            for stem in changed:
                open(os.path.join(tmp, f'{stem}.sisu'), 'w').close()
            base = fake_sisu(tmp, 'base', {stem: diagnostic() for stem in changed})
            head = fake_sisu(tmp, 'head', {stem: diagnostic(**f) for stem, f in changed.items()})
            runs = [subprocess.run([sys.executable, script, *flag, base, head, tmp],
                                   capture_output=True, text=True)
                    for flag in (['--codes'], [], ['--full'])]
        self.assertEqual((runs[0].returncode, runs[0].stdout),
                         (0, 'diag_diff: 6 programs, no differences\n'))
        for run in runs[1:]:
            self.assertEqual(run.returncode, 1)
            reported = {Path(line.split(': ', 1)[1]).stem
                        for line in run.stdout.splitlines() if line.startswith('changed: ')}
            self.assertEqual(reported, set(changed))


    def test_jobs_should_bound_the_sisu_processes_that_run_at_once(self):
        self.assertLessEqual(max(peak_runs(lambda sisu, tmp: dd.main(sisu, sisu, tmp, jobs=2))), 2)

    def test_main_should_run_at_most_two_sisu_processes_by_default(self):
        self.assertLessEqual(max(peak_runs(lambda sisu, tmp: dd.main(sisu, sisu, tmp))), 2)

    def test_command_line_should_bound_the_sisu_processes_by_jobs(self):
        script = os.path.join(os.path.dirname(os.path.abspath(dd.__file__)), 'diag_diff.py')
        for flag, limit in [([], 2), (['--jobs=1'], 1)]:
            def run(sisu, tmp, flag=flag):
                subprocess.run([sys.executable, script, *flag, sisu, sisu, tmp], check=True,
                               capture_output=True)
            self.assertLessEqual(max(peak_runs(run)), limit, flag)


def peak_runs(run):
    """The count of fake sisu runs at each start while `run(sisu, dir)` checks six programs."""
    with tempfile.TemporaryDirectory() as tmp:
        for n in range(6):
            open(os.path.join(tmp, f'p{n}.sisu'), 'w').close()
        # Each run marks itself, counts the runs that have a mark, waits, and removes its mark.
        d = shlex.quote(tmp)
        sisu = os.path.join(tmp, 'sisu')
        with open(sisu, 'w', encoding='utf-8') as f:
            f.write(f'#!/bin/sh\ntouch {d}/run.$$\n'
                    f'ls {d} | grep -c "^run" >> {d}/counts\nsleep 0.1\nrm {d}/run.$$\n')
        os.chmod(sisu, 0o755)
        with contextlib.redirect_stdout(io.StringIO()):
            run(sisu, tmp)
        counts = [int(n) for n in Path(tmp, 'counts').read_text().split()]
    assert len(counts) == 12, counts
    return counts


if __name__ == '__main__':
    unittest.main()
