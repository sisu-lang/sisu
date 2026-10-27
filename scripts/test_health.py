"""Prints the health of a nextest JUnit report: counts, times, the slowest tests and the tests
over their budget (CODING_STANDARDS.md, Tests). A milestone pull request quotes it before and after.

Usage: python3 test_health.py JUNIT.xml
Prints nothing when the report is missing, and one line when it cannot be read. It exits 0.
"""
import sys
import xml.etree.ElementTree as ET

import slow_tests

TOP = 10


def report(junit: str, top: int = TOP) -> str:
    """The report text for the JUnit report text."""
    tests = slow_tests.timed(junit)
    wall = float(ET.fromstring(junit).get('time', '0'))
    binaries: dict[str, list[float]] = {}
    for seconds, _, name in tests:
        binaries.setdefault(name.split(' ', 1)[0], []).append(seconds)
    lines = [f"Tests: {len(tests)}",
             f"Sum of test times: {sum(t[0] for t in tests):.2f} s; wall time of the run: {wall:.2f} s",
             "Per binary:"]
    lines += [f"  {binary}: {len(times)} tests, {sum(times):.2f} s" for binary, times in sorted(binaries.items())]
    lines.append(f"Slowest {top} tests:")
    lines += [f"  {seconds:6.2f} s (budget {budget:g} s)  {name}" for seconds, budget, name in sorted(tests, reverse=True)[:top]]
    lines.append(f"Tests over budget: {len(slow_tests.over_budget(tests))}")
    return '\n'.join(lines)


def main(path: str) -> int:
    try:
        with open(path, encoding='utf-8') as file:
            print(report(file.read()))
    except FileNotFoundError:
        pass
    except (OSError, ET.ParseError, ValueError) as error:
        # An interrupted run can leave the report half written; that is still no reason to fail.
        print(f"test_health: cannot read {path}: {error}")
    return 0


if __name__ == '__main__':
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    sys.exit(main(sys.argv[1]))
