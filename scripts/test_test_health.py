"""Unit tests for test_health.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import os
import subprocess
import sys
import tempfile
import unittest

import test_health as th

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'test_health.py')

REPORT = """<testsuites name="nextest-run" time="12.500">
<testsuite name="@setup-script:build-runtime">
<testcase name="build-runtime" classname="@setup-script:build-runtime" time="9.000"/>
</testsuite>
<testsuite name="sisuc">
<testcase name="fast" classname="sisuc" time="0.010"/>
<testcase name="slow" classname="sisuc" time="1.500"/>
</testsuite>
<testsuite name="sisuc::cases">
<testcase name="run::fib.sisu" classname="sisuc::cases" time="3.000"/>
<testcase name="run::big.sisu" classname="sisuc::cases" time="7.000"/>
</testsuite>
</testsuites>"""


class TestHealth(unittest.TestCase):
    def test_the_report_should_show_counts_times_and_the_slowest_tests(self):
        self.assertEqual(th.report(REPORT, top=2).splitlines(), [
            "Tests: 4",
            "Sum of test times: 11.51 s; wall time of the run: 12.50 s",
            "Per binary:",
            "  sisuc: 2 tests, 1.51 s",
            "  sisuc::cases: 2 tests, 10.00 s",
            "Slowest 2 tests:",
            "    7.00 s (budget 4 s)  sisuc::cases run::big.sisu",
            "    3.00 s (budget 4 s)  sisuc::cases run::fib.sisu",
            "Tests over budget: 2",
        ])

    def test_the_script_should_print_nothing_and_exit_0_for_a_missing_report(self):
        with tempfile.TemporaryDirectory() as directory:
            run = subprocess.run([sys.executable, SCRIPT, os.path.join(directory, 'none.xml')],
                                 capture_output=True, text=True)
        self.assertEqual((run.returncode, run.stdout), (0, ''))

    def test_the_script_should_exit_0_for_a_report_that_cannot_be_read(self):
        with tempfile.NamedTemporaryFile('w', suffix='.xml') as file:
            file.write('<testsuites')
            file.flush()
            run = subprocess.run([sys.executable, SCRIPT, file.name], capture_output=True, text=True)
        self.assertEqual(run.returncode, 0)
        self.assertIn('cannot read', run.stdout)


if __name__ == '__main__':
    unittest.main()
