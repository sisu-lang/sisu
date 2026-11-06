"""Warn about the tests in a nextest JUnit report that ran over their time budget.

The budget is in CODING_STANDARDS.md, Tests. A warning, never a failure: wall-clock time depends
on the machine and its load.

Usage: python3 slow_tests.py JUNIT.xml [--diff REF]
Prints nothing when the report is missing or every test is within budget, and one line when the
report cannot be read. Always exits 0. Under GitHub Actions, each slow test is also a workflow
warning. With --diff, it lists only the slow tests whose name `git diff -U0 REF` mentions, the
tests a change added or edited, and counts the rest: on a loaded machine the rest are noise. When
git gives no diff, it says why in one line and lists every slow test.
"""

import argparse
import os
import re
import subprocess
import time
import xml.etree.ElementTree as ET
from pathlib import Path

# Seconds on a GitHub-hosted runner. A unit test runs in-process. An end-to-end test spawns sisu,
# links, and may run under Valgrind. A `tests/run` case, with its builds and Valgrind runs, takes
# about 2 s.
UNIT_BUDGET = 1.0
END_TO_END_BUDGET = 4.0


def timed(junit: str) -> list[tuple[float, float, str]]:
    """Return (seconds, budget, name) of each test in the report, setup scripts left out."""
    tests = []
    for case in ET.fromstring(junit).iter("testcase"):  # noqa: S314  the report is the output of nextest in this repository
        binary = case.get("classname", "")
        if binary.startswith("@"):  # a setup script, not a test
            continue
        # nextest names an integration test binary `<crate>::<binary>`, a crate's own tests
        # `<crate>`.
        budget = END_TO_END_BUDGET if "::" in binary else UNIT_BUDGET
        tests.append((float(case.get("time", "0")), budget, f"{binary} {case.get('name')}"))
    return tests


def over_budget(tests: list[tuple[float, float, str]]) -> list[tuple[float, float, str]]:
    """Return the tests over their budget, slowest first."""
    return sorted((test for test in tests if test[0] > test[1]), reverse=True)


def in_diff(name: str, diff: str) -> bool:
    """Tell whether the diff mentions the test.

    The diff mentions a test by its function name, which a `fn` line or a hunk header carries
    (`.gitattributes` gives Rust files git's `rust` hunk headers), or by the stem of its `tests/`
    case file, such as `fib` for `run::fib.sisu`.
    """
    key = name.split(" ", 1)[-1].rsplit("::", 1)[-1].removesuffix(".sisu")
    return re.search(rf"(?<!\w){re.escape(key)}(?!\w)", diff) is not None


def diff_against(ref: str) -> str | None:
    """Return `git diff -U0 ref`, or print why git gives no diff and return None."""
    try:
        done = subprocess.run(
            ["git", "diff", "-U0", "--no-color", "--no-ext-diff", "--no-textconv", ref],
            capture_output=True,
            text=True,
            errors="surrogateescape",
            check=False,
        )
    except OSError as error:
        print(f"slow_tests: cannot run `git diff {ref}`: {error}")
        return None
    if done.returncode != 0:
        first_line = done.stderr.partition("\n")[0]
        print(f"slow_tests: `git diff {ref}` failed: {first_line}")
        return None
    return done.stdout


def main(path: str, ref: str | None = None) -> None:
    """Print the tests over budget in the report `path`.

    With no `ref`, it prints all of them. With a `ref`, it prints only the tests that the diff
    against `ref` names, and says how many others are over budget.
    """
    try:
        junit = Path(path).read_text(encoding="utf-8")
        tests = timed(junit)
        minutes = (time.time() - Path(path).stat().st_mtime) / 60
    except FileNotFoundError:
        return
    except (OSError, ET.ParseError, ValueError) as error:
        # An interrupted run can leave the report half written; that is still no reason to fail.
        print(f"slow_tests: cannot read {path}: {error}")
        return
    slow = over_budget(tests)
    others = 0
    if ref is not None and slow:
        diff = diff_against(ref)
        if diff is not None:
            mentioned = [test for test in slow if in_diff(test[2], diff)]
            others = len(slow) - len(mentioned)
            slow = mentioned
    if others:
        print(f"slow_tests: {others} other tests over budget are outside `git diff {ref}`")
    if slow:
        print(
            "Tests over their time budget (CODING_STANDARDS.md, Tests), "
            f"in {path} from {minutes:.0f} min ago:"
        )
    for seconds, budget, name in slow:
        print(f"  {seconds:6.2f} s (budget {budget:g} s)  {name}")
        if os.environ.get("GITHUB_ACTIONS") == "true":
            print(
                f"::warning title=Slow test::{name} took {seconds:.2f} s; "
                f"its budget is {budget:g} s"
            )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("junit")
    parser.add_argument("--diff", metavar="REF")
    args = parser.parse_args()
    main(args.junit, args.diff)
