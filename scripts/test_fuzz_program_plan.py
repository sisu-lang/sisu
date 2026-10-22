"""Unit tests for fuzz_program_plan.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import json
import os
import tempfile
import unittest
from unittest import mock

import fuzz_program_plan as fp


def pull_request(labels, action='synchronize', added=None):
    payload = {'action': action, 'pull_request': {'labels': [{'name': n} for n in labels]}}
    if added is not None:
        payload['label'] = {'name': added}
    return payload


class Labels(unittest.TestCase):
    def plan(self, labels, **kw):
        return fp.plan('pull_request', pull_request(labels, **kw), {})

    def test_minutes_and_hours(self):
        self.assertEqual(self.plan(['fuzz-program 45m']), (2700, True))
        self.assertEqual(self.plan(['bug', 'fuzz-program 2h']), (7200, True))
        # The bounds themselves are allowed.
        self.assertEqual(self.plan(['fuzz-program 1m']), (60, True))
        self.assertEqual(self.plan(['fuzz-program 4h']), (14400, True))

    def test_a_bare_label_fuzzes_two_hours(self):
        self.assertEqual(self.plan(['fuzz-program']), (7200, True))

    def test_out_of_range_fails(self):
        for label in ('fuzz-program 0m', 'fuzz-program 6h', 'fuzz-program 241m'):
            with self.subTest(label), self.assertRaisesRegex(ValueError, 'outside'):
                self.plan([label])

    def test_two_labels_fail_naming_both(self):
        with self.assertRaisesRegex(ValueError, 'fuzz-program 45m, fuzz-program$'):
            self.plan(['fuzz-program 45m', 'fuzz-program'])

    def test_unrelated_labels_run_nothing(self):
        for labels in ([], ['bug'], ['fuzz-program 45'], ['fuzz-program 2d'], ['xfuzz-program'],
                       ['fuzz-program  2h'], ['fuzz-program 2h ']):
            with self.subTest(labels):
                self.assertIsNone(self.plan(labels))

    def test_adding_another_label_runs_nothing(self):
        self.assertIsNone(self.plan(['fuzz-program', 'bug'], action='labeled', added='bug'))
        self.assertEqual(
            self.plan(['fuzz-program 45m'], action='labeled', added='fuzz-program 45m'),
            (2700, True))


class Events(unittest.TestCase):
    def test_the_nightly_run_keeps_30_minutes_in_90(self):
        self.assertEqual(fp.plan('schedule', {}, {}), (1800, False))
        self.assertEqual(fp.minutes(1800, False), 90)

    def test_the_longest_run_fits_the_6_hour_job_limit(self):
        self.assertLessEqual(fp.minutes(fp.MOST, True), 360)

    def test_dispatch_takes_its_inputs(self):
        env = {'FUZZ_SECONDS': '600', 'FUZZ_COVERAGE': 'true'}
        self.assertEqual(fp.plan('workflow_dispatch', {}, env), (600, True))
        with self.assertRaisesRegex(ValueError, 'outside'):
            fp.plan('workflow_dispatch', {}, {'FUZZ_SECONDS': '30', 'FUZZ_COVERAGE': 'false'})

    def main(self, labels):
        """Runs main() on a pull_request event with `labels`; returns its status and output."""
        with tempfile.TemporaryDirectory() as tmp:
            event = os.path.join(tmp, 'event.json')
            output = os.path.join(tmp, 'output')
            with open(event, 'w', encoding='utf-8') as f:
                json.dump(pull_request(labels), f)
            open(output, 'w', encoding='utf-8').close()
            env = {'GITHUB_EVENT_NAME': 'pull_request', 'GITHUB_EVENT_PATH': event,
                   'GITHUB_OUTPUT': output}
            with mock.patch.dict(os.environ, env), mock.patch('sys.stdout'):
                status = fp.main()
            with open(output, encoding='utf-8') as f:
                return status, f.read()

    def test_main_writes_the_outputs(self):
        self.assertEqual(self.main(['fuzz-program 45m']),
                         (0, 'seconds=2700\ncoverage=true\nminutes=150\n'))

    def test_main_plans_no_run_for_an_unlabeled_pull_request(self):
        self.assertEqual(self.main(['bug']), (0, ''))


if __name__ == '__main__':
    unittest.main()
