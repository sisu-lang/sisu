"""Print the health of a nextest JUnit report: counts, times, the slowest tests and the tests
over their budget (CODING_STANDARDS.md, Tests). A milestone pull request quotes it before and after.

Usage: python3 test_health.py [--summary] JUNIT.xml
Prints nothing when the report is missing, and one line when it cannot be read. It exits 0.
"""

import argparse
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

import slow_tests

TOP = 10


def summary(junit: str) -> str:
    """Return a one-line pass and fail count for the JUnit report, setup scripts left out."""
    root = ET.fromstring(junit)  # noqa: S314  the report is the output of nextest in this repository
    cases = [
        case for case in root.iter("testcase") if not case.get("classname", "").startswith("@")
    ]
    failed = sum(
        1 for case in cases if case.find("failure") is not None or case.find("error") is not None
    )
    passed = len(cases) - failed
    return f"Tests: {passed} passed, {failed} failed"


def report(junit: str, top: int = TOP) -> str:
    """The report text for the JUnit report text."""
    tests = slow_tests.timed(junit)
    wall = float(ET.fromstring(junit).get("time", "0"))  # noqa: S314  the report is the output of nextest in this repository
    binaries: dict[str, list[float]] = {}
    for seconds, _, name in tests:
        binaries.setdefault(name.split(" ", 1)[0], []).append(seconds)
    lines = [
        f"Tests: {len(tests)}",
        f"Sum of test times: {sum(t[0] for t in tests):.2f} s; wall time of the run: {wall:.2f} s",
        "Per binary:",
    ]
    lines += [
        f"  {binary}: {len(times)} tests, {sum(times):.2f} s"
        for binary, times in sorted(binaries.items())
    ]
    lines.append(f"Slowest {min(top, len(tests))} tests:")
    lines += [
        f"  {seconds:6.2f} s (budget {budget:g} s)  {name}"
        for seconds, budget, name in sorted(tests, reverse=True)[:top]
    ]
    lines.append(f"Tests over budget: {len(slow_tests.over_budget(tests))}")
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    """Run the command line `argv`, and exit 0."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument(
        "--summary", action="store_true", help="print only the pass and fail counts"
    )
    parser.add_argument("junit", help="path to the nextest JUnit report")
    args = parser.parse_args(argv)
    try:
        junit = Path(args.junit).read_text(encoding="utf-8")
        print(summary(junit) if args.summary else report(junit))
    except FileNotFoundError:
        pass
    except (OSError, ET.ParseError, ValueError) as error:
        # An interrupted run can leave the report half written; that is still no reason to fail.
        print(f"test_health: cannot read {args.junit}: {error}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
