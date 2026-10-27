"""Unit tests for slow_tests.py: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import os
import subprocess
import sys
import tempfile
import unittest

import git_fixture
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
        with git_fixture.repo({'a.rs': 'fn slow() {}\n'}) as tmp:
            with open(os.path.join(tmp, 'a.rs'), 'w', encoding='utf-8') as file:
                file.write('fn big() {}\nfn slow() {}\n')
            report = os.path.join(tmp, 'junit.xml')
            with open(report, 'w', encoding='utf-8') as file:
                file.write(REPORT.replace('run::big.sisu', 'run::big'))
            run = subprocess.run([sys.executable, SCRIPT, report, '--diff', 'HEAD'],
                                 capture_output=True, text=True, cwd=tmp, env=git_fixture.env())
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


HARNESS = r"""datatest_stable::harness! {
    { test = run, root = "tests/run", pattern = r"\.sisu$" },
    { test = run_fail, root = "tests/run-fail", pattern = r"\.sisu$" },
    { test = site_run, root = "../../site/programs/run", pattern = r"\.sisu$" },
    { test = every_program_certifies, root = "tests", pattern = r"^(run|run-fail|programs)/[^/]+\.sisu$" },
    { test = every_program_certifies, root = "../../bench", pattern = r"main\.sisu$" },
}
"""
MANIFEST = '[package]\nname = "sisuc"\n\n[[bin]]\nname = "sisu"\npath = "src/main.rs"\n'


def rust(body: str, *, module: str = 'tests', name: str = 'pattern_spans') -> str:
    """A Rust file with one test `name` in the inline module `module`; `body` is its only line."""
    return f"mod {module} {{\n    #[test]\n    fn {name}() {{\n        {body}\n    }}\n}}\n"


def report(*tests: tuple[str, str, float]) -> str:
    """A JUnit report of (binary, name, seconds)."""
    cases = ''.join(f'<testcase name="{name}" classname="{binary}" time="{seconds}"/>'
                    for binary, name, seconds in tests)
    return f'<testsuites><testsuite name="x">{cases}</testsuite></testsuites>'


def fail_changed(files: dict[str, str], edits: dict[str, str], *tests: tuple[str, str, float]):
    """Runs `--fail-changed HEAD` on a repository of `files` whose working tree holds `edits`.
    Returns the exit code and the "binary name" of each failure line."""
    with git_fixture.repo({'crates/sisuc/Cargo.toml': MANIFEST, **files}) as tmp:
        for name, text in edits.items():
            git_fixture.write(tmp, name, text)
        git_fixture.write(tmp, 'junit.xml', report(*tests))
        run = subprocess.run([sys.executable, SCRIPT, os.path.join(tmp, 'junit.xml'), '--fail-changed', 'HEAD'],
                             capture_output=True, text=True, cwd=tmp, env=git_fixture.env())
    failed = [line.split(' took ')[0] for line in run.stdout.splitlines() if ' limit for a changed test ' in line]
    return run, failed


class FailChanged(unittest.TestCase):
    def test_a_changed_test_over_twice_its_budget_should_fail(self):
        lib = 'crates/sisuc/src/parser.rs'
        run, failed = fail_changed({lib: rust('1;')}, {lib: rust('2;')}, ('sisuc', 'parser::tests::pattern_spans', 2.5))
        self.assertEqual((run.returncode, failed), (1, ['sisuc parser::tests::pattern_spans']))
        self.assertIn("sisuc parser::tests::pattern_spans took 2.50 s; its budget is 1 s and the limit for a "
                      "changed test is 2 s: shrink its input or split it (CODING_STANDARDS.md, Tests)",
                      run.stdout)

    def test_the_slow_check_should_fail_only_for_a_changed_test(self):
        lib = 'crates/sisuc/src/parser.rs'
        other = 'crates/sisuc/src/check.rs'
        files = {lib: rust('1;'), other: rust('1;')}
        for row, edits, seconds, expected in [
                ('changed and over twice its budget', {lib: rust('2;')}, 2.5, 1),
                ('changed and over its budget only', {lib: rust('2;')}, 1.5, 0),
                ('unchanged and over twice its budget', {other: rust('2;')}, 2.5, 0)]:
            run, failed = fail_changed(files, edits, ('sisuc', 'parser::tests::pattern_spans', seconds))
            self.assertEqual(run.returncode, expected, f'{row}: {run.stdout}')
            self.assertIn('parser::tests::pattern_spans', run.stdout, f'{row}: it is listed as slow')

    def test_a_junit_name_should_map_to_its_own_file(self):
        cases_rs = 'crates/sisuc/tests/cases.rs'
        cli = 'crates/sisuc/tests/cli.rs'
        for row, files, edits, tests, expected in [
                ('two same-named tests in different files: only the changed one',
                 {'crates/sisuc/src/parser.rs': rust('1;'), 'crates/sisuc/src/check.rs': rust('1;')},
                 {'crates/sisuc/src/parser.rs': rust('2;')},
                 [('sisuc', 'parser::tests::pattern_spans', 9), ('sisuc', 'check::tests::pattern_spans', 9)],
                 ['sisuc parser::tests::pattern_spans']),
                ('a file module below the changed file is another file',
                 {'crates/sisuc/src/check.rs': rust('1;'), 'crates/sisuc/src/check/patterns.rs': rust('1;')},
                 {'crates/sisuc/src/check.rs': rust('2;')},
                 [('sisuc', 'check::tests::pattern_spans', 9), ('sisuc', 'check::patterns::tests::pattern_spans', 9)],
                 ['sisuc check::tests::pattern_spans']),
                ('a src/check/mod.rs test is the module check',
                 {'crates/sisuc/src/check/mod.rs': rust('1;'), 'crates/sisuc/src/other.rs': rust('1;')},
                 {'crates/sisuc/src/check/mod.rs': rust('2;')},
                 [('sisuc', 'check::tests::pattern_spans', 9), ('sisuc', 'other::tests::pattern_spans', 9)],
                 ['sisuc check::tests::pattern_spans']),
                ('a module named tests below src/ is not a tests/ target',
                 {'crates/sisuc/src/check/tests/mod.rs': rust('1;'), 'crates/sisuc/src/other.rs': rust('1;')},
                 {'crates/sisuc/src/check/tests/mod.rs': rust('2;')},
                 [('sisuc', 'check::tests::tests::pattern_spans', 9), ('sisuc', 'other::tests::pattern_spans', 9)],
                 ['sisuc check::tests::tests::pattern_spans']),
                ('a src/lib.rs test has no module prefix',
                 {'crates/sisuc/src/lib.rs': rust('1;'), 'crates/sisuc/src/other.rs': rust('1;')},
                 {'crates/sisuc/src/lib.rs': rust('2;')},
                 [('sisuc', 'tests::pattern_spans', 9), ('sisuc', 'other::tests::pattern_spans', 9)],
                 ['sisuc tests::pattern_spans']),
                ('a src/main.rs test is in the bin binary',
                 {'crates/sisuc/src/main.rs': rust('1;'), 'crates/sisuc/src/lib.rs': rust('1;')},
                 {'crates/sisuc/src/main.rs': rust('2;')},
                 [('sisuc::bin/sisu', 'tests::pattern_spans', 9), ('sisuc', 'tests::pattern_spans', 9)],
                 ['sisuc::bin/sisu tests::pattern_spans']),
                ('a tests/<bin>.rs test is in its own binary',
                 {cli: rust('1;'), 'crates/sisuc/src/lib.rs': rust('1;')},
                 {cli: rust('2;')},
                 [('sisuc::cli', 'tests::pattern_spans', 9), ('sisuc', 'tests::pattern_spans', 9)],
                 ['sisuc::cli tests::pattern_spans']),
                ('a tests/<bin>/x.rs test has the prefix x::',
                 {'crates/sisuc/tests/cli/x.rs': rust('1;'), 'crates/sisuc/tests/cli.rs': rust('1;')},
                 {'crates/sisuc/tests/cli/x.rs': rust('2;')},
                 [('sisuc::cli', 'x::tests::pattern_spans', 9), ('sisuc::cli', 'tests::pattern_spans', 9)],
                 ['sisuc::cli x::tests::pattern_spans']),
                ('a changed run_fail case at 9 s fails',
                 {cases_rs: HARNESS, 'crates/sisuc/tests/run-fail/a.sisu': 'x\n'},
                 {'crates/sisuc/tests/run-fail/a.sisu': 'y\n'},
                 [('sisuc::cases', 'run_fail::a.sisu', 9)], ['sisuc::cases run_fail::a.sisu']),
                ('a changed site_run case',
                 {cases_rs: HARNESS, 'site/programs/run/a.sisu': 'x\n'},
                 {'site/programs/run/a.sisu': 'y\n'},
                 [('sisuc::cases', 'site_run::a.sisu', 9)], ['sisuc::cases site_run::a.sisu']),
                ('an every_program_certifies case under bench/',
                 {cases_rs: HARNESS, 'bench/fib/main.sisu': 'x\n'},
                 {'bench/fib/main.sisu': 'y\n'},
                 [('sisuc::cases', 'every_program_certifies::fib/main.sisu', 9)],
                 ['sisuc::cases every_program_certifies::fib/main.sisu']),
                ('a case whose .out file changed',
                 {cases_rs: HARNESS, 'crates/sisuc/tests/run/a.sisu': 'x\n', 'crates/sisuc/tests/run/a.out': '1\n'},
                 {'crates/sisuc/tests/run/a.out': '2\n'},
                 [('sisuc::cases', 'run::a.sisu', 9)], ['sisuc::cases run::a.sisu']),
                ('an unchanged case only warns',
                 {cases_rs: HARNESS, 'crates/sisuc/tests/run/a.sisu': 'x\n', 'crates/sisuc/tests/run/b.sisu': 'x\n'},
                 {'crates/sisuc/tests/run/b.sisu': 'y\n'},
                 [('sisuc::cases', 'run::a.sisu', 9)], [])]:
            run, failed = fail_changed(files, edits, *tests)
            self.assertEqual(failed, expected, f'{row}: {run.stdout}{run.stderr}')
            self.assertEqual(run.returncode, int(bool(expected)), row)

    def test_a_report_with_no_test_should_print_one_note_and_pass(self):
        run, failed = fail_changed({}, {}, ('@setup-script:build-runtime', 'build-runtime', 9))
        self.assertEqual((run.returncode, run.stdout, failed), (
            0, "slow_tests: the report holds no test, so no changed test was checked\n", []))


if __name__ == '__main__':
    unittest.main()
