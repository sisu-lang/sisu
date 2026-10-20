"""Warns about the tests in a nextest JUnit report that ran over their time budget
(CODING_STANDARDS.md, Tests). A warning, never a failure: wall-clock time depends on the machine
and its load.

Usage: python3 slow_tests.py JUNIT.xml [--diff REF]
Prints nothing when the report is missing or every test is within budget, and one line when the
report cannot be read. Always exits 0. Under GitHub Actions, each slow test is also a workflow
warning. With --diff, it lists only the slow tests whose name `git diff -U0 REF` mentions, the
tests a change added or edited, and counts the rest: on a loaded machine the rest are noise.
"""
import os
import re
import subprocess
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


def in_diff(name: str, diff: str) -> bool:
    """Whether the diff mentions the test: its function name, which a `fn` line or a hunk header
    carries (`.gitattributes` gives Rust files git's `rust` hunk headers), or the stem of its
    `tests/` case file, such as `fib` for `run::fib.sisu`."""
    key = name.split(' ', 1)[-1].rsplit('::', 1)[-1].removesuffix('.sisu')
    return re.search(rf'(?<!\w){re.escape(key)}(?!\w)', diff) is not None


def main(path: str, ref: str | None = None) -> None:
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
    others = 0
    if ref is not None and slow:
        diff = subprocess.run(['git', 'diff', '-U0', ref], capture_output=True, text=True).stdout
        mentioned = [test for test in slow if in_diff(test[2], diff)]
        others = len(slow) - len(mentioned)
        slow = mentioned
    if others:
        print(f"slow_tests: {others} other tests over budget are outside `git diff {ref}`")
    if not slow:
        return
    print(f"Tests over their time budget (CODING_STANDARDS.md, Tests), in {path} from {minutes:.0f} min ago:")
    for seconds, budget, name in slow:
        print(f"  {seconds:6.2f} s (budget {budget:g} s)  {name}")
        if os.environ.get('GITHUB_ACTIONS') == 'true':
            print(f"::warning title=Slow test::{name} took {seconds:.2f} s; its budget is {budget:g} s")


if __name__ == '__main__':
    main(sys.argv[1], sys.argv[3] if sys.argv[2:3] == ['--diff'] else None)
