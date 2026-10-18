"""Warns about the tests in a nextest JUnit report that ran over their time budget
(CODING_STANDARDS.md, Tests). A warning, never a failure: wall-clock time depends on the machine
and its load.

Usage: python3 slow_tests.py JUNIT.xml
Prints nothing when the report is missing or every test is within budget, and one line when the
report cannot be read. Always exits 0. Under GitHub Actions, each slow test is also a workflow
warning.
"""
import os
import sys
import time
import xml.etree.ElementTree as ET

# Seconds on a GitHub-hosted runner, which takes about twice as long as a developer machine. A
# unit test runs in-process; an end-to-end test spawns sisuc, links, and may run under Valgrind,
# and a `tests/run` case, with three builds and two Valgrind runs, takes about 2 s.
UNIT_BUDGET = 1.0
END_TO_END_BUDGET = 4.0


def slow_tests(junit: str) -> list[tuple[float, float, str]]:
    """(seconds, budget, name) of each test over its budget, slowest first."""
    slow = []
    for case in ET.fromstring(junit).iter('testcase'):
        binary = case.get('classname', '')
        if binary.startswith('@'):  # a setup script, not a test
            continue
        # nextest names an integration test binary `<crate>::<binary>`, a crate's own tests `<crate>`.
        budget = END_TO_END_BUDGET if '::' in binary else UNIT_BUDGET
        seconds = float(case.get('time', '0'))
        if seconds > budget:
            slow.append((seconds, budget, f"{binary} {case.get('name')}"))
    return sorted(slow, reverse=True)


def main(path: str) -> None:
    try:
        with open(path, encoding='utf-8') as file:
            junit = file.read()
        slow = slow_tests(junit)
        minutes = (time.time() - os.path.getmtime(path)) / 60
    except FileNotFoundError:
        return
    except (OSError, ET.ParseError, ValueError) as error:
        # An interrupted run can leave the report half written; that is still no reason to fail.
        print(f"slow_tests: cannot read {path}: {error}")
        return
    if not slow:
        return
    print(f"Tests over their time budget (CODING_STANDARDS.md, Tests), in {path} from {minutes:.0f} min ago:")
    for seconds, budget, name in slow:
        print(f"  {seconds:6.2f} s (budget {budget:g} s)  {name}")
        if os.environ.get('GITHUB_ACTIONS') == 'true':
            print(f"::warning title=Slow test::{name} took {seconds:.2f} s; its budget is {budget:g} s")


if __name__ == '__main__':
    main(sys.argv[1])
