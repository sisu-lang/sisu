"""Unit tests for test_health.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import test_health as th

SCRIPT = Path(__file__).absolute().parent / "test_health.py"

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
    def test_the_report_should_show_counts_times_and_the_slowest_tests(self) -> None:
        self.assertEqual(
            th.report(REPORT, top=2).splitlines(),
            [
                "Tests: 4",
                "Sum of test times: 11.51 s; wall time of the run: 12.50 s",
                "Per binary:",
                "  sisuc: 2 tests, 1.51 s",
                "  sisuc::cases: 2 tests, 10.00 s",
                "Slowest 2 tests:",
                "    7.00 s (budget 4 s)  sisuc::cases run::big.sisu",
                "    3.00 s (budget 4 s)  sisuc::cases run::fib.sisu",
                "Tests over budget: 2",
            ],
        )

    def test_the_report_should_list_ten_tests_by_default_and_no_more_than_it_has(self) -> None:
        self.assertEqual(th.TOP, 10)
        self.assertIn("Slowest 4 tests:", th.report(REPORT).splitlines())

    def test_the_script_should_print_nothing_and_exit_0_for_a_missing_report(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            run = subprocess.run(
                [sys.executable, SCRIPT, Path(directory, "none.xml")],
                capture_output=True,
                text=True,
                check=False,
            )
        self.assertEqual((run.returncode, run.stdout), (0, ""))

    def test_the_script_should_exit_0_for_a_report_that_cannot_be_read(self) -> None:
        with tempfile.NamedTemporaryFile("w", suffix=".xml") as file:
            file.write("<testsuites")
            file.flush()
            run = subprocess.run(
                [sys.executable, SCRIPT, file.name], capture_output=True, text=True, check=False
            )
        self.assertEqual(run.returncode, 0)
        self.assertIn("cannot read", run.stdout)

    def test_summary_should_count_passed_and_failed_tests_without_setup_scripts(self) -> None:
        self.assertEqual(th.summary(REPORT), "Tests: 4 passed, 0 failed")
        failing = """<testsuites name="nextest-run" time="5.000">
<testsuite name="@setup-script:build-runtime">
<testcase name="build-runtime" classname="@setup-script:build-runtime" time="1.000">
<failure message="ignored"/>
</testcase>
</testsuite>
<testsuite name="sisuc">
<testcase name="ok1" classname="sisuc" time="0.010"/>
<testcase name="ok2" classname="sisuc" time="0.010"/>
<testcase name="bad" classname="sisuc" time="0.020"><failure message="boom"/></testcase>
<testcase name="err" classname="sisuc" time="0.020"><error message="err"/></testcase>
</testsuite>
</testsuites>"""
        self.assertEqual(th.summary(failing), "Tests: 2 passed, 2 failed")

    def test_summary_flag_should_print_one_line_and_handle_missing_reports(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            junit = Path(directory, "junit.xml")
            junit.write_text(REPORT, encoding="utf-8")
            run = subprocess.run(
                [sys.executable, SCRIPT, "--summary", junit],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual((run.returncode, run.stdout), (0, "Tests: 4 passed, 0 failed\n"))
            missing = subprocess.run(
                [sys.executable, SCRIPT, "--summary", Path(directory, "none.xml")],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual((missing.returncode, missing.stdout), (0, ""))


if __name__ == "__main__":
    unittest.main()
