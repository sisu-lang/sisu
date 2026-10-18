"""Unit tests for slow_tests.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import os
import subprocess
import sys
import tempfile
import unittest

import slow_tests as st

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'slow_tests.py')

REPORT = """<testsuites>
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


class SlowTests(unittest.TestCase):
    def test_each_kind_has_its_budget_and_scripts_are_skipped(self):
        self.assertEqual(st.slow_tests(REPORT), [
            (7.0, st.END_TO_END_BUDGET, "sisuc::cases run::big.sisu"),
            (1.5, st.UNIT_BUDGET, "sisuc slow"),
        ])

    def test_an_unusable_report_never_fails_the_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            for name, text in [('missing', None), ('empty', ''), ('truncated', '<testsuites><testsuite>'),
                               ('bad_time', '<testsuites><testcase classname="sisuc" name="t" time="x"/></testsuites>')]:
                path = os.path.join(tmp, f'{name}.xml')
                if text is not None:
                    with open(path, 'w', encoding='utf-8') as file:
                        file.write(text)
                run = subprocess.run([sys.executable, SCRIPT, path], capture_output=True, text=True)
                self.assertEqual((run.returncode, run.stderr), (0, ''), name)
                self.assertEqual(run.stdout.startswith('slow_tests: cannot read'), text is not None, name)


if __name__ == '__main__':
    unittest.main()
