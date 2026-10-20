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

    def test_a_test_is_in_the_diff_by_its_function_name_or_case_stem(self):
        diff = ("@@ -9,2 +9,3 @@ fn many_arms() {\n+    let n = 2;\n"
                "+++ b/crates/sisuc/tests/run/fib.out\n")
        for name, mentioned in [("sisuc check::tests::many_arms", True),
                                ("sisuc check::tests::many_arms_wide", False),
                                ("sisuc::cases run::fib.sisu", True),
                                ("sisuc::cases run::fibs.sisu", False)]:
            self.assertEqual(st.in_diff(name, diff), mentioned, name)

    def test_with_diff_only_mentioned_tests_are_listed_and_the_rest_counted(self):
        with tempfile.TemporaryDirectory() as tmp:
            # A pre-commit hook exports GIT_DIR and GIT_INDEX_FILE, which would point git at this
            # repository, and under CI GITHUB_ACTIONS adds a workflow warning line per slow test.
            env = {key: value for key, value in os.environ.items()
                   if not key.startswith('GIT_') and key != 'GITHUB_ACTIONS'}
            git = lambda *args: subprocess.run(['git', '-C', tmp, *args], check=True, capture_output=True, env=env)
            git('init', '-q')
            source = os.path.join(tmp, 'a.rs')
            with open(source, 'w', encoding='utf-8') as file:
                file.write('fn slow() {}\n')
            git('add', '.')
            git('-c', 'user.email=t@t', '-c', 'user.name=t', 'commit', '-qm', 'a')
            with open(source, 'w', encoding='utf-8') as file:
                file.write('fn big() {}\nfn slow() {}\n')
            report = os.path.join(tmp, 'junit.xml')
            with open(report, 'w', encoding='utf-8') as file:
                file.write(REPORT.replace('run::big.sisu', 'run::big'))
            run = subprocess.run([sys.executable, SCRIPT, report, '--diff', 'HEAD'],
                                 capture_output=True, text=True, cwd=tmp, env=env)
        lines = run.stdout.splitlines()
        self.assertEqual(lines[0], "slow_tests: 1 other tests over budget are outside `git diff HEAD`")
        self.assertEqual([line.split()[-1] for line in lines[2:]], ["run::big"])

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
